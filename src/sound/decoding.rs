//! Decoding a file once with ffmpeg and taking every measurement from that one pass.

use super::*;

/// The ffmpeg and ffprobe programs measuring runs.
pub struct Tools {
    /// The ffmpeg to run.
    pub ffmpeg: PathBuf,
    /// The ffprobe to run.
    pub ffprobe: PathBuf,
}

/// "file:" keeps a name with a colon from being read as a protocol.
pub(super) fn input(file: &Path) -> OsString {
    let mut input = OsString::from("file:");
    input.push(file);
    input
}

/// The end of a log, for an error message.
pub(super) fn tail(log: &str) -> String {
    let log = log.trim();
    let start = log.len().saturating_sub(500);
    let start = (start..=log.len())
        .find(|&i| log.is_char_boundary(i))
        .unwrap_or(log.len());
    log[start..].to_owned()
}

#[derive(Deserialize)]
pub(super) struct Probed {
    #[serde(default)]
    pub(super) streams: Vec<ProbedStream>,
}

#[derive(Deserialize)]
pub(super) struct ProbedStream {
    pub(super) channels: Option<usize>,
    pub(super) sample_rate: Option<String>,
}

/// The first audio stream's channels and sample rate, which the raw PCM is read by. Taken
/// from ffprobe rather than the tags, so they're what ffmpeg will actually decode.
pub(super) async fn layout(ffprobe: &Path, file: &Path) -> Result<(usize, u32), MeasureError> {
    let output = Command::new(ffprobe)
        .args(["-v", "error", "-select_streams", "a:0"])
        .args([
            "-show_entries",
            "stream=channels,sample_rate",
            "-of",
            "json",
        ])
        .arg(input(file))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| format!("starting {}", ffprobe.display()))
        .map_err(MeasureError::Unavailable)?;
    if !output.status.success() {
        return Err(MeasureError::Failed(tail(&String::from_utf8_lossy(
            &output.stderr,
        ))));
    }
    let probed: Probed = serde_json::from_slice(&output.stdout)
        .map_err(|err| MeasureError::Failed(format!("ffprobe's answer: {err}")))?;
    let stream = probed
        .streams
        .first()
        .ok_or_else(|| MeasureError::Failed("no audio stream".into()))?;
    let channels = stream.channels.unwrap_or(0);
    let rate: u32 = stream
        .sample_rate
        .as_deref()
        .and_then(|r| r.parse().ok())
        .unwrap_or(0);
    if channels < 1 || rate < 1 {
        return Err(MeasureError::Failed(format!(
            "ffprobe reported {channels} channels at {rate} Hz"
        )));
    }
    Ok((channels, rate))
}

/// Decodes `file` once and takes every measurement from it.
pub async fn measure(tools: &Tools, file: &Path) -> Result<Measurement, MeasureError> {
    // Only a file we can open is worth asking about: a dropped share would otherwise be
    // recorded as a broken file and never tried again.
    if let Err(err) = tokio::fs::File::open(file).await {
        return Err(MeasureError::Unreadable(err.to_string()));
    }
    let result = match tokio::time::timeout(TIMEOUT, decode(tools, file)).await {
        Ok(result) => result,
        Err(_) => Err(MeasureError::Unreadable(format!(
            "took over {} minutes",
            TIMEOUT.as_secs() / 60
        ))),
    };
    // A share that dropped mid-decode looks like a broken file; only one still readable is.
    if let Err(MeasureError::Failed(_)) = &result
        && let Err(err) = tokio::fs::File::open(file).await
    {
        return Err(MeasureError::Unreadable(err.to_string()));
    }
    result
}

/// Only how loud `file` is: one ffmpeg run through `loudnorm`, for ReplayGain at import. None
/// for a track too quiet to measure; the caller leaves out tracks too short to.
pub async fn loudness_of(tools: &Tools, file: &Path) -> Result<Option<Loudness>, MeasureError> {
    let run = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-nostats", "-v", "info", "-nostdin", "-i"])
        .arg(input(file))
        .args(["-vn", "-map", "0:a:0", "-af", "loudnorm=print_format=json"])
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(TIMEOUT, run).await {
        Ok(output) => output
            .with_context(|| format!("starting {}", tools.ffmpeg.display()))
            .map_err(MeasureError::Unavailable)?,
        Err(_) => {
            return Err(MeasureError::Unreadable(format!(
                "took over {} minutes",
                TIMEOUT.as_secs() / 60
            )));
        }
    };
    let log = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(MeasureError::Failed(tail(&log)));
    }
    Ok(loudness::parse(&log))
}

