//! Reading a file's container, duration and streams with ffprobe, and keeping the results.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::db::unix_now;
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

mod chapters;
mod stored;
mod storing;

pub use chapters::*;
pub use stored::store;
pub use storing::*;

const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
/// ffprobe processes at once. Each mostly waits on the disk or the NAS.
const PARALLEL_PROBES: usize = 4;
const TEXT_SUBTITLES: &[&str] = &["subrip", "ass", "ssa", "mov_text", "webvtt", "text"];

/// What ffprobe says about a video file, stored as JSON with the file's probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// ffprobe's format name, e.g. "matroska,webm" or "mov,mp4,m4a,3gp,3g2,mj2".
    pub container: Option<String>,
    /// The length in seconds; None when ffprobe can't tell.
    pub duration_seconds: Option<f64>,
    /// The overall bit rate in bits per second; None when ffprobe can't tell.
    pub bit_rate: Option<i64>,
    /// The video streams, cover art left out.
    pub video: Vec<VideoStream>,
    /// The audio streams, in the file's order.
    pub audio: Vec<AudioStream>,
    /// The subtitle streams, in the file's order.
    pub subtitles: Vec<SubtitleStream>,
}

/// A video stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoStream {
    /// The stream's index in the file, as ffmpeg's `-map 0:<index>` takes it.
    pub index: i64,
    /// ffprobe's codec name, such as "h264" or "hevc"; "unknown" when ffprobe has none.
    pub codec: String,
    /// The codec profile, such as "Main 10".
    pub profile: Option<String>,
    /// The width in pixels.
    pub width: Option<i64>,
    /// The height in pixels.
    pub height: Option<i64>,
    /// ffprobe's pixel format, such as "yuv420p10le".
    pub pixel_format: Option<String>,
    /// Frames per second, to three decimals (23.976); None when ffprobe says 0/0.
    pub frame_rate: Option<f64>,
    /// "Dolby Vision", "HDR10" or "HLG"; `None` for SDR.
    pub hdr: Option<String>,
    /// For Dolby Vision, its profile and the picture players without it fall back to. Probes
    /// stored before this was recorded lack it, and are probed again.
    #[serde(default)]
    pub dolby_vision: Option<DolbyVision>,
}

/// What a Dolby Vision stream's configuration record says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DolbyVision {
    /// 5, 7, 8 and so on; `None` if the file doesn't say.
    pub profile: Option<i64>,
    /// What a player that ignores Dolby Vision shows: "hdr10", "hlg" or "sdr"; `None` when the
    /// base layer is Dolby Vision's own (profile 5), shown with wrong colours.
    pub fallback: Option<String>,
}

/// What a video shows on a player that doesn't do Dolby Vision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    Sdr,
    Hdr10,
    Hlg,
    /// Dolby Vision with no other picture to fall back to, or one not known.
    DolbyVisionOnly,
}

impl VideoStream {
    /// What this stream shows on a player that doesn't do Dolby Vision.
    pub fn picture(&self) -> Picture {
        match self.hdr.as_deref() {
            None => Picture::Sdr,
            Some("HDR10") => Picture::Hdr10,
            Some("HLG") => Picture::Hlg,
            _ => match self
                .dolby_vision
                .as_ref()
                .and_then(|d| d.fallback.as_deref())
            {
                Some("hdr10") => Picture::Hdr10,
                Some("hlg") => Picture::Hlg,
                Some("sdr") => Picture::Sdr,
                _ => Picture::DolbyVisionOnly,
            },
        }
    }
}

/// An audio stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioStream {
    /// The stream's index in the file, as ffmpeg's `-map 0:<index>` takes it.
    pub index: i64,
    /// ffprobe's codec name, such as "aac" or "truehd"; "unknown" when ffprobe has none.
    pub codec: String,
    /// The codec profile, such as "LC" or "DTS-HD MA".
    pub profile: Option<String>,
    /// How many channels it has.
    pub channels: Option<i64>,
    /// ffprobe's channel layout, such as "stereo" or "5.1(side)".
    pub channel_layout: Option<String>,
    /// The language tag, such as "eng"; None when it's missing, empty or "und".
    pub language: Option<String>,
    /// The stream's title tag, such as "Commentary"; None when it's missing or empty.
    pub title: Option<String>,
    /// Whether the file marks it as the default.
    pub default: bool,
}

