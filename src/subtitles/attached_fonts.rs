//! Fonts attached to a video, pulled out once into the cache for burning subtitles in and for
//! players that draw ASS themselves.

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
    let indexes: Vec<i64> = fonts.iter().map(|f| f.index).collect();
    save_fonts(ffmpeg, cache_dir, key, video, &indexes, &dir).await?;
    Ok(Some(dir))
}

/// A font attached to a video, as players are offered it.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Font {
    /// The attachment's stream index, which names it in its URL and in the fonts' folder.
    pub index: i64,
    /// The name stored with it, without any folders. Only ever shown or handed to a renderer,
    /// never used as a path.
    pub file_name: String,
    /// One of [`FONT_TYPES`], from its name or stored type; never the stored type as it is.
    pub mime_type: String,
    /// Its size in bytes.
    pub size: u64,
}

/// The types fonts are offered as.
pub const FONT_TYPES: &[&str] = &[
    "font/ttf",
    "font/otf",
    "font/collection",
    "font/woff",
    "font/woff2",
    "font/sfnt",
];

/// The most a font offered to players may weigh; bigger ones aren't offered.
pub const FONT_LIMIT: u64 = 64 * 1024 * 1024;

/// The fonts attached to a video that players are offered: those up to [`FONT_LIMIT`]. Listed
/// once into `cache_dir/fonts-{key}.json`; empty for a video without any.
pub async fn font_list(
    ffprobe: &Path,
    cache_dir: &Path,
    key: &str,
    video: &Path,
) -> anyhow::Result<Vec<Font>> {
    let listed = cache_dir.join(format!("fonts-{key}.json"));
    let none = cache_dir.join(format!("fonts-{key}.none"));
    let offered = |fonts: Vec<Font>| fonts.into_iter().filter(|f| f.size <= FONT_LIMIT).collect();
    if let Ok(saved) = tokio::fs::read(&listed).await
        && let Ok(fonts) = serde_json::from_slice::<Vec<Font>>(&saved)
    {
        return Ok(offered(fonts));
    }
    if tokio::fs::try_exists(&none).await.unwrap_or(false) {
        return Ok(Vec::new());
    }
    let fonts = font_attachments(ffprobe, video).await?;
    if fonts.is_empty() {
        save_atomically(&none, b"").await?;
        return Ok(Vec::new());
    }
    save_atomically(&listed, &serde_json::to_vec(&fonts)?).await?;
    Ok(offered(fonts))
}

/// The type a font is offered as, from its name, else its stored type.
fn font_type(name: &str, stored: &str) -> &'static str {
    const BY_NAME: [(&str, &str); 6] = [
        (".ttf", "font/ttf"),
        (".otf", "font/otf"),
        (".ttc", "font/collection"),
        (".otc", "font/collection"),
        (".woff", "font/woff"),
        (".woff2", "font/woff2"),
    ];
    const BY_TYPE: [(&str, &str); 7] = [
        ("woff2", "font/woff2"),
        ("woff", "font/woff"),
        ("collection", "font/collection"),
        ("opentype", "font/otf"),
        ("otf", "font/otf"),
        ("truetype", "font/ttf"),
        ("ttf", "font/ttf"),
    ];
    let named = BY_NAME.iter().find(|(end, _)| name.ends_with(end));
    let typed = || BY_TYPE.iter().find(|(word, _)| stored.contains(word));
    named.or_else(typed).map_or("font/sfnt", |(_, kind)| kind)
}

/// The last part of a stored name, without folders or control characters; at most 255
/// characters.
fn bare_name(stored: &str, index: i64) -> String {
    let last = stored.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = last.chars().filter(|c| !c.is_control()).take(255).collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        format!("font{index}")
    } else {
        name.to_owned()
    }
}

/// The video's attachments that are fonts, by type or name; at most 200.
async fn font_attachments(ffprobe: &Path, video: &Path) -> anyhow::Result<Vec<Font>> {
    let listed = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "t",
            "-show_entries",
            "stream=index,extradata_size:stream_tags=mimetype,filename",
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
    let listing: Listing =
        serde_json::from_slice(&listed.stdout).context("reading ffprobe's attachments")?;
    let fonts: Vec<Font> = listing
        .streams
        .iter()
        .filter_map(Attachment::font)
        .take(200)
        .collect();
    Ok(fonts)
}

/// ffprobe's list of a video's attachments.
#[derive(serde::Deserialize)]
struct Listing {
    #[serde(default)]
    streams: Vec<Attachment>,
}

#[derive(serde::Deserialize)]
struct Attachment {
    index: i64,
    #[serde(default)]
    extradata_size: u64,
    #[serde(default)]
    tags: HashMap<String, String>,
}

impl Attachment {
    /// The attachment as a font, when it is one by its type or name.
    fn font(&self) -> Option<Font> {
        let tag = |name: &str| self.tags.get(name).map(String::as_str).unwrap_or_default();
        let stored_type = tag("mimetype").to_ascii_lowercase();
        let stored_name = tag("filename").to_ascii_lowercase();
        let is_font = stored_type.contains("font")
            || [".ttf", ".otf", ".ttc", ".woff", ".woff2"]
                .iter()
                .any(|e| stored_name.ends_with(e));
        if !is_font {
            return None;
        }
        let file_name = bare_name(tag("filename"), self.index);
        let mime_type = font_type(&file_name.to_ascii_lowercase(), &stored_type);
        Some(Font {
            index: self.index,
            file_name,
            mime_type: mime_type.to_owned(),
            size: self.extradata_size,
        })
    }
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
