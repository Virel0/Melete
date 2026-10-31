//! Copied video's keyframes, found through the file's index, and the HLS playlists of fixed
//! segments or of keyframes.

use super::*;

/// Copied video can only be cut at keyframes, and `-ss` lands only on one, so segments start
/// at keyframes about [`SEGMENT_SECONDS`] apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyframes {
    /// Seconds from the start of the file; the first is 0.
    pub starts: Vec<f64>,
    /// How far past a keyframe `-ss` must point to land on it; see [`keyframes`].
    pub seek_past: f64,
    /// For each segment, its keyframe's decode time in the file's own timestamps: a run from
    /// the start puts copied audio from before it in the segment before. Matroska keeps no
    /// decode times, so they're worked out as ffmpeg does, from the frame rate and the frames
    /// held back for B-frames. The first segment's is never needed.
    pub audio_from: Vec<f64>,
}

/// Longer than this between two keyframes, and copying is refused: a segment would be huge,
/// or the file has no index and every seek came back to the start.
pub(super) const MAX_COPY_SEGMENT: f64 = 30.0;
pub(super) const KEYFRAME_PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// What [`keyframes`] asks ffprobe of the file first.
#[derive(Deserialize)]
struct Probe {
    format: Format,
    #[serde(default)]
    streams: Vec<Stream>,
}

#[derive(Deserialize)]
struct Format {
    start_time: Option<String>,
}

#[derive(Deserialize)]
struct Stream {
    index: i64,
    has_b_frames: Option<i64>,
    avg_frame_rate: Option<String>,
}

impl Stream {
    /// How far a keyframe's decode time comes before its display time, when the file doesn't
    /// say: the frames held back for B-frames.
    fn held_back(&self) -> Option<f64> {
        let (num, den) = self.avg_frame_rate.as_deref()?.split_once('/')?;
        let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
        (num > 0.0 && den > 0.0).then(|| self.has_b_frames.unwrap_or(0) as f64 * den / num)
    }
}

/// Finds keyframes about every [`SEGMENT_SECONDS`] without reading the whole file: ffprobe
/// seeks to each point through the file's index (Matroska cues, MP4 sync samples) and reports
/// the keyframe it landed on.
pub async fn keyframes(
    ffprobe: &Path,
    input: &Path,
    video: i64,
    duration: f64,
) -> anyhow::Result<Keyframes> {
    let mut file = OsString::from("file:");
    file.push(input);
    let run = |args: Vec<OsString>| async move {
        let output = Command::new(ffprobe)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(KEYFRAME_PROBE_TIMEOUT, output)
            .await
            .map_err(|_| anyhow::anyhow!("finding keyframes took too long"))??;
        anyhow::ensure!(
            output.status.success(),
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        anyhow::Ok(String::from_utf8(output.stdout)?)
    };
    let mut args: Vec<OsString> = ["-v", "error", "-show_entries"].map(OsString::from).into();
    args.push("format=start_time:stream=index,has_b_frames,avg_frame_rate".into());
    args.extend(["-of", "json"].map(OsString::from));
    args.push(file.clone());
    let probe: Probe = serde_json::from_str(&run(args).await?)?;
    let start_time: f64 = probe
        .format
        .start_time
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.0);
    // ffmpeg seeks this much (3/23 s) before -ss when any stream has B-frames
    // (fftools/ffmpeg_demux.c, for containers that seek by decoding time, as Matroska and MP4
    // do), and lands on the keyframe at or before that point.
    let b_frames = probe
        .streams
        .iter()
        .any(|s| s.has_b_frames.unwrap_or(0) > 0);
    let seek_past = if b_frames { 0.130434 } else { 0.0 } + 0.0005;
    let held_back = probe
        .streams
        .iter()
        .find(|s| s.index == video)
        .and_then(Stream::held_back)
        .unwrap_or(0.0);

    let intervals: Vec<String> = (0..segment_count(duration))
        .map(|n| format!("{:.6}%+#1", start_time + n as f64 * SEGMENT_SECONDS))
        .collect();
    let mut args: Vec<OsString> = ["-v", "error", "-select_streams"]
        .map(OsString::from)
        .into();
    args.push(video.to_string().into());
    args.push("-read_intervals".into());
    args.push(intervals.join(",").into());
    args.extend(
        [
            "-show_entries",
            "packet=pts_time,dts_time,flags",
            "-of",
            "csv=p=0",
        ]
        .map(OsString::from),
    );
    args.push(file);
    let mut starts = vec![0.0];
    let mut audio_from = vec![f64::NEG_INFINITY];
    for line in run(args).await?.lines() {
        let mut fields = line.split(',');
        let (Some(pts), Some(dts), Some(flags)) = (fields.next(), fields.next(), fields.next())
        else {
            anyhow::bail!("unexpected ffprobe output {line:?}");
        };
        anyhow::ensure!(
            flags.contains('K'),
            "a seek landed on a frame that isn't a keyframe"
        );
        let pts: f64 = pts.parse()?;
        let at = pts - start_time;
        // A keyframe right at the start belongs to the first segment, as does a sliver of one.
        if at > 0.5 && at < duration && at > starts.last().copied().unwrap_or(0.0) + 0.001 {
            starts.push(at);
            audio_from.push(dts.parse().unwrap_or(pts - held_back));
        }
    }
    let longest = starts
        .iter()
        .zip(starts.iter().skip(1).chain([&duration]))
        .map(|(a, b)| b - a)
        .fold(0.0, f64::max);
    anyhow::ensure!(
        longest <= MAX_COPY_SEGMENT,
        "{longest:.1} s without a keyframe found"
    );
    Ok(Keyframes {
        starts,
        seek_past,
        audio_from,
    })
}

/// How many fixed segments a video `duration` seconds long has; at least one.
pub fn segment_count(duration: f64) -> i64 {
    ((duration / SEGMENT_SECONDS).ceil() as i64).max(1)
}

/// The whole file as a VOD playlist of fixed segments named `{n}.ts`.
#[cfg(test)]
pub fn playlist(duration: f64) -> String {
    let starts: Vec<f64> = (0..segment_count(duration))
        .map(|n| n as f64 * SEGMENT_SECONDS)
        .collect();
    playlist_of(&starts, duration, "ts", false)
}

pub(super) fn playlist_of(starts: &[f64], duration: f64, extension: &str, fmp4: bool) -> String {
    let lengths: Vec<f64> = starts
        .iter()
        .zip(starts.iter().skip(1).chain([&duration]))
        .map(|(a, b)| b - a)
        .collect();
    let target = lengths
        .iter()
        .copied()
        .fold(SEGMENT_SECONDS, f64::max)
        .ceil() as i64;
    let mut out = format!(
        "#EXTM3U\n#EXT-X-VERSION:{}\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n",
        if fmp4 { 7 } else { 3 }
    );
    if fmp4 {
        out.push_str("#EXT-X-MAP:URI=\"init.mp4\"\n");
    }
    for (n, length) in lengths.iter().enumerate() {
        out.push_str(&format!("#EXTINF:{length:.6},\n{n}.{extension}\n"));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}
