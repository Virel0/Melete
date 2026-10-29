//! Which audio and subtitle tracks to play a file with for a user, and why, so every app picks
//! the same: the tracks the user last chose by hand in the series, else their languages in
//! order, else what the file marks as its default. Untagged tracks are never taken for a match.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::languages::Language;
use crate::probe::MediaInfo;
use crate::subtitles::Sidecar;

mod stored;

pub use stored::*;

/// When to show subtitles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleMode {
    /// Never, forced ones included.
    Off,
    /// Only forced subtitles, the parts in another language.
    Forced,
    /// Full subtitles when the audio isn't in one of the user's subtitle languages (or its
    /// language isn't known), else forced ones.
    Foreign,
    /// Full subtitles always.
    Always,
}

impl SubtitleMode {
    /// Every mode as the API names it.
    pub const NAMES: [&str; 4] = ["off", "forced", "foreign", "always"];

    /// The mode the API names `name`.
    pub fn parse(name: &str) -> Option<SubtitleMode> {
        Some(match name {
            "off" => SubtitleMode::Off,
            "forced" => SubtitleMode::Forced,
            "foreign" => SubtitleMode::Foreign,
            "always" => SubtitleMode::Always,
            _ => return None,
        })
    }

    /// The API's name for the mode.
    pub fn name(self) -> &'static str {
        match self {
            SubtitleMode::Off => "off",
            SubtitleMode::Forced => "forced",
            SubtitleMode::Foreign => "foreign",
            SubtitleMode::Always => "always",
        }
    }
}

/// A user's languages, most wanted first.
#[derive(Debug, Clone)]
pub struct Preferences {
    /// Audio languages.
    pub audio: Vec<Language>,
    /// Subtitle languages; the audio counts as understood when it's in one of these.
    pub subtitles: Vec<Language>,
    /// When to show subtitles.
    pub mode: SubtitleMode,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            audio: Vec::new(),
            subtitles: Vec::new(),
            mode: SubtitleMode::Foreign,
        }
    }
}

/// The subtitles a user last chose by hand in a series.
#[derive(Debug, Clone, PartialEq)]
pub enum SubtitleChoice {
    /// Turned off.
    Off,
    /// A track in this language, forced or not, for the hearing impaired or not.
    On {
        /// Its language.
        language: Language,
        /// Whether it was a forced track.
        forced: bool,
        /// Whether it was for the hearing impaired.
        hearing_impaired: bool,
    },
}

/// The tracks a user last chose by hand in a series; either part may be unknown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeriesChoice {
    /// The audio's language.
    pub audio: Option<Language>,
    /// The subtitles.
    pub subtitles: Option<SubtitleChoice>,
}

/// A subtitle track as picking sees it, embedded or beside the file.
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    /// As the subtitle list names it: `s{index}` or `x{id}`.
    pub track: String,
    /// Its language, when tagged.
    pub language: Option<Language>,
    /// Whether the file marks it as the default.
    pub default: bool,
    /// Only the parts in another language.
    pub forced: bool,
    /// For the hearing impaired.
    pub hearing_impaired: bool,
    /// Text rather than pictures, which can be shown without converting the video.
    pub text: bool,
}

/// The tracks to play with, and why each was picked.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pick {
    /// The audio stream's index; None for a file without audio.
    pub audio: Option<i64>,
    /// `series_choice`, `preferred_language`, `file_default` or `no_tracks`.
    pub audio_reason: &'static str,
    /// The subtitle track as the subtitle list names it; None for none.
    pub subtitle: Option<String>,
    /// `series_choice`, `always`, `foreign_audio`, `forced`, `off`, `audio_understood`,
    /// `no_match` or `no_tracks`.
    pub subtitle_reason: &'static str,
}

/// A file's subtitle tracks, embedded first, then those beside it.
pub fn subtitle_tracks(media: Option<&MediaInfo>, sidecars: &[Sidecar]) -> Vec<SubtitleTrack> {
    let embedded = media
        .iter()
        .flat_map(|m| &m.subtitles)
        .map(|s| SubtitleTrack {
            track: format!("s{}", s.index),
            language: s.language.as_deref().and_then(Language::parse),
            default: s.default,
            forced: s.forced,
            hearing_impaired: false,
            text: s.text,
        });
    let beside = sidecars.iter().map(|s| SubtitleTrack {
        track: format!("x{}", s.id),
        language: s.language.as_deref().and_then(Language::parse),
        default: false,
        forced: s.forced,
        hearing_impaired: s.hearing_impaired,
        text: true,
    });
    embedded.chain(beside).collect()
}

/// The tracks to play `media` with, among its audio and `subtitles`.
pub fn pick(
    preferences: &Preferences,
    choice: &SeriesChoice,
    media: Option<&MediaInfo>,
    subtitles: &[SubtitleTrack],
) -> Pick {
    let audio_streams = media.map_or(&[][..], |m| &m.audio[..]);
    let (audio, audio_reason) = pick_audio(preferences, choice, audio_streams);
    let audio_language = audio
        .and_then(|i| audio_streams.iter().find(|a| a.index == i))
        .and_then(|a| a.language.as_deref())
        .and_then(Language::parse);
    let (subtitle, subtitle_reason) =
        pick_subtitle(preferences, choice, audio_language.as_ref(), subtitles);
    Pick {
        audio,
        audio_reason,
        subtitle,
        subtitle_reason,
    }
}

