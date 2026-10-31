//! What searches and other lookups answer, and how a search is written so a title can't change it.

use super::*;

/// A search result: a release, how well it scored, and its release group.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Found {
    /// The release's MusicBrainz id.
    pub id: String,
    /// How well it matched the search, 0 to 100.
    pub score: i64,
    /// The release's title.
    pub title: String,
    /// `Official`, `Promotion`, `Bootleg` and the like; none when MusicBrainz doesn't say.
    pub status: Option<String>,
    /// Its release date as MusicBrainz writes it: `2011`, `2011-03` or `2011-03-14`.
    pub date: Option<String>,
    /// How many tracks it has on all its media.
    #[serde(rename = "track-count")]
    pub track_count: Option<i64>,
    /// Its artists, in order, each with what joins it to the next.
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
    /// The album it's an edition of.
    #[serde(rename = "release-group")]
    pub release_group: ReleaseGroup,
}

/// A release group with its artists, as looked up by id.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReleaseGroupDetail {
    /// The release group's MusicBrainz id.
    pub id: String,
    /// Its title.
    pub title: String,
    /// `Album`, `Single`, `EP` and the like; none when MusicBrainz doesn't say.
    #[serde(rename = "primary-type")]
    pub primary_type: Option<String>,
    /// `Live`, `Compilation`, `Soundtrack` and the like; none or empty for a plain album.
    #[serde(rename = "secondary-types", default)]
    pub secondary_types: Option<Vec<String>>,
    /// Its earliest release's date, as MusicBrainz writes it.
    #[serde(rename = "first-release-date")]
    pub first_release_date: Option<String>,
    /// Its artists, in order, each with what joins it to the next.
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
}

/// An artist, as looked up by id.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Artist {
    /// The artist's MusicBrainz id.
    pub id: String,
    /// Their name.
    pub name: String,
}

/// A recording with its artists and the releases it's on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecordingDetail {
    /// The recording's MusicBrainz id.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Milliseconds.
    pub length: Option<i64>,
    /// Its artists, in order, each with what joins it to the next.
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
    /// The releases it's on.
    #[serde(default)]
    pub releases: Vec<RecordingRelease>,
}

/// A release a recording is on, as far as matching reads it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RecordingRelease {
    /// The release's title.
    pub title: String,
    /// `Official`, `Promotion`, `Bootleg` and the like; none when MusicBrainz doesn't say.
    pub status: Option<String>,
    /// The album it's an edition of, when MusicBrainz includes it.
    #[serde(rename = "release-group")]
    pub release_group: Option<ReleaseGroup>,
}

/// An artist found by name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FoundArtist {
    /// The artist's MusicBrainz id.
    pub id: String,
    /// Their name.
    pub name: String,
    /// What tells it from others of the name ("English rock band").
    pub disambiguation: Option<String>,
    /// Person, Group…
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Where they're from, as an ISO 3166 code (`GB`); none when unknown.
    pub country: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct RecordingSearch {
    #[serde(default)]
    pub(super) recordings: Vec<RecordingDetail>,
}

#[derive(Deserialize)]
pub(super) struct IsrcLookup {
    #[serde(default)]
    pub(super) recordings: Vec<RecordingDetail>,
}

/// Whether `code` is shaped like an ISRC: country, registrant, year and number (CCXXXYYNNNNN).
pub fn is_isrc(code: &str) -> bool {
    let b = code.as_bytes();
    b.len() == 12
        && b[..2].iter().all(u8::is_ascii_uppercase)
        && b[2..5]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && b[5..].iter().all(u8::is_ascii_digit)
}

/// Results a search for a request asks for.
pub(super) const FOUND: &str = "20";

#[derive(Deserialize)]
pub(super) struct Browse {
    #[serde(rename = "release-groups", default)]
    pub(super) release_groups: Vec<ReleaseGroup>,
    #[serde(rename = "release-group-count", default)]
    pub(super) count: usize,
}

/// An artist's release groups asked for at a time, and pages at most.
pub(super) const BROWSE_PAGE: usize = 100;
pub(super) const BROWSE_PAGES: usize = 5;

#[derive(Deserialize)]
pub(super) struct Search {
    #[serde(default)]
    pub(super) releases: Vec<Found>,
}

/// An artist credit as one line: "Simon & Garfunkel".
pub fn credit_line(credits: &[Credit]) -> String {
    credits
        .iter()
        .map(|c| format!("{}{}", c.name, c.joinphrase))
        .collect()
}

/// A Lucene phrase: quotes and backslashes escaped, so a title can't change the query.
pub(super) fn phrase(text: &str) -> String {
    let escaped: String = text
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            _ => vec![c],
        })
        .collect();
    format!("\"{escaped}\"")
}