/// A subtitle stream inside the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubtitleStream {
    /// The stream's index in the file, as ffmpeg's `-map 0:<index>` takes it.
    pub index: i64,
    /// ffprobe's codec name, such as "subrip" or "hdmv_pgs_subtitle"; "unknown" when it has none.
    pub codec: String,
    /// The language tag, such as "eng"; None when it's missing, empty or "und".
    pub language: Option<String>,
    /// The stream's title tag, such as "SDH"; None when it's missing or empty.
    pub title: Option<String>,
    /// Whether the file marks it as the default.
    pub default: bool,
    /// Whether the file marks it as forced: only the parts in another language.
    pub forced: bool,
    /// Text (SRT, ASS, WebVTT…) rather than pictures (PGS, VobSub).
    pub text: bool,
}

/// Why a file has no probe result; only `Failed` is stored.
#[derive(Debug)]
pub enum ProbeError {
    /// ffprobe itself can't run; nothing about the file is known.
    Unavailable(anyhow::Error),
    /// Took too long, probably a slow disk; worth trying again later.
    TimedOut,
    /// The file couldn't be read (share down, permissions) or ffprobe died without a verdict;
    /// says nothing about the file, so it is tried again later.
    Unreadable(String),
    /// ffprobe ran and rejected the file.
    Failed(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(err) => write!(f, "ffprobe can't run: {err:#}"),
            Self::TimedOut => write!(f, "ffprobe took over {} s", PROBE_TIMEOUT.as_secs()),
            Self::Unreadable(reason) => write!(f, "couldn't read the file: {reason}"),
            Self::Failed(reason) => write!(f, "ffprobe rejected the file: {reason}"),
        }
    }
}

/// Runs ffprobe on one file, within a minute.
///
/// # Errors
///
/// [`ProbeError::Failed`] only when ffprobe rejected a file it could read; the other cases
/// say nothing about the file.
pub async fn probe(ffprobe: &Path, file: &Path) -> Result<MediaInfo, ProbeError> {
    let output = run_ffprobe(ffprobe, file).await?;
    parse(&output).map_err(|err| ProbeError::Failed(format!("{err:#}")))
}

/// ffprobe's JSON for a file: format, streams and chapters.
async fn run_ffprobe(ffprobe: &Path, file: &Path) -> Result<Vec<u8>, ProbeError> {
    // Only a file we can open is worth asking ffprobe about: a dropped share or a permissions
    // problem would otherwise be recorded as a broken file and never retried.
    if let Err(err) = tokio::fs::File::open(file).await {
        return Err(ProbeError::Unreadable(err.to_string()));
    }
    // "file:" keeps a name with a colon from being read as a protocol.
    let mut input = std::ffi::OsString::from("file:");
    input.push(file);
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ])
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(PROBE_TIMEOUT, command.output()).await {
        Err(_) => return Err(ProbeError::TimedOut),
        Ok(Err(err)) => {
            let err = anyhow::Error::from(err).context(format!("starting {}", ffprobe.display()));
            return Err(ProbeError::Unavailable(err));
        }
        Ok(Ok(output)) => output,
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        // Killed (out of memory, say) or silent: no verdict on the file.
        if output.status.code().is_none() || stderr.is_empty() {
            let reason = format!("ffprobe ended without a verdict ({})", output.status);
            return Err(ProbeError::Unreadable(reason));
        }
        // A share that dropped mid-probe looks like a read error; only a file that is still
        // readable has really been rejected.
        if let Err(err) = tokio::fs::File::open(file).await {
            return Err(ProbeError::Unreadable(err.to_string()));
        }
        return Err(ProbeError::Failed(stderr.chars().take(500).collect()));
    }
    Ok(output.stdout)
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    streams: Vec<RawStream>,
    format: Option<RawFormat>,
}

