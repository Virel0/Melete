//! Subtitle files beside a video, found by name with their language and flags.

use super::*;

/// A subtitle file beside a video: `Movie.srt`, `Movie.en.srt`, `Movie.fr.forced.ass`…
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sidecar {
    /// From the file's name, so it stays the same when other subtitle files come and go.
    pub id: String,
    /// The file's name, in the video's folder.
    pub file: String,
    /// The format, as ffprobe would name it: "subrip" for .srt, else the extension in lower case.
    pub codec: String,
    /// The language tag in the name, such as "en" or "fre"; None without one.
    pub language: Option<String>,
    /// Whether the name says forced: only the parts in another language.
    pub forced: bool,
    /// Whether the name says it's for the hearing impaired: sdh, cc, or hi beside a language.
    pub hearing_impaired: bool,
    /// Its full path, never sent to clients.
    #[serde(skip)]
    pub path: PathBuf,
    /// Its size in bytes, never sent to clients.
    #[serde(skip)]
    pub size: u64,
    /// When it was last changed, in Unix seconds (0 when unknown), never sent to clients.
    #[serde(skip)]
    pub modified: u64,
}

/// The subtitle files beside a video. Symlinks are ignored, so only files really in the
/// video's folder are offered.
pub fn sidecars(video: &Path) -> std::io::Result<Vec<Sidecar>> {
    let (Some(dir), Some(stem)) = (video.parent(), video.file_stem().and_then(|s| s.to_str()))
    else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some((base, extension)) = name.rsplit_once('.') else {
            continue;
        };
        let extension = extension.to_ascii_lowercase();
        if !SIDECAR_EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        let tags = match base.strip_prefix(stem) {
            Some("") => "",
            Some(rest) => match rest.strip_prefix('.') {
                Some(tags) => tags,
                None => continue, // "Movie 2.srt" belongs to another video
            },
            None => continue,
        };
        let tags: Vec<String> = tags
            .split('.')
            .filter(|t| !t.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let has = |words: &[&str]| tags.iter().any(|t| words.contains(&t.as_str()));
        // A language tag, with a region after a hyphen when there is one ("pt-br", "es-419").
        let is_language = |t: &String, flags: &[&str]| {
            let base = t.split_once('-').map_or(t.as_str(), |(base, _)| base);
            (2..=3).contains(&base.len())
                && base.bytes().all(|b| b.is_ascii_alphabetic())
                && crate::languages::Language::parse(t).is_some()
                && !flags.contains(&t.as_str())
        };
        // "hi" is Hindi, and also what Bazarr writes for hearing impaired ("Movie.en.hi.srt"):
        // it's the flag when another language tag is there, the language when it's alone.
        let other_language = tags
            .iter()
            .find(|t| is_language(t, &["sdh", "cc", "hi", "forced"]));
        let language = other_language
            .or_else(|| tags.iter().find(|t| t.as_str() == "hi"))
            .cloned();
        let hearing_impaired = has(&["sdh", "cc"]) || (other_language.is_some() && has(&["hi"]));
        let metadata = entry.metadata()?;
        let digest = Sha256::digest(name.as_bytes());
        found.push(Sidecar {
            id: digest[..4].iter().map(|b| format!("{b:02x}")).collect(),
            file: name.to_owned(),
            codec: match extension.as_str() {
                "srt" => "subrip".into(),
                other => other.into(),
            },
            language,
            forced: has(&["forced"]),
            hearing_impaired,
            path: entry.path(),
            size: metadata.len(),
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs()),
        });
    }
    found.sort_by(|a, b| a.file.cmp(&b.file));
    Ok(found)
}
