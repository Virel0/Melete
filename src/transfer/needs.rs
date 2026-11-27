//! Whether a download can be understood, for a profile that needs subtitles in a language
//! (`subtitles_language`, English for anime profiles): each video must have audio in that
//! language, or subtitles in it, inside the file or beside it. A raw (no subtitles at all) or
//! one with subtitles in other languages only is set aside at import, blocklisted and searched
//! again, as a release's name rarely says which subtitles it carries.
//!
//! A subtitle track counts when it's tagged with the language, or untagged and its title
//! names no other language (many fansubs tag nothing). A track that shows only signs and songs
//! never counts: it translates what's on screen, not what's said. Audio counts only when it's
//! tagged with the language.

use crate::languages::Language;
use crate::probe::MediaInfo;

use super::*;

/// Words naming a language in a track's title or a subtitle file's name, with its code.
const NAMES: &[(&str, &str)] = &[
    ("english", "en"),
    ("spanish", "es"),
    ("español", "es"),
    ("espanol", "es"),
    ("castellano", "es"),
    ("latino", "es"),
    ("portuguese", "pt"),
    ("português", "pt"),
    ("portugues", "pt"),
    ("brazilian", "pt"),
    ("french", "fr"),
    ("français", "fr"),
    ("francais", "fr"),
    ("german", "de"),
    ("deutsch", "de"),
    ("italian", "it"),
    ("italiano", "it"),
    ("russian", "ru"),
    ("arabic", "ar"),
    ("chinese", "zh"),
    ("korean", "ko"),
    ("indonesian", "id"),
    ("malay", "ms"),
    ("thai", "th"),
    ("vietnamese", "vi"),
    ("polish", "pl"),
    ("turkish", "tr"),
    ("dutch", "nl"),
    ("japanese", "ja"),
    ("hindi", "hi"),
];

/// The language a download's profile needs subtitles in, when its audio isn't in it.
pub(crate) async fn needed(pool: &SqlitePool, row: &Row) -> anyhow::Result<Option<Language>> {
    let Some(target) = row.target() else {
        return Ok(None);
    };
    let Some(wanted) = crate::search::load_wanted(pool, target).await? else {
        return Ok(None);
    };
    let Ok(loaded) = crate::search::profile_for(pool, target, &wanted).await? else {
        return Ok(None);
    };
    Ok(loaded
        .profile
        .subtitles_language
        .as_deref()
        .and_then(Language::parse))
}

/// The languages words in `text` name (lower case, split at anything not a letter).
fn named_in(text: &str) -> Vec<&'static str> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter_map(|word| {
            NAMES
                .iter()
                .find(|(name, _)| *name == word)
                .map(|(_, code)| *code)
        })
        .collect()
}

/// Whether a text is a track or file for signs and songs alone, or forced parts.
fn signs_only(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("forced") || text.contains("signs")
}

/// What a subtitle file beside the video is in, by its name: a code before its extension
/// (".en.ass", ".eng.forced.srt") or a language's name in it; None for a name that says none.
fn sidecar_language(name: &str) -> Option<Language> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    for part in stem.rsplit('.').take(2) {
        if let Some(language) = Language::parse(part).filter(|l| l.base.len() == 2) {
            return Some(language);
        }
    }
    named_in(stem)
        .first()
        .and_then(|code| Language::parse(code))
}

/// Why video `name` (read as `info`, with subtitle files `beside` it) offers nothing in
/// `needed`; None when it has the audio or the subtitles.
pub(crate) fn lacking(
    name: &str,
    info: &MediaInfo,
    beside: &[&str],
    needed: &Language,
) -> Option<String> {
    let is_needed =
        |code: &str| Language::parse(code).is_some_and(|l| needed.answers(&l).is_some());
    if info
        .audio
        .iter()
        .any(|a| a.language.as_deref().is_some_and(is_needed))
    {
        return None;
    }
    let inside = info.subtitles.iter().any(|s| {
        if s.counts_as_forced() {
            return false;
        }
        match s.language.as_deref() {
            Some(code) => is_needed(code),
            None => s.title.as_deref().is_none_or(|title| {
                let named = named_in(title);
                named.is_empty() || named.iter().any(|code| is_needed(code))
            }),
        }
    });
    let alongside = beside.iter().any(|file| {
        !signs_only(file) && sidecar_language(file).is_none_or(|l| needed.answers(&l).is_some())
    });
    if inside || alongside {
        return None;
    }
    let audio: Vec<String> = info
        .audio
        .iter()
        .filter_map(|a| a.language.clone())
        .collect();
    let heard = if audio.is_empty() {
        "untagged".to_owned()
    } else {
        audio.join(", ")
    };
    let shown = if info.subtitles.is_empty() && beside.is_empty() {
        "no subtitles".to_owned()
    } else {
        format!("no {} subtitles", needed.code())
    };
    Some(format!(
        "{name} has {heard} audio and {shown}, and the profile needs {} audio or subtitles",
        needed.code()
    ))
}

#[cfg(test)]
mod tests;
