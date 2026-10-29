//! Matching an imported track list (a Spotify export, say) against the library, by the rules
//! Aoide's apps share, so the server and a phone agree on what's missing
//! (aoide-sidecar's contract, "Matching an imported track list"; its 16-row table is a test).
//!
//! Titles and artists are compared as word sets after normalising: lower case, accents
//! folded, `&` as `and`, decorations dropped (bracketed groups and ` - ` suffixes holding a
//! noise word like "feat" or "remastered"), letters and digits only. Equal sets score 1, one
//! inside the other 0.9, else their Jaccard index. Durations within 5 s score 1, within 15 s
//! 0.8, beyond that it's another recording; unknown on either side 0.9. A match needs title
//! 0.75 and artist 0.7, or, with no artist on one side, an exact title. An ISRC on both sides
//! settles it alone.

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// One list across Aoide's apps and the server.
const NOISE_WORDS: [&str; 30] = [
    "feat",
    "featuring",
    "ft",
    "remaster",
    "remastered",
    "remix",
    "remixed",
    "mix",
    "live",
    "version",
    "edit",
    "edited",
    "deluxe",
    "mono",
    "stereo",
    "acoustic",
    "instrumental",
    "demo",
    "radio",
    "single",
    "bonus",
    "anniversary",
    "explicit",
    "clean",
    "extended",
    "reissue",
    "with",
    "from",
    "original",
    "edition",
];
const TITLE_FLOOR: f64 = 0.75;
const ARTIST_FLOOR: f64 = 0.7;
const UNKNOWN: f64 = 0.9;

/// A row of an imported list.
#[derive(Debug, Clone, Default)]
pub struct Imported {
    /// The row's title; none or blank matches nothing but by ISRC.
    pub title: Option<String>,
    /// The row's artists, as listed.
    pub artists: Vec<String>,
    /// The row's length in milliseconds, when the list gives one.
    pub duration_ms: Option<i64>,
    /// The row's ISRC, when the list gives one; a match on it wins outright.
    pub isrc: Option<String>,
}

/// A track in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryTrack {
    /// The track's id.
    pub id: String,
    /// Its title.
    pub title: Option<String>,
    /// Its artists.
    pub artists: Vec<String>,
    /// Its album's title.
    pub album: Option<String>,
    /// Its length in milliseconds.
    pub duration_ms: Option<i64>,
    /// Its ISRC, from its tags.
    pub isrc: Option<String>,
}

/// A library track accepted for an imported row, and how well each part of it fits.
#[derive(Debug, PartialEq)]
pub struct Match<'a> {
    /// The track found.
    pub track: &'a LibraryTrack,
    /// The weighted score the match was accepted on, 0 to 1; 1.0 for an ISRC match.
    pub confidence: f64,
    /// How well the titles' words agree, 0 to 1.
    pub title_score: f64,
    /// How well the best pair of artists agree, 0 to 1; 0 when either side lists none.
    pub artist_score: f64,
    /// How close the lengths are, 0 to 1: 1.0 within five seconds, 0.8 within fifteen, 0.9 when
    /// either is unknown.
    pub duration_score: f64,
}

/// The words of a text: runs of letters and digits, in any script.
fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn noisy(text: &str) -> bool {
    words(text)
        .iter()
        .any(|w| NOISE_WORDS.contains(&w.as_str()))
}

/// Lower case with accents folded: decomposed, combining marks dropped, recomposed. (Every
/// kind of mark goes; the plugin dropped non-spacing ones only, which differs for spacing
/// marks such as some Indic vowel signs. Both sides are folded alike.)
fn fold(text: &str) -> String {
    text.to_lowercase()
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .nfc()
        .collect()
}

