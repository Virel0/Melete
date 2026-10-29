//! Converting music for devices that can't play a file as it is: ALAC or
//! FLAC (lossless stays lossless), AAC, Opus or MP3. A conversion is made whole, then kept in
//! the transcode folder and served with byte ranges like any file, so players can seek; a
//! track takes a second or two. Ones unused for two days are deleted. The library is only read.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Semaphore;

/// Conversions running at once; each is one CPU-bound ffmpeg.
const SLOTS: usize = 2;
/// The longest a conversion may take.
const TIMEOUT: Duration = Duration::from_secs(300);
/// How long an unused conversion is kept: longer than a stream link lasts (a day).
const KEEP_FOR: Duration = Duration::from_secs(2 * 24 * 3600);
/// How often old conversions are looked for.
const CLEAN_EVERY: Duration = Duration::from_secs(3600);

/// A format a track can be converted to for a player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Alac,
    Flac,
    Aac,
    Opus,
    Mp3,
}

impl Format {
    /// The format named `alac`, `flac`, `aac`, `opus` or `mp3`; none for anything else.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "alac" => Self::Alac,
            "flac" => Self::Flac,
            "aac" => Self::Aac,
            "opus" => Self::Opus,
            "mp3" => Self::Mp3,
            _ => return None,
        })
    }

    /// Its name, as [`Format::parse`] takes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alac => "alac",
            Self::Flac => "flac",
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
        }
    }

    /// Whether it keeps every bit (ALAC, FLAC).
    pub fn lossless(self) -> bool {
        matches!(self, Self::Alac | Self::Flac)
    }

    /// The bitrate used when none is asked for (lossy formats only).
    pub fn default_kbps(self) -> Option<i64> {
        match self {
            Self::Aac => Some(256),
            Self::Opus => Some(160),
            Self::Mp3 => Some(320),
            Self::Alac | Self::Flac => None,
        }
    }

    /// The file extension a conversion to it gets, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Alac | Self::Aac => "m4a",
            Self::Flac => "flac",
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
        }
    }

    /// ffmpeg's encoder and container arguments; `stereo` folds every channel into two.
    fn args(self, kbps: Option<i64>, stereo: bool) -> Vec<String> {
        let (codec, container) = match self {
            Self::Alac => ("alac", "ipod"),
            Self::Flac => ("flac", "flac"),
            Self::Aac => ("aac", "ipod"),
            Self::Opus => ("libopus", "ogg"),
            Self::Mp3 => ("libmp3lame", "mp3"),
        };
        let mut args = vec!["-c:a".to_owned(), codec.to_owned()];
        if stereo {
            args.extend(["-ac".to_owned(), "2".to_owned()]);
        }
        if let Some(kbps) = kbps {
            args.extend(["-b:a".to_owned(), format!("{kbps}k")]);
        }
        if container == "ipod" {
            // The index first, so a player can start before the whole file has arrived.
            args.extend(["-movflags".to_owned(), "+faststart".to_owned()]);
        }
        args.extend(["-f".to_owned(), container.to_owned()]);
        args
    }
}

/// Converts tracks with ffmpeg into a folder of kept conversions, a few at a time.
pub struct Converter {
    ffmpeg: PathBuf,
    dir: PathBuf,
    slots: Semaphore,
    /// When old conversions were last looked for, in Unix seconds.
    cleaned_at: AtomicU64,
}

impl Converter {
    /// Conversions go in `music/` inside the transcode folder.
    pub fn new(ffmpeg: PathBuf, transcode_dir: &Path) -> Self {
        Self {
            ffmpeg,
            dir: transcode_dir.join("music"),
            slots: Semaphore::new(SLOTS),
            cleaned_at: AtomicU64::new(0),
        }
    }

    /// Where the conversion called `name` is (or will be) kept.
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// The converted file's name, made now unless it's kept from before, keeping the source's
    /// channels. `key` names the source's version (a new version of a file gets a new
    /// conversion).
    #[cfg(test)]
    pub async fn convert(
        &self,
        source: &Path,
        key: &str,
        format: Format,
        kbps: Option<i64>,
    ) -> anyhow::Result<String> {
        self.convert_channels(source, key, format, kbps, false)
            .await
    }

