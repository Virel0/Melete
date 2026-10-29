//! Fonts attached to a video, pulled out once into the cache for burning subtitles in.

use super::*;

/// A path as an ffmpeg input that can't be read as a protocol or option.
pub(super) fn file_input(path: &Path) -> std::ffi::OsString {
    let mut input = std::ffi::OsString::from("file:");
    input.push(path);
    input
}

/// The fonts attached to a video (anime releases carry the fonts their styled subtitles use),
/// pulled out once into `cache_dir/fonts-{key}` for burning subtitles in. `None` when the
/// video has none. Each is written to a name Melete picks, never the name stored in the file,
/// which could point anywhere.
pub async fn fonts(
    ffprobe: &Path,
    ffmpeg: &Path,
    cache_dir: &Path,
    key: &str,
    video: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let dir = cache_dir.join(format!("fonts-{key}"));
    let none = cache_dir.join(format!("fonts-{key}.none"));
    if tokio::fs::try_exists(&none).await.unwrap_or(false) {
        return Ok(None);
    }
    if tokio::fs::try_exists(&dir).await.unwrap_or(false) {
        return Ok(Some(dir));
    }
    tokio::fs::create_dir_all(cache_dir).await?;
    let fonts = font_attachments(ffprobe, video).await?;
    if fonts.is_empty() {
        save_atomically(&none, b"").await?;
        return Ok(None);
    }
    save_fonts(ffmpeg, cache_dir, key, video, &fonts, &dir).await?;
    Ok(Some(dir))
}

/// The indexes of the video's attachments that are fonts, by type or name; at most 200.
async fn font_attachments(ffprobe: &Path, video: &Path) -> anyhow::Result<Vec<i64>> {
    let listed = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "t",
            "-show_entries",
            "stream=index:stream_tags=mimetype,filename",
            "-of",
            "json",
        ])
        .arg(file_input(video))
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let listed = tokio::time::timeout(Duration::from_secs(30), listed)
        .await
        .context("listing attachments timed out")??;
    if !listed.status.success() {
        bail!(
            "ffprobe couldn't list attachments: {}",
            String::from_utf8_lossy(&listed.stderr).trim()
        );
    }
    #[derive(serde::Deserialize)]
    struct Listing {
        #[serde(default)]
        streams: Vec<Attachment>,
    }
    #[derive(serde::Deserialize)]
    struct Attachment {
        index: i64,
        #[serde(default)]
        tags: HashMap<String, String>,
    }
    let listing: Listing =
        serde_json::from_slice(&listed.stdout).context("reading ffprobe's attachments")?;
    let is_font = |a: &Attachment| {
        let mime = a
            .tags
            .get("mimetype")
            .map(|m| m.to_ascii_lowercase())
            .unwrap_or_default();
        let name = a
            .tags
            .get("filename")
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();
        mime.contains("font")
            || [".ttf", ".otf", ".ttc", ".woff", ".woff2"]
                .iter()
                .any(|e| name.ends_with(e))
    };
    let fonts: Vec<i64> = listing
        .streams
        .iter()
        .filter(|a| is_font(a))
        .map(|a| a.index)
        .take(200)
        .collect();
    Ok(fonts)
}

/// Pulls the fonts out of the video into `dir`, each named by its stream's index.
async fn save_fonts(
    ffmpeg: &Path,
    cache_dir: &Path,
    key: &str,
    video: &Path,
    fonts: &[i64],
    dir: &Path,
) -> anyhow::Result<()> {
    // Into a folder of its own first, then renamed: readers never see half of it.
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let partial = cache_dir.join(format!(".fonts-{key}.{}.part", u64::from_le_bytes(suffix)));
    tokio::fs::create_dir_all(&partial).await?;
    let mut command = Command::new(ffmpeg);
    command.args(["-nostdin", "-y", "-v", "error"]);
    for index in fonts {
        command
            .arg(format!("-dump_attachment:{index}"))
            .arg(partial.join(format!("font{index}")));
    }
    command
        .arg("-i")
        .arg(file_input(video))
        .args(["-t", "0", "-f", "null", "-"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let dumped = tokio::time::timeout(Duration::from_secs(60), command.output()).await;
    let dumped = match dumped {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(anyhow::anyhow!(
            "ffmpeg couldn't save the fonts: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Ok(Err(err)) => Err(err.into()),
        Err(_) => Err(anyhow::anyhow!("saving the fonts timed out")),
    };
    if let Err(err) = dumped {
        let _ = tokio::fs::remove_dir_all(&partial).await;
        return Err(err);
    }
    if let Err(err) = tokio::fs::rename(&partial, dir).await {
        let _ = tokio::fs::remove_dir_all(&partial).await;
        // Another transcode saved them first.
        if !tokio::fs::try_exists(dir).await.unwrap_or(false) {
            return Err(err.into());
        }
    }
    Ok(())
}