/// A title or name as the words that count: decorations dropped, "(What's the Story)" kept.
pub fn word_set(raw: Option<&str>) -> HashSet<String> {
    let Some(raw) = raw.filter(|r| !r.trim().is_empty()) else {
        return HashSet::new();
    };
    let text = fold(raw).replace('&', " and ");
    // Bracketed groups, each up to its first closing bracket, kept unless decoration.
    let mut kept = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find(['(', '[', '{']) {
        let close = match rest.as_bytes()[start] {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        };
        let Some(length) = rest[start + 1..].find(close) else {
            break;
        };
        let end = start + 1 + length + 1;
        kept.push_str(&rest[..start]);
        let group = &rest[start..end];
        kept.push_str(if noisy(group) { " " } else { group });
        rest = &rest[end..];
    }
    kept.push_str(rest);
    // " - Remastered 2011" goes; " - Part 2" stays.
    let mut segments = kept.split(" - ");
    let mut text = segments.next().unwrap_or_default().to_owned();
    for segment in segments {
        if !noisy(segment) {
            text.push(' ');
            text.push_str(segment);
        }
    }
    words(&text)
}

fn set_score(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    if a.is_subset(b) || b.is_subset(a) {
        return 0.9;
    }
    let shared = a.intersection(b).count();
    shared as f64 / (a.len() + b.len() - shared) as f64
}

fn duration_score(a: Option<i64>, b: Option<i64>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => match (a - b).abs() {
            0..=5_000 => 1.0,
            5_001..=15_000 => 0.8,
            _ => 0.0,
        },
        _ => UNKNOWN,
    }
}

fn rank(title: f64, artist: f64, duration: f64) -> f64 {
    0.5 * title + 0.35 * artist + 0.15 * duration
}

struct Indexed {
    title: HashSet<String>,
    artists: Vec<HashSet<String>>,
}

/// The library, indexed by title word and ISRC for one request's rows.
pub struct Matcher<'a> {
    tracks: &'a [LibraryTrack],
    indexed: Vec<Indexed>,
    by_word: HashMap<String, Vec<usize>>,
    by_isrc: HashMap<String, usize>,
}

