//! Converting a track once with ffmpeg, one conversion per track, and caching the result or the
//! failure.

use std::sync::PoisonError;

use super::*;

/// The track as WebVTT, converted by ffmpeg once and then served from `cache_dir/{key}.vtt`.
/// `key` must already be a safe file name.
pub async fn webvtt(
    ffmpeg: &Path,
    cache_dir: &Path,
    key: &str,
    source: Source<'_>,
) -> anyhow::Result<Vec<u8>> {
    let path = convert(ffmpeg, cache_dir, key, source, Format::WebVtt).await?;
    Ok(tokio::fs::read(path).await?)
}

/// Converts the track once into `cache_dir/{key}.{vtt|ass}` and returns that file's path.
/// `key` must already be a safe file name.
pub async fn convert(
    ffmpeg: &Path,
    cache_dir: &Path,
    key: &str,
    source: Source<'_>,
    format: Format,
) -> anyhow::Result<PathBuf> {
    let cached = cache_dir.join(format!("{key}.{}", format.extension()));
    let failed = cache_dir.join(format!("{key}.{}.failed", format.extension()));
    if let Some(done) = finished(&cached, &failed).await? {
        return Ok(done);
    }
    // Requests for the same track wait here for the one conversion, then find its result.
    let lock = IN_FLIGHT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(cached.clone())
        .or_default()
        .clone();
    let result = {
        let _same_track = lock.lock().await;
        match finished(&cached, &failed).await? {
            Some(done) => Ok(done),
            None => run(ffmpeg, &cached, &failed, source, format).await,
        }
    };
    {
        let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(PoisonError::into_inner);
        // Only the map and this request still hold the lock: nobody else is waiting.
        if Arc::strong_count(&lock) <= 2 {
            in_flight.remove(&cached);
        }
    }
    result
}

/// The cached result, or the error of a recent failure; `None` when it needs converting.
pub(super) async fn finished(cached: &Path, failed: &Path) -> anyhow::Result<Option<PathBuf>> {
    if tokio::fs::try_exists(cached).await.unwrap_or(false) {
        return Ok(Some(cached.to_path_buf()));
    }
    if let Ok(metadata) = tokio::fs::metadata(failed).await {
        let age = metadata
            .modified()
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .unwrap_or_default();
        if age < RETRY_FAILED_AFTER {
            let reason = tokio::fs::read_to_string(failed).await.unwrap_or_default();
            bail!("converting this track failed recently: {reason}");
        }
    }
    Ok(None)
}

/// Runs ffmpeg and caches the result, or remembers the failure for a while.
pub(super) async fn run(
    ffmpeg: &Path,
    cached: &Path,
    failed: &Path,
    source: Source<'_>,
    format: Format,
) -> anyhow::Result<PathBuf> {
    let _slot = tokio::time::timeout(SLOT_WAIT, CONVERSIONS.acquire())
        .await
        .map_err(|_| anyhow::Error::new(Busy))??;
    let (input, stream) = match source {
        Source::Embedded { video, index } => (video, format!("0:{index}")),
        Source::Sidecar(path) => (path, "0:s:0".to_owned()),
    };
    // A file beside the video in another encoding is read through a UTF-8 copy, removed once
    // ffmpeg is done with it.
    let utf8_copy = match source {
        Source::Sidecar(path) => utf8_copy(path, cached).await?,
        Source::Embedded { .. } => None,
    };
    let input = utf8_copy.as_ref().map_or(input, |copy| copy.path.as_path());
    let (codec, muxer, header): (&str, &str, &[u8]) = match format {
        Format::WebVtt => ("webvtt", "webvtt", b"WEBVTT"),
        Format::Ass => ("ass", "ass", b"[Script Info]"),
    };
    // "file:" keeps a name with a colon from being read as a protocol.
    let mut input_arg = std::ffi::OsString::from("file:");
    input_arg.push(input);
    let mut command = Command::new(ffmpeg);
    command
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(input_arg)
        .args(["-map", &stream, "-c:s", codec, "-f", muxer, "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(CONVERT_TIMEOUT, command.output()).await;
    drop(utf8_copy);
    let output = output
        .context("converting the subtitles took too long")?
        .with_context(|| format!("starting {}", ffmpeg.display()))?;
    let body = output
        .stdout
        .strip_prefix(b"\xef\xbb\xbf".as_slice())
        .unwrap_or(&output.stdout);
    if !output.status.success() || !body.starts_with(header) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason: String = stderr.trim().chars().take(500).collect();
        // Remembered, so requests don't each re-read the whole file to fail again.
        let _ = save_atomically(failed, reason.as_bytes()).await;
        bail!("ffmpeg couldn't convert the subtitles: {reason}");
    }
    save_atomically(cached, &output.stdout).await?;
    Ok(cached.to_path_buf())
}

/// A UTF-8 copy of a subtitle file in another encoding, in the cache; removed when dropped.
struct Utf8Copy {
    path: PathBuf,
}

impl Drop for Utf8Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// For a subtitle file that isn't UTF-8, a UTF-8 copy of it beside `cached`, named as the
/// cache's temporary files are so a copy left behind is cleaned up; None for a UTF-8 file.
async fn utf8_copy(path: &Path, cached: &Path) -> anyhow::Result<Option<Utf8Copy>> {
    let file = path.to_path_buf();
    let bytes = tokio::task::spawn_blocking(move || read_limited(&file)).await??;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let (encoding, Some(text)) = to_utf8(&bytes, name) else {
        return Ok(None);
    };
    tracing::debug!(
        encoding = encoding.name(),
        "reading a subtitle file that isn't UTF-8"
    );
    let dir = cached.parent().context("cache paths have a parent")?;
    tokio::fs::create_dir_all(dir).await?;
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let copy = Utf8Copy {
        path: dir.join(format!(".{}.part", u64::from_le_bytes(suffix))),
    };
    tokio::fs::write(&copy.path, text).await?;
    Ok(Some(copy))
}

pub(super) async fn save_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().context("cache paths have a parent")?;
    tokio::fs::create_dir_all(dir).await?;
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let temp = dir.join(format!(".{}.part", u64::from_le_bytes(suffix)));
    let written = match tokio::fs::write(&temp, bytes).await {
        Ok(()) => tokio::fs::rename(&temp, path).await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err.into());
    }
    Ok(())
}