fn pick_audio(
    preferences: &Preferences,
    choice: &SeriesChoice,
    streams: &[crate::probe::AudioStream],
) -> (Option<i64>, &'static str) {
    if streams.is_empty() {
        return (None, "no_tracks");
    }
    // The best stream in a language: its region matching, then the file's default, then first.
    let best_in = |wanted: &Language| {
        streams
            .iter()
            .filter_map(|a| {
                let language = Language::parse(a.language.as_deref()?)?;
                Some((wanted.answers(&language)?, a))
            })
            .max_by_key(|(score, a)| (*score, a.default, std::cmp::Reverse(a.index)))
            .map(|(_, a)| a.index)
    };
    if let Some(index) = choice.audio.as_ref().and_then(best_in) {
        return (Some(index), "series_choice");
    }
    if let Some(index) = preferences.audio.iter().find_map(best_in) {
        return (Some(index), "preferred_language");
    }
    let default = streams.iter().find(|a| a.default).unwrap_or(&streams[0]);
    (Some(default.index), "file_default")
}

/// The best track among `tracks` that `wanted` answers and `keep` keeps: the language's region
/// matching, then `prefer`, then text (pictures need converting to be drawn in), then the
/// file's default, then the first listed.
fn best_track<'a>(
    tracks: &'a [SubtitleTrack],
    wanted: &Language,
    keep: impl Fn(&SubtitleTrack) -> bool,
    prefer: impl Fn(&SubtitleTrack) -> bool,
) -> Option<&'a SubtitleTrack> {
    tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| keep(t))
        .filter_map(|(i, t)| Some((wanted.answers(t.language.as_ref()?)?, i, t)))
        .max_by_key(|(score, i, t)| (*score, prefer(t), t.text, t.default, std::cmp::Reverse(*i)))
        .map(|(_, _, t)| t)
}

fn pick_subtitle(
    preferences: &Preferences,
    choice: &SeriesChoice,
    audio: Option<&Language>,
    tracks: &[SubtitleTrack],
) -> (Option<String>, &'static str) {
    if tracks.is_empty() {
        return (None, "no_tracks");
    }
    if let Some(remembered) = chosen_subtitle(choice, tracks) {
        return (remembered, "series_choice");
    }
    let mode = preferences.mode;
    if mode == SubtitleMode::Off {
        return (None, "off");
    }
    let wanted = &preferences.subtitles;
    let understood = audio.is_some_and(|a| wanted.iter().any(|w| w.answers(a).is_some()));
    let full = match mode {
        SubtitleMode::Always => Some("always"),
        SubtitleMode::Foreign if !understood => Some("foreign_audio"),
        _ => None,
    };
    if let Some(reason) = full {
        let found = wanted
            .iter()
            .find_map(|w| best_track(tracks, w, |t| !t.forced, |t| !t.hearing_impaired));
        if let Some(t) = found {
            return (Some(t.track.clone()), reason);
        }
    }
    if let Some(t) = forced_track(wanted, audio, tracks) {
        return (Some(t.track.clone()), "forced");
    }
    let reason = if mode == SubtitleMode::Foreign && understood {
        "audio_understood"
    } else {
        "no_match"
    };
    (None, reason)
}

/// What the series choice picks: Some(None) when it turned subtitles off, None when there's
/// no choice or no track like the one chosen.
fn chosen_subtitle(choice: &SeriesChoice, tracks: &[SubtitleTrack]) -> Option<Option<String>> {
    match choice.subtitles.as_ref()? {
        SubtitleChoice::Off => Some(None),
        SubtitleChoice::On {
            language,
            forced,
            hearing_impaired,
        } => best_track(
            tracks,
            language,
            |t| t.forced == *forced,
            |t| t.hearing_impaired == *hearing_impaired,
        )
        .map(|t| Some(t.track.clone())),
    }
}

/// A forced track in one of the user's subtitle languages, else in the audio's language; with
/// no subtitle languages set, the file's first forced track.
fn forced_track<'a>(
    wanted: &[Language],
    audio: Option<&Language>,
    tracks: &'a [SubtitleTrack],
) -> Option<&'a SubtitleTrack> {
    let forced = |t: &SubtitleTrack| t.forced;
    let no_preference = |_: &SubtitleTrack| false;
    wanted
        .iter()
        .chain(audio)
        .find_map(|w| best_track(tracks, w, forced, no_preference))
        .or_else(|| {
            if !wanted.is_empty() {
                return None;
            }
            let mut forced: Vec<&SubtitleTrack> = tracks.iter().filter(|t| t.forced).collect();
            forced.sort_by_key(|t| (!t.text, !t.default));
            forced.first().copied()
        })
}

#[cfg(test)]
mod tests;
