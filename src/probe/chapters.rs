//! A file's chapters, as its author marked them: kept as they are, none made up, and the ones
//! named as an intro or credits found by name.

use super::*;

/// A chapter of a file, from the file itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    /// Where it starts, in seconds from the beginning of the file.
    #[serde(rename = "start_seconds")]
    pub start: f64,
    /// Where it ends, in seconds from the beginning of the file.
    #[serde(rename = "end_seconds")]
    pub end: f64,
    /// Its title, such as "Opening"; None when it has none.
    pub title: Option<String>,
}

/// What probing a file found: its streams, and its chapters.
#[derive(Debug, Clone, PartialEq)]
pub struct Probed {
    /// Its container, duration and streams.
    pub info: MediaInfo,
    /// Its chapters; None when it wasn't read for them.
    pub chapters: Option<Vec<Chapter>>,
}

#[derive(Deserialize)]
struct RawChapters {
    #[serde(default)]
    chapters: Vec<RawChapter>,
}

#[derive(Deserialize)]
struct RawChapter {
    start_time: Option<String>,
    end_time: Option<String>,
    #[serde(default)]
    tags: HashMap<String, String>,
}

/// The chapters in ffprobe's JSON (`-show_chapters`), in order. A chapter without times, of no
/// length, or starting before the one kept before it is left out, and nothing is made up: a
/// file without chapters has none.
///
/// # Errors
///
/// Fails when the output isn't JSON.
pub fn parse_chapters(json: &[u8]) -> anyhow::Result<Vec<Chapter>> {
    let raw: RawChapters = serde_json::from_slice(json).context("reading ffprobe's chapters")?;
    let mut chapters: Vec<Chapter> = Vec::new();
    for c in raw.chapters {
        let time = |t: &Option<String>| t.as_deref()?.parse::<f64>().ok().filter(|t| t.is_finite());
        let (Some(start), Some(end)) = (time(&c.start_time), time(&c.end_time)) else {
            continue;
        };
        let in_order = chapters.last().is_none_or(|before| start >= before.start);
        if start < 0.0 || end <= start || !in_order {
            continue;
        }
        let title = c
            .tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("title"))
            .map(|(_, v)| v.trim().to_owned())
            .filter(|t| !t.is_empty());
        chapters.push(Chapter { start, end, title });
    }
    Ok(chapters)
}

/// Runs ffprobe on one file and reads its streams and chapters, within a minute.
///
/// # Errors
///
/// As [`probe`].
pub async fn probe_with_chapters(ffprobe: &Path, file: &Path) -> Result<Probed, ProbeError> {
    let output = run_ffprobe(ffprobe, file).await?;
    let info = parse(&output).map_err(|err| ProbeError::Failed(format!("{err:#}")))?;
    let chapters = parse_chapters(&output).map_err(|err| ProbeError::Failed(format!("{err:#}")))?;
    Ok(Probed {
        info,
        chapters: Some(chapters),
    })
}

/// The chapters of a file as it is now; none when it has none, or hasn't been read for them.
pub async fn chapters_of(pool: &SqlitePool, file_id: i64) -> anyhow::Result<Vec<Chapter>> {
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT p.chapters FROM media_probes p JOIN media_files f ON f.id = p.file_id
         WHERE p.file_id = ? AND p.size = f.size AND p.modified_at = f.modified_at",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(match stored {
        Some(json) => serde_json::from_str(&json)?,
        None => Vec::new(),
    })
}

/// What a chapter's title marks it as, when it's one of the names files give an intro or the
/// credits: "Opening", "OP", "Intro", "Opening Credits"; "Ending", "ED", "Credits", "End
/// Credits". A number after the name ("OP2", "Ending 1") doesn't matter; case and punctuation
/// don't either.
pub fn chapter_kind(title: &str) -> Option<ChapterKind> {
    let name = normalised(title);
    if INTRO_NAMES.contains(&name.as_str()) {
        Some(ChapterKind::Intro)
    } else if CREDITS_NAMES.contains(&name.as_str()) {
        Some(ChapterKind::Credits)
    } else {
        None
    }
}

/// What a named chapter is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChapterKind {
    /// The opening titles or song.
    Intro,
    /// The end credits or ending song.
    Credits,
}

const INTRO_NAMES: &[&str] = &[
    "opening",
    "op",
    "intro",
    "introduction",
    "opening credits",
    "opening titles",
    "opening theme",
    "opening song",
    "main titles",
];

const CREDITS_NAMES: &[&str] = &[
    "ending",
    "ed",
    "credits",
    "end credits",
    "ending credits",
    "closing credits",
    "end titles",
    "ending theme",
    "ending song",
    "outro",
];

/// A chapter's title as its name is compared: lower case, words only, a number after each
/// word dropped ("OP2" is "op", "Ending 1" is "ending").
fn normalised(title: &str) -> String {
    let words: Vec<String> = title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned())
        .filter(|w| !w.is_empty())
        .collect();
    words.join(" ")
}

const RECAP_NAMES: &[&str] = &[
    "recap",
    "previously",
    "previously on",
    "last time",
    "story so far",
    "the story so far",
];

const PREVIEW_NAMES: &[&str] = &[
    "preview",
    "next episode",
    "next episode preview",
    "preview of the next episode",
    "next time",
];

/// Whether a chapter's title names it a recap: "Recap", "Previously", "Previously on…", "Last
/// Time", "The Story So Far". Case, punctuation and a number after the name don't matter.
pub fn is_recap(title: &str) -> bool {
    let name = normalised(title);
    RECAP_NAMES.contains(&name.as_str()) || name.starts_with("previously on ")
}

/// Whether a chapter's title names it a preview of what comes next: "Preview", "Next Episode",
/// "Next Time". Case, punctuation and a number after the name don't matter.
pub fn is_preview(title: &str) -> bool {
    PREVIEW_NAMES.contains(&normalised(title).as_str())
}

/// The chapters marked as a recap and as a preview: the first named as a recap and the last
/// named as a preview.
pub fn named_recap_and_preview(chapters: &[Chapter]) -> (Option<&Chapter>, Option<&Chapter>) {
    let titled = |test: fn(&str) -> bool| move |c: &&Chapter| c.title.as_deref().is_some_and(test);
    (
        chapters.iter().find(titled(is_recap)),
        chapters.iter().rev().find(titled(is_preview)),
    )
}

/// The chapters marked as the intro and as the credits: the first named as an intro and the
/// last named as credits.
pub fn named_segments(chapters: &[Chapter]) -> (Option<&Chapter>, Option<&Chapter>) {
    let named = |kind: ChapterKind| {
        move |c: &&Chapter| c.title.as_deref().and_then(chapter_kind) == Some(kind)
    };
    (
        chapters.iter().find(named(ChapterKind::Intro)),
        chapters.iter().rev().find(named(ChapterKind::Credits)),
    )
}

#[cfg(test)]
mod tests;