    /// The converted file's name, made now unless it's kept from before. `key` names the
    /// source's version; `stereo` folds a multichannel or Dolby Atmos track into two channels,
    /// for devices that play neither.
    pub async fn convert_channels(
        &self,
        source: &Path,
        key: &str,
        format: Format,
        kbps: Option<i64>,
        stereo: bool,
    ) -> anyhow::Result<String> {
        self.clean_now_and_then().await;
        let name = format!(
            "{key}-{}{}{}.{}",
            format.as_str(),
            kbps.map(|k| format!("-{k}")).unwrap_or_default(),
            if stereo { "-stereo" } else { "" },
            format.extension()
        );
        let path = self.dir.join(&name);
        if tokio::fs::metadata(&path).await.is_ok() {
            // Used again: kept another two days.
            let _ = std::fs::File::options()
                .append(true)
                .open(&path)
                .and_then(|f| f.set_modified(SystemTime::now()));
            return Ok(name);
        }
        let _slot = self.slots.acquire().await?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let partial = self.dir.join(format!("{name}.partial"));
        let mut command = tokio::process::Command::new(&self.ffmpeg);
        command
            .args(["-nostdin", "-loglevel", "error", "-y", "-i"])
            .arg(source)
            // The first audio stream and the tags; no cover picture, no video.
            .args(["-map", "0:a:0", "-map_metadata", "0", "-vn"])
            .args(format.args(kbps, stereo))
            .arg(&partial)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(TIMEOUT, command.output()).await;
        let ok = matches!(&output, Ok(Ok(o)) if o.status.success());
        if !ok {
            let _ = tokio::fs::remove_file(&partial).await;
            let reason = match output {
                Err(_) => "it took too long".to_owned(),
                Ok(Err(err)) => format!("ffmpeg didn't start: {err}"),
                Ok(Ok(o)) => String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .last()
                    .unwrap_or("ffmpeg failed")
                    .to_owned(),
            };
            anyhow::bail!("converting to {} failed: {reason}", format.as_str());
        }
        tokio::fs::rename(&partial, &path).await?;
        Ok(name)
    }

    /// Deletes conversions unused for two days, at most once an hour.
    async fn clean_now_and_then(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let last = self.cleaned_at.load(Ordering::Relaxed);
        if now.saturating_sub(last) < CLEAN_EVERY.as_secs()
            || self
                .cleaned_at
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        let removed = self.clean(KEEP_FOR).await;
        if removed > 0 {
            tracing::info!(removed, "deleted unused music conversions");
        }
    }

    /// Deletes conversions (and leftover partial ones) not used for `age`; returns how many.
    pub async fn clean(&self, age: Duration) -> usize {
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return 0;
        };
        let mut removed = 0;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let old = entry
                .metadata()
                .await
                .and_then(|m| m.modified())
                .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() >= age);
            if old && tokio::fs::remove_file(entry.path()).await.is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(dir: &Path) -> PathBuf {
        let path = dir.join("tone.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=d=2"])
            .args(["-metadata", "title=Tone"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        path
    }

    #[tokio::test]
    async fn converts_to_each_format_once_and_keeps_tags() {
        let dir = tempfile::tempdir().unwrap();
        let source = tone(dir.path());
        let converter = Converter::new("ffmpeg".into(), &dir.path().join("transcode"));
        for (format, codec) in [
            (Format::Alac, "alac"),
            (Format::Flac, "flac"),
            (Format::Aac, "aac"),
            (Format::Opus, "opus"),
            (Format::Mp3, "mp3"),
        ] {
            let name = converter
                .convert(&source, "t1", format, format.default_kbps())
                .await
                .unwrap();
            let tags = crate::library::tags::read(&converter.path(&name)).unwrap();
            assert_eq!(tags.codec, Some(codec), "{name}");
            assert_eq!(tags.title.as_deref(), Some("Tone"), "{name}");
            assert!(tags.duration_seconds.unwrap() > 1.9, "{name}");
        }
        let again = converter
            .convert(&source, "t1", Format::Aac, Some(256))
            .await
            .unwrap();
        assert_eq!(again, "t1-aac-256.m4a");
        let made = std::fs::read_dir(dir.path().join("transcode/music"))
            .unwrap()
            .count();
        assert_eq!(made, 5, "nothing converted twice, nothing partial left");
    }

    #[tokio::test]
    async fn a_failed_conversion_leaves_nothing_and_old_ones_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let converter = Converter::new("ffmpeg".into(), &dir.path().join("transcode"));
        let missing = dir.path().join("missing.flac");
        assert!(
            converter
                .convert(&missing, "x", Format::Mp3, None)
                .await
                .is_err()
        );
        let source = tone(dir.path());
        converter
            .convert(&source, "t1", Format::Flac, None)
            .await
            .unwrap();
        assert_eq!(converter.clean(Duration::from_secs(3600)).await, 0);
        assert_eq!(converter.clean(Duration::ZERO).await, 1);
        assert_eq!(
            std::fs::read_dir(dir.path().join("transcode/music"))
                .unwrap()
                .count(),
            0
        );
    }
}
