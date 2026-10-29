//! Releases as looked up, with their tracks, recordings and credits, and the works recordings
//! perform.

use super::*;

/// A release as looked up, with its tracks.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Release {
    /// The release's MusicBrainz id.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its release date as MusicBrainz writes it: `2011`, `2011-03` or `2011-03-14`.
    pub date: Option<String>,
    /// `Official`, `Promotion`, `Bootleg` and the like; none when MusicBrainz doesn't say.
    pub status: Option<String>,
    /// Where it was released, as an ISO 3166 code (`GB`, or `XW` for worldwide).
    pub country: Option<String>,
    /// The album it's an edition of.
    #[serde(rename = "release-group")]
    pub release_group: ReleaseGroup,
    /// Its artists, in order, each with what joins it to the next.
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
    /// Whether the Cover Art Archive has art for it; none when MusicBrainz doesn't say.
    #[serde(rename = "cover-art-archive")]
    pub cover_art: Option<CoverArt>,
    /// Its discs (or other media), in order, with their tracks.
    #[serde(default)]
    pub media: Vec<Medium>,
}

/// A release group: an album, whichever edition.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReleaseGroup {
    /// Its MusicBrainz id.
    pub id: String,
    /// Its title; none when the answer leaves it out.
    pub title: Option<String>,
    /// `Album`, `Single`, `EP` and the like; none when MusicBrainz doesn't say.
    #[serde(rename = "primary-type")]
    pub primary_type: Option<String>,
    /// Left out, or null, when there are none.
    #[serde(rename = "secondary-types", default)]
    pub secondary_types: Option<Vec<String>>,
    /// Its earliest release's date, as MusicBrainz writes it.
    #[serde(rename = "first-release-date")]
    pub first_release_date: Option<String>,
}

/// One artist in a credit, as named there.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Credit {
    /// The name as credited, which can differ from the artist's own.
    pub name: String,
    /// What joins it to the next name (` & `, ` feat. `); empty for the last.
    #[serde(default)]
    pub joinphrase: String,
    /// The artist credited.
    pub artist: CreditedArtist,
}

/// An artist as a credit or relation names them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CreditedArtist {
    /// Their MusicBrainz id.
    pub id: String,
    /// Their name.
    pub name: String,
    /// The name they sort under (`Beatles, The`); none when the answer leaves it out.
    #[serde(rename = "sort-name")]
    pub sort_name: Option<String>,
}

/// What the Cover Art Archive has for a release.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CoverArt {
    /// Whether it has a front cover.
    #[serde(default)]
    pub front: bool,
}

/// One disc (or other medium) of a release.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Medium {
    /// Which disc it is, from 1.
    pub position: i64,
    /// Its tracks, in order.
    #[serde(default)]
    pub tracks: Vec<Track>,
}

/// One track of a release.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Track {
    /// The track's MusicBrainz id (not the recording's).
    pub id: String,
    /// Its place on its disc, from 1.
    pub position: i64,
    /// The number as printed (`A1` on a record); none when the answer leaves it out.
    pub number: Option<String>,
    /// Its title on this release.
    pub title: String,
    /// Its length in milliseconds; none when unknown.
    pub length: Option<i64>,
    /// Its artists as this release credits them, in order (a featured artist among them).
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<Credit>,
    /// The recording it is.
    pub recording: Recording,
}

/// A recording, with what it performs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Recording {
    /// The recording's MusicBrainz id.
    pub id: String,
    /// The works it performs, as a release lookup gives them.
    #[serde(default)]
    pub relations: Vec<Relation>,
}

/// A relation of a recording or work, as far as Melete reads them: a recording's performance of
/// a work, a work's composers, and the work a part (movement) is part of.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Relation {
    /// The relation's type: `performance`, `composer`, `parts` and so on.
    #[serde(rename = "type")]
    pub kind: String,
    /// `forward` or `backward`: for `parts`, backward points from a part to the whole.
    pub direction: Option<String>,
    /// Where a part sits in its whole, from 1.
    #[serde(rename = "ordering-key")]
    pub ordering_key: Option<i64>,
    /// The work it points to, for a work relation.
    pub work: Option<Work>,
    /// The artist it points to, for an artist relation such as `composer`.
    pub artist: Option<CreditedArtist>,
}

/// A work: a composition, which recordings perform.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Work {
    /// Its MusicBrainz id.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its composers, and the whole it's a part of, as relations.
    #[serde(default)]
    pub relations: Vec<Relation>,
}

/// What a recording performs: the work (the whole one, when the recording is one of its parts),
/// the part and its number, and the composers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Performed {
    /// The work performed: the whole one, when the recording is one of its parts.
    pub work_id: String,
    /// That work's title.
    pub work_title: String,
    /// The part's own name, without the work's title or its number; none for a whole work.
    pub movement: Option<String>,
    /// The part's number in its whole, from 1; none for a whole work or when unknown.
    pub movement_number: Option<i64>,
    /// The work's composers, as credited.
    pub composers: Vec<CreditedArtist>,
}

impl Recording {
    /// The first work it performs, if MusicBrainz knows one.
    pub fn performed(&self) -> Option<Performed> {
        let work = self
            .relations
            .iter()
            .find(|r| r.kind == "performance")
            .and_then(|r| r.work.as_ref())?;
        let composers = work
            .relations
            .iter()
            .filter(|r| r.kind == "composer")
            .filter_map(|r| r.artist.clone())
            .collect();
        // A part points back at the work it's part of.
        let whole = work
            .relations
            .iter()
            .find(|r| r.kind == "parts" && r.direction.as_deref() == Some("backward"));
        Some(
            match whole.and_then(|r| Some((r.work.as_ref()?, r.ordering_key))) {
                Some((whole, number)) => Performed {
                    work_id: whole.id.clone(),
                    work_title: whole.title.clone(),
                    movement: Some(part_name(&work.title, &whole.title, number)),
                    movement_number: number.filter(|n| *n > 0),
                    composers,
                },
                None => Performed {
                    work_id: work.id.clone(),
                    work_title: work.title.clone(),
                    composers,
                    ..Performed::default()
                },
            },
        )
    }
}

/// A part's own name: its title without the whole work's ("Symphony no. 5 in C minor, op. 67:
/// I. Allegro con brio" is "Allegro con brio"), and without its number when that leads it.
pub fn part_name(title: &str, whole: &str, number: Option<i64>) -> String {
    let rest = title
        .get(..whole.len())
        .filter(|head| head.eq_ignore_ascii_case(whole))
        .map_or(title, |_| &title[whole.len()..]);
    let rest = rest.trim_start_matches([':', ',', '-', '–', ' ']);
    let unnumbered = number.and_then(|n| {
        let (head, tail) = rest.split_once(". ")?;
        (head == roman(n) || head == n.to_string()).then_some(tail)
    });
    let name = unnumbered.unwrap_or(rest).trim();
    if name.is_empty() { title.trim() } else { name }.to_owned()
}

pub(super) fn roman(mut n: i64) -> String {
    const NUMERALS: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}