#[derive(Deserialize)]
struct RawFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Deserialize)]
struct RawStream {
    index: i64,
    codec_type: Option<String>,
    codec_name: Option<String>,
    profile: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    pix_fmt: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    channels: Option<i64>,
    channel_layout: Option<String>,
    color_transfer: Option<String>,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    disposition: HashMap<String, i64>,
    #[serde(default)]
    side_data_list: Vec<HashMap<String, Value>>,
}

impl RawStream {
    /// Tag names vary in case between containers ("language", "LANGUAGE").
    fn tag(&self, name: &str) -> Option<String> {
        self.tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty() && v != "und")
    }

    fn flag(&self, name: &str) -> bool {
        self.disposition.get(name).is_some_and(|&v| v != 0)
    }

    fn dovi(&self) -> Option<&HashMap<String, Value>> {
        self.side_data_list.iter().find(|d| {
            d.get("side_data_type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.contains("DOVI"))
        })
    }

    fn dolby_vision(&self) -> Option<DolbyVision> {
        let record = self.dovi()?;
        let number = |key: &str| record.get(key).and_then(Value::as_i64);
        // dv_bl_signal_compatibility_id: what the base layer is on its own.
        let fallback = match number("dv_bl_signal_compatibility_id") {
            Some(1 | 6) => Some("hdr10"),
            Some(2) => Some("sdr"),
            Some(4) => Some("hlg"),
            _ => None,
        };
        Some(DolbyVision {
            profile: number("dv_profile"),
            fallback: fallback.map(Into::into),
        })
    }

    fn hdr(&self) -> Option<String> {
        if self.dovi().is_some() {
            return Some("Dolby Vision".into());
        }
        match self.color_transfer.as_deref() {
            Some("smpte2084") => Some("HDR10".into()),
            Some("arib-std-b67") => Some("HLG".into()),
            _ => None,
        }
    }
}

/// "24000/1001" as 23.976; `None` for "0/0".
fn frame_rate(rate: Option<&str>) -> Option<f64> {
    let (num, den) = rate?.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    (num > 0.0 && den > 0.0).then(|| (num / den * 1000.0).round() / 1000.0)
}

/// Reads ffprobe's JSON output (`-show_format -show_streams`).
///
/// # Errors
///
/// Fails when the output isn't JSON of that shape.
pub fn parse(json: &[u8]) -> anyhow::Result<MediaInfo> {
    let raw: Raw = serde_json::from_slice(json).context("reading ffprobe's output")?;
    let format = raw.format;
    let mut info = MediaInfo {
        container: format.as_ref().and_then(|f| f.format_name.clone()),
        duration_seconds: format
            .as_ref()
            .and_then(|f| f.duration.as_deref()?.parse().ok()),
        bit_rate: format
            .as_ref()
            .and_then(|f| f.bit_rate.as_deref()?.parse().ok()),
        video: Vec::new(),
        audio: Vec::new(),
        subtitles: Vec::new(),
    };
    for s in &raw.streams {
        // ffprobe leaves out codec_name for codecs it doesn't know; keep the stream anyway, so
        // a file doesn't look like it has no audio.
        let codec = s.codec_name.clone().unwrap_or_else(|| "unknown".into());
        match s.codec_type.as_deref() {
            // Cover art is stored as a one-frame video stream.
            Some("video") if !s.flag("attached_pic") => info.video.push(VideoStream {
                index: s.index,
                codec,
                profile: s.profile.clone(),
                width: s.width,
                height: s.height,
                pixel_format: s.pix_fmt.clone(),
                frame_rate: frame_rate(s.avg_frame_rate.as_deref())
                    .or_else(|| frame_rate(s.r_frame_rate.as_deref())),
                hdr: s.hdr(),
                dolby_vision: s.dolby_vision(),
            }),
            Some("audio") => info.audio.push(AudioStream {
                index: s.index,
                codec,
                profile: s.profile.clone(),
                channels: s.channels,
                channel_layout: s.channel_layout.clone(),
                language: s.tag("language"),
                title: s.tag("title"),
                default: s.flag("default"),
            }),
            Some("subtitle") => info.subtitles.push(SubtitleStream {
                index: s.index,
                text: TEXT_SUBTITLES.contains(&codec.as_str()),
                codec,
                language: s.tag("language"),
                title: s.tag("title"),
                default: s.flag("default"),
                forced: s.flag("forced"),
            }),
            _ => {}
        }
    }
    Ok(info)
}