pub(super) async fn decode(tools: &Tools, file: &Path) -> Result<Measurement, MeasureError> {
    let (channels, rate) = layout(&tools.ffprobe, file).await?;
    // One run, two outputs: the decoded audio is split, one branch to the pipe as raw float
    // PCM at the file's own layout (no resampling, which would move a threshold crossing),
    // the other through loudnorm to a null muxer purely so the filter prints its measurement.
    // -v info because loudnorm prints at that level; -nostats keeps the progress line out of
    // the middle of it.
    let mut child = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-nostats", "-v", "info", "-nostdin", "-i"])
        .arg(input(file))
        .args(["-vn", "-filter_complex"])
        .arg("[0:a:0]asplit=2[raw][loud];[loud]loudnorm=print_format=json[measured]")
        .args(["-map", "[raw]", "-f", "f32le", "pipe:1"])
        .args(["-map", "[measured]", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {}", tools.ffmpeg.display()))
        .map_err(MeasureError::Unavailable)?;
    let stdout = child
        .stdout
        .take()
        .expect("stdout is set to piped above, and taken only here");
    let mut stderr = child
        .stderr
        .take()
        .expect("stderr is set to piped above, and taken only here");
    let log = tokio::spawn(async move {
        let mut log = Vec::new();
        stderr.read_to_end(&mut log).await.map(|_| log)
    });
    let scans = Scans {
        bounds: BoundsScan::new(channels, rate),
        tempo: TempoScan::new(channels, rate),
        key: KeyScan::new(channels, rate),
        voice: VocalScan::new(channels, rate),
    };
    let (frames, scans) = pump(stdout, channels, scans)
        .await
        .map_err(|err| MeasureError::Unreadable(format!("{err:#}")))?;
    let status = child
        .wait()
        .await
        .map_err(|err| MeasureError::Unreadable(err.to_string()))?;
    let log = log
        .await
        .ok()
        .and_then(Result::ok)
        .map(|log| String::from_utf8_lossy(&log).into_owned())
        .unwrap_or_default();
    if !status.success() {
        // Killed (out of memory, say) or silent: no verdict on the file.
        if status.code().is_none() || log.trim().is_empty() {
            return Err(MeasureError::Unreadable(format!(
                "ffmpeg ended without a verdict ({status})"
            )));
        }
        return Err(MeasureError::Failed(tail(&log)));
    }
    // loudnorm still prints an answer for a two-second interlude, but its gating hasn't seen
    // enough for it to mean anything.
    let loudness = if frames >= loudness::MINIMUM_SECONDS * i64::from(rate) {
        loudness::parse(&log)
    } else {
        None
    };
    let tempo = scans.tempo.result();
    let duration_ms = frames as f64 * 1000.0 / f64::from(rate);
    let t = &scans.tempo;
    let found = beat_grid::analyze_with_beats(
        &t.onsets,
        &t.low_onsets,
        t.rate,
        t.first_onset_ms(),
        duration_ms,
        tempo.as_ref(),
    );
    let arrangement = arrangement::analyze(arrangement::Inputs {
        timbre: &t.timbre,
        timbre_rate: t.timbre_rate(),
        onsets: &t.onsets,
        onset_rate: t.rate,
        first_onset_ms: t.first_onset_ms(),
        beats: found.as_ref().map(|(_, beats)| beats),
        grid: found.as_ref().map(|(grid, _)| grid),
        duration_ms,
        vocals: scans.voice.spans(),
    });
    Ok(Measurement {
        bounds: scans.bounds.result(),
        loudness,
        tempo,
        grid: found.map(|(grid, _)| grid),
        key: scans.key.result(),
        arrangement,
    })
}