impl<'a> Matcher<'a> {
    /// Indexes `tracks` by title word and ISRC.
    pub fn new(tracks: &'a [LibraryTrack]) -> Self {
        let mut by_word: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_isrc = HashMap::new();
        let mut indexed = Vec::with_capacity(tracks.len());
        for (i, track) in tracks.iter().enumerate() {
            let title = word_set(track.title.as_deref());
            for word in &title {
                by_word.entry(word.clone()).or_default().push(i);
            }
            if let Some(isrc) = track
                .isrc
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                by_isrc.entry(isrc.to_ascii_uppercase()).or_insert(i);
            }
            let artists = track
                .artists
                .iter()
                .map(|a| word_set(Some(a)))
                .filter(|w| !w.is_empty())
                .collect();
            indexed.push(Indexed { title, artists });
        }
        Self {
            tracks,
            indexed,
            by_word,
            by_isrc,
        }
    }

    /// How many tracks the library has.
    pub fn library_size(&self) -> usize {
        self.tracks.len()
    }

    /// The best accepted track for a row, if any.
    pub fn find(&self, row: &Imported) -> Option<Match<'a>> {
        if let Some(isrc) = row.isrc.as_deref().map(str::trim).filter(|s| !s.is_empty())
            && let Some(&i) = self.by_isrc.get(&isrc.to_ascii_uppercase())
        {
            return Some(Match {
                track: &self.tracks[i],
                confidence: 1.0,
                title_score: 1.0,
                artist_score: 1.0,
                duration_score: 1.0,
            });
        }
        let title = word_set(row.title.as_deref());
        if title.is_empty() {
            return None;
        }
        let artists: Vec<HashSet<String>> = row
            .artists
            .iter()
            .map(|a| word_set(Some(a)))
            .filter(|w| !w.is_empty())
            .collect();
        let mut candidates: Vec<usize> = title
            .iter()
            .filter_map(|w| self.by_word.get(w))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        candidates.dedup();
        let mut best: Option<Match<'a>> = None;
        for i in candidates {
            if let Some(found) = self.score(i, &title, &artists, row.duration_ms)
                && best
                    .as_ref()
                    .is_none_or(|b| found.confidence > b.confidence)
            {
                best = Some(found);
            }
        }
        best
    }

    fn score(
        &self,
        i: usize,
        title: &HashSet<String>,
        artists: &[HashSet<String>],
        duration_ms: Option<i64>,
    ) -> Option<Match<'a>> {
        let track = &self.tracks[i];
        let indexed = &self.indexed[i];
        let duration = duration_score(duration_ms, track.duration_ms);
        if duration <= 0.0 {
            return None;
        }
        let title_score = set_score(title, &indexed.title);
        if artists.is_empty() || indexed.artists.is_empty() {
            // Nothing tells two same-titled songs apart: only an exact title is trusted.
            return (title_score >= 1.0).then(|| Match {
                track,
                confidence: rank(title_score, 0.0, duration),
                title_score,
                artist_score: 0.0,
                duration_score: duration,
            });
        }
        let artist = artists
            .iter()
            .flat_map(|mine| {
                indexed
                    .artists
                    .iter()
                    .map(move |theirs| set_score(mine, theirs))
            })
            .fold(0.0, f64::max);
        (title_score >= TITLE_FLOOR && artist >= ARTIST_FLOOR).then(|| Match {
            track,
            confidence: rank(title_score, artist, duration),
            title_score,
            artist_score: artist,
            duration_score: duration,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    const TABLE: &str = include_str!("../../tests/fixtures/aoide/match-table.json");

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The table Aoide's apps share (each client keeps the same copy):
    /// the server must agree with it row for row.
    #[test]
    fn agrees_with_the_apps_shared_table() {
        let table: Vec<Value> = serde_json::from_str(TABLE).unwrap();
        assert_eq!(table.len(), 16);
        for case in table {
            let library: Vec<LibraryTrack> = case["library"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| LibraryTrack {
                    id: t["jellyfinId"].as_str().unwrap().to_owned(),
                    title: t["title"].as_str().map(str::to_owned),
                    artists: strings(&t["artists"]),
                    album: t["album"].as_str().map(str::to_owned),
                    duration_ms: t["durationMs"].as_i64(),
                    isrc: t["isrc"].as_str().map(str::to_owned),
                })
                .collect();
            let row = &case["import"];
            let imported = Imported {
                title: row["title"].as_str().map(str::to_owned),
                artists: strings(&row["artists"]),
                duration_ms: row["durationMs"].as_i64(),
                isrc: row["isrc"].as_str().map(str::to_owned),
            };
            let found = Matcher::new(&library)
                .find(&imported)
                .map(|m| m.track.id.clone());
            assert_eq!(
                found.as_deref(),
                case["expected"].as_str(),
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn normalises_titles_as_the_apps_do() {
        let set = |s: &str| {
            let mut words: Vec<String> = word_set(Some(s)).into_iter().collect();
            words.sort();
            words.join(" ")
        };
        assert_eq!(set("Crazy in Love (feat. Jay-Z)"), "crazy in love");
        assert_eq!(
            set("(What's the Story) Morning Glory?"),
            "glory morning s story the what"
        );
        assert_eq!(set("Karma Police - Remastered 2009"), "karma police");
        assert_eq!(set("Part 1 - Part 2"), "1 2 part");
        assert_eq!(set("Beyoncé & Co [Live]"), "and beyonce co");
        assert_eq!(set("Город (Remix)"), "город");
        assert!(word_set(Some("  ")).is_empty());
        assert!(word_set(None).is_empty());
    }

    #[test]
    fn an_isrc_on_both_sides_settles_it() {
        let library = vec![LibraryTrack {
            id: "1".into(),
            title: Some("Something Else".into()),
            artists: vec!["Nobody".into()],
            album: None,
            duration_ms: Some(1),
            isrc: Some("usug12305108".into()),
        }];
        let row = Imported {
            title: Some("bad idea right?".into()),
            artists: vec!["Olivia Rodrigo".into()],
            duration_ms: Some(184_000),
            isrc: Some(" USUG12305108 ".into()),
        };
        let found = Matcher::new(&library).find(&row).unwrap();
        assert_eq!((found.track.id.as_str(), found.confidence), ("1", 1.0));
    }
}