#[cfg(test)]
pub mod testing {
    use std::path::{Path, PathBuf};

    /// A 2-second H.264 clip with stereo English AAC and a forced French SRT, made with the
    /// local ffmpeg (a test prerequisite, like ffprobe).
    /// A `seconds`-long 320x240 H.264 clip with stereo AAC, for transcoding tests.
    pub fn make_long_clip(dir: &Path, seconds: u32) -> PathBuf {
        let clip = dir.join("long.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size=320x240:rate=24:duration={seconds}"))
            .args(["-f", "lavfi", "-i"])
            .arg(format!("sine=frequency=440:duration={seconds}"))
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .args(["-c:a", "aac", "-ac", "2"])
            .arg(&clip)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        clip
    }

    pub fn make_clip(dir: &Path) -> PathBuf {
        let srt = dir.join("s.srt");
        std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:01,000\nHello\n").unwrap();
        let clip = dir.join("clip.mkv");
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=24:duration=2",
            ])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2", "-i"])
            .arg(&srt)
            .args([
                "-map", "0", "-map", "1", "-map", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p",
            ])
            .args([
                "-c:a",
                "aac",
                "-ac",
                "2",
                "-c:s",
                "srt",
                "-metadata:s:a:0",
                "language=eng",
            ])
            .args([
                "-metadata:s:s:0",
                "language=fre",
                "-disposition:s:0",
                "forced",
            ])
            .arg(&clip)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        clip
    }
}

#[cfg(test)]
mod tests {
    use super::testing::make_clip;
    use super::*;
    use crate::db;
    use crate::library::{LibraryConfig, LibraryKind, sync_config};

    #[tokio::test]
    async fn probes_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        let info = probe(Path::new("ffprobe"), &clip).await.unwrap();
        assert_eq!(info.container.as_deref(), Some("matroska,webm"));
        assert!((info.duration_seconds.unwrap() - 2.0).abs() < 0.1);
        assert_eq!(
            info.video,
            vec![VideoStream {
                index: 0,
                codec: "h264".into(),
                profile: Some("High".into()),
                width: Some(320),
                height: Some(240),
                pixel_format: Some("yuv420p".into()),
                frame_rate: Some(24.0),
                hdr: None,
                dolby_vision: None,
            }]
        );
        assert_eq!(info.audio.len(), 1);
        assert_eq!(
            (info.audio[0].codec.as_str(), info.audio[0].channels),
            ("aac", Some(2))
        );
        assert_eq!(info.audio[0].language.as_deref(), Some("eng"));
        assert_eq!(
            info.subtitles,
            vec![SubtitleStream {
                index: 2,
                codec: "subrip".into(),
                language: Some("fre".into()),
                title: None,
                default: false,
                forced: true,
                text: true,
            }]
        );
    }

    #[tokio::test]
    async fn names_with_colons_are_files_not_protocols() {
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path());
        let colon = dir.path().join("Star Wars: Episode IV.mkv");
        std::fs::rename(&clip, &colon).unwrap();
        assert!(probe(Path::new("ffprobe"), &colon).await.is_ok());
    }

    #[tokio::test]
    async fn broken_files_and_missing_ffprobe_are_told_apart() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.mkv");
        std::fs::write(&junk, b"not a video").unwrap();
        assert!(matches!(
            probe(Path::new("ffprobe"), &junk).await,
            Err(ProbeError::Failed(_))
        ));
        assert!(matches!(
            probe(Path::new("/nonexistent/ffprobe"), &junk).await,
            Err(ProbeError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn unreadable_files_are_retried_not_recorded_as_broken() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let pool = db::open_in_memory().await;
        // "gone.mkv" is in the database but not on disk, as when the share drops.
        library_with(
            &pool,
            dir.path(),
            &[("gone.mkv", b""), ("locked.mkv", b"junk")],
        )
        .await;
        let locked = dir.path().join("locked.mkv");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        assert!(matches!(
            probe(Path::new("ffprobe"), &dir.path().join("gone.mkv")).await,
            Err(ProbeError::Unreadable(_))
        ));
        let report = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            report,
            ProbeReport {
                probed: 0,
                failed: 0,
                retry_later: 2
            }
        );
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM media_probes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 0, "nothing recorded as broken");
    }

    #[test]
    fn streams_with_unknown_codecs_are_kept() {
        let json = br#"{"streams": [{"index": 1, "codec_type": "audio", "channels": 6}]}"#;
        let info = parse(json).unwrap();
        assert_eq!(info.audio[0].codec, "unknown");
        assert_eq!(info.audio[0].channels, Some(6));
    }

    #[test]
    fn detects_hdr_and_skips_cover_art() {
        let json = br#"{"format": {"format_name": "matroska,webm", "duration": "7200.5"},
            "streams": [
              {"index": 0, "codec_type": "video", "codec_name": "hevc", "color_transfer": "smpte2084",
               "avg_frame_rate": "24000/1001"},
              {"index": 1, "codec_type": "video", "codec_name": "hevc",
               "side_data_list": [{"side_data_type": "DOVI configuration record", "dv_profile": 8}]},
              {"index": 2, "codec_type": "video", "codec_name": "hevc", "color_transfer": "arib-std-b67"},
              {"index": 3, "codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}},
              {"index": 4, "codec_type": "subtitle", "codec_name": "hdmv_pgs_subtitle",
               "tags": {"LANGUAGE": "und"}},
              {"index": 5, "codec_type": "attachment"}
            ]}"#;
        let info = parse(json).unwrap();
        let hdr: Vec<Option<&str>> = info.video.iter().map(|v| v.hdr.as_deref()).collect();
        assert_eq!(hdr, [Some("HDR10"), Some("Dolby Vision"), Some("HLG")]);
        assert_eq!(info.video[0].frame_rate, Some(23.976));
        assert_eq!(info.duration_seconds, Some(7200.5));
        assert!(!info.subtitles[0].text);
        assert_eq!(info.subtitles[0].language, None, "und means unknown");
    }

    async fn library_with(pool: &SqlitePool, dir: &Path, files: &[(&str, &[u8])]) {
        sync_config(
            pool,
            &[LibraryConfig {
                name: "movies".into(),
                kind: LibraryKind::Movies,
                path: dir.into(),
            }],
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'M', 'M', 1)")
            .execute(pool)
            .await
            .unwrap();
        for (i, (name, bytes)) in files.iter().enumerate() {
            if !bytes.is_empty() {
                std::fs::write(dir.join(name), bytes).unwrap();
            }
            sqlx::query(
                "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                     first_seen_generation, seen_generation) VALUES (?, 1, ?, 1, 1, 1, 1, 1)",
            )
            .bind(i as i64 + 1)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn probes_pending_files_once_and_again_when_they_change() {
        let dir = tempfile::tempdir().unwrap();
        let clip = std::fs::read(make_clip(dir.path())).unwrap();
        let pool = db::open_in_memory().await;
        library_with(
            &pool,
            dir.path(),
            &[("good.mkv", &clip), ("bad.mkv", b"junk")],
        )
        .await;

        let report = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        assert_eq!(
            report,
            ProbeReport {
                probed: 1,
                failed: 1,
                retry_later: 0
            }
        );
        let again = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        assert_eq!(again, ProbeReport::default(), "nothing changed");

        sqlx::query("UPDATE media_files SET size = 2 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        let changed = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        assert_eq!(changed.probed, 1);
        let duration: f64 = sqlx::query_scalar(
            "SELECT json_extract(info, '$.duration_seconds') FROM media_probes WHERE file_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!((duration - 2.0).abs() < 0.1);
    }

    #[test]
    fn records_the_dolby_vision_profile_and_its_fallback() {
        let stream = |index: i64, record: &str| {
            format!(
                r#"{{"index": {index}, "codec_type": "video", "codec_name": "hevc", "color_transfer": "smpte2084",
                     "side_data_list": [{{"side_data_type": "DOVI configuration record", {record}}}]}}"#
            )
        };
        let json = format!(
            r#"{{"format": {{"format_name": "matroska,webm"}}, "streams": [{}, {}, {}, {}, {}, {},
                {{"index": 6, "codec_type": "video", "codec_name": "hevc", "color_transfer": "smpte2084"}}]}}"#,
            stream(0, r#""dv_profile": 5, "dv_bl_signal_compatibility_id": 0"#),
            stream(1, r#""dv_profile": 8, "dv_bl_signal_compatibility_id": 1"#),
            stream(2, r#""dv_profile": 7, "dv_bl_signal_compatibility_id": 6"#),
            stream(3, r#""dv_profile": 8, "dv_bl_signal_compatibility_id": 4"#),
            stream(4, r#""dv_profile": 8, "dv_bl_signal_compatibility_id": 2"#),
            stream(5, r#""rpu_present_flag": 1"#),
        );
        let info = parse(json.as_bytes()).unwrap();
        let seen: Vec<(Option<i64>, Option<&str>, Picture)> = info
            .video
            .iter()
            .map(|v| {
                let dv = v.dolby_vision.as_ref();
                (
                    dv.and_then(|d| d.profile),
                    dv.and_then(|d| d.fallback.as_deref()),
                    v.picture(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (Some(5), None, Picture::DolbyVisionOnly),
                (Some(8), Some("hdr10"), Picture::Hdr10),
                (Some(7), Some("hdr10"), Picture::Hdr10),
                (Some(8), Some("hlg"), Picture::Hlg),
                (Some(8), Some("sdr"), Picture::Sdr),
                (None, None, Picture::DolbyVisionOnly),
                (None, None, Picture::Hdr10),
            ]
        );
        assert!(info.video[6].dolby_vision.is_none(), "plain HDR10");
        // Probes stored before the profile was recorded read as Dolby Vision only.
        let old: VideoStream = serde_json::from_str(
            r#"{"index": 0, "codec": "hevc", "profile": null, "width": null, "height": null,
                "pixel_format": null, "frame_rate": null, "hdr": "Dolby Vision"}"#,
        )
        .unwrap();
        assert_eq!(old.picture(), Picture::DolbyVisionOnly);
    }

    #[tokio::test]
    async fn dolby_vision_probed_before_its_profile_was_recorded_is_probed_again() {
        let dir = tempfile::tempdir().unwrap();
        let clip = std::fs::read(make_clip(dir.path())).unwrap();
        let pool = db::open_in_memory().await;
        library_with(&pool, dir.path(), &[("a.mkv", &clip), ("b.mkv", &clip)]).await;
        probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        // File 1 as an older Melete stored it: Dolby Vision, no profile.
        let old = r#"{"container": "matroska,webm", "duration_seconds": 2.0, "bit_rate": null,
            "video": [{"index": 0, "codec": "hevc", "profile": null, "width": null, "height": null,
                       "pixel_format": null, "frame_rate": null, "hdr": "Dolby Vision"}],
            "audio": [], "subtitles": []}"#;
        sqlx::query("UPDATE media_probes SET info = ? WHERE file_id = 1")
            .bind(old)
            .execute(&pool)
            .await
            .unwrap();
        let again = probe_pending(&pool, Path::new("ffprobe")).await.unwrap();
        assert_eq!(again.probed, 1, "only the old Dolby Vision probe");
        assert_eq!(
            probe_pending(&pool, Path::new("ffprobe")).await.unwrap(),
            ProbeReport::default()
        );
    }

    #[tokio::test]
    async fn a_missing_ffprobe_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = db::open_in_memory().await;
        library_with(&pool, dir.path(), &[("a.mkv", b"junk")]).await;
        let err = probe_pending(&pool, Path::new("/nonexistent/ffprobe"))
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("MELETE_FFPROBE"));
        let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM media_probes")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 0);
    }
}
