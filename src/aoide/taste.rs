//! What a listener has been listening to, as numbers to sort by: the clients' taste ranking
//! (`TasteProfile`, `TasteRanking`) as the sidecar ported it. Arithmetic over the history, not a
//! recommender: every position it produces can be explained in a sentence. Held to the
//! clients' sixteen-row parity table.

use std::collections::{HashMap, HashSet};

/// How much of the score a familiar genre is worth.
pub const GENRE_WEIGHT: f64 = 0.45;
/// How much a familiar artist is worth: less than genre, on purpose.
pub const ARTIST_WEIGHT: f64 = 0.3;
/// How far a track's own finish rate can pull it either way.
pub const FINISH_WEIGHT: f64 = 0.3;
/// What having been heard lately costs: wider than the rest of the scale, so everything
/// heard lately sits below everything that hasn't been.
pub const RECENT_PENALTY: f64 = 1.4;
/// What being by an artist heard lately costs.
pub const SAME_ARTIST_PENALTY: f64 = 0.4;
/// Taste is built from finished plays this recent…
pub const WINDOW_MS: i64 = 90 * 24 * 60 * 60 * 1000;
/// …the latest this many of them.
pub const DEPTH: usize = 200;

/// One listen, as the clients recorded it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayEvent {
    /// The track's id.
    pub track: String,
    /// When it started, in milliseconds since the epoch.
    pub started_at: i64,
    /// When it ended, in milliseconds since the epoch; none while still playing or unreported.
    pub ended_at: Option<i64>,
    /// Whether it played to the end.
    pub completed: bool,
    /// Whether the listener skipped it.
    pub skipped: bool,
}

/// A track as taste sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryTrack {
    /// The track's id.
    pub id: String,
    /// The album artist, or the first artist, or "Unknown Artist": the clients' rule.
    pub artist: String,
    /// Its genres, as tagged.
    pub genres: Vec<String>,
}

/// How strongly each genre and artist figures in a listener's recent finished plays.
#[derive(Debug, Default)]
pub struct TasteProfile {
    /// Genre, lowercased, to how strongly it figures: 0 to 1, the strongest at 1.
    pub genres: HashMap<String, f64>,
    /// The same for artists.
    pub artists: HashMap<String, f64>,
}

/// What was heard recently enough that hearing it again would feel like a repeat, and whose
/// records those were (lowercased).
#[derive(Debug, Default)]
pub struct Recent {
    /// Tracks heard lately.
    pub tracks: HashSet<String>,
    /// Their artists, lowercased.
    pub artists: HashSet<String>,
}

/// Listens that reached a verdict, and how many reached the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FinishCounts {
    /// Listens that ended finished or skipped.
    pub starts: u32,
    /// Of those, how many played to the end.
    pub completed: u32,
}

impl FinishCounts {
    /// Below this many decided listens the rate is noise.
    const MINIMUM_SAMPLE: u32 = 3;

    fn rate(self) -> Option<f64> {
        (self.starts >= Self::MINIMUM_SAMPLE)
            .then(|| f64::from(self.completed) / f64::from(self.starts))
    }
}

/// The taste part of a score, before penalties: the best-matching genre, the artist, and the
/// finish bias (none for a track with too little history). About −0.3 to 1.05.
pub fn taste(
    artist: &str,
    genres: &[String],
    profile: &TasteProfile,
    finish: Option<FinishCounts>,
) -> f64 {
    let genre = genres
        .iter()
        .filter_map(|tag| profile.genres.get(&tag.to_lowercase()))
        .fold(
            0.0,
            |best: f64, &value| if value > best { value } else { best },
        );
    let artist = profile
        .artists
        .get(&artist.to_lowercase())
        .copied()
        .unwrap_or(0.0);
    let finish_bias = finish.and_then(FinishCounts::rate).unwrap_or(0.5) - 0.5;
    genre * GENRE_WEIGHT + artist * ARTIST_WEIGHT + finish_bias * 2.0 * FINISH_WEIGHT
}

/// Taste less the two penalties, in the clients' order of operations. The parity table's
/// rule; `/aoide/next` adds its own terms between the two, in its own order.
#[cfg(test)]
pub fn score(
    track: &LibraryTrack,
    profile: &TasteProfile,
    recent: &Recent,
    finish: Option<FinishCounts>,
) -> f64 {
    let mut score = taste(&track.artist, &track.genres, profile, finish);
    if recent.tracks.contains(&track.id) {
        score -= RECENT_PENALTY;
    }
    if recent.artists.contains(&track.artist.to_lowercase()) {
        score -= SAME_ARTIST_PENALTY;
    }
    score
}

/// The best `limit` by score, ties broken on id (ordinal) so the same history gives the
/// same list twice.
pub fn best<T>(mut scored: Vec<(T, f64)>, id: impl Fn(&T) -> &str, limit: usize) -> Vec<T> {
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| id(&a.0).cmp(id(&b.0))));
    scored
        .into_iter()
        .take(limit)
        .map(|(item, _)| item)
        .collect()
}

/// Genre and artist weights from the latest [`DEPTH`] finished plays since `since`, each
/// normalised against its strongest, and how many plays that was.
pub fn listened(
    history: &[PlayEvent],
    by_id: &HashMap<&str, &LibraryTrack>,
    since: i64,
) -> (HashMap<String, f64>, HashMap<String, f64>, usize) {
    let mut finished: Vec<&PlayEvent> = history
        .iter()
        .filter(|e| e.completed && e.started_at >= since)
        .collect();
    // Stable, as LINQ's OrderByDescending is: equal start times keep the log's order.
    finished.sort_by_key(|e| std::cmp::Reverse(e.started_at));
    finished.truncate(DEPTH);
    let mut genres: HashMap<String, f64> = HashMap::new();
    let mut artists: HashMap<String, f64> = HashMap::new();
    for play in &finished {
        let Some(track) = by_id.get(play.track.as_str()) else {
            continue;
        };
        if !track.artist.is_empty() {
            *artists.entry(track.artist.to_lowercase()).or_default() += 1.0;
        }
        for genre in track.genres.iter().filter(|g| !g.is_empty()) {
            *genres.entry(genre.to_lowercase()).or_default() += 1.0;
        }
    }
    (normalised(genres), normalised(artists), finished.len())
}

/// Against the strongest, not the total: "how much of a favourite is this".
fn normalised(counts: HashMap<String, f64>) -> HashMap<String, f64> {
    let top = counts.values().copied().fold(0.0, f64::max);
    if top <= 0.0 {
        return HashMap::new();
    }
    counts.into_iter().map(|(k, v)| (k, v / top)).collect()
}

#[cfg(test)]
mod tests {
    //! The clients' `TasteRankingParityTests`: sixteen candidates, sixteen scores, one order.

    use super::*;

    fn profile() -> TasteProfile {
        TasteProfile {
            genres: HashMap::from([("electronic".into(), 0.5), ("metal".into(), 1.0)]),
            artists: HashMap::from([("perturbator".into(), 0.4), ("sabaton".into(), 1.0)]),
        }
    }

    fn recent() -> Recent {
        Recent {
            tracks: HashSet::from(["heard-an-hour-ago".into()]),
            artists: HashSet::new(),
        }
    }

    fn finish(starts: u32, completed: u32) -> Option<FinishCounts> {
        Some(FinishCounts { starts, completed })
    }

    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static [&'static str],
        Option<FinishCounts>,
        f64,
    );

    fn rows() -> Vec<Row> {
        vec![
            (
                "a favourite genre is most of what there is to earn",
                "metal-stranger",
                "Nobody",
                &["Metal"],
                None,
                0.45,
            ),
            (
                "an unfamiliar genre earns nothing",
                "ambient-stranger",
                "Nobody",
                &["Ambient"],
                None,
                0.0,
            ),
            (
                "a genre matches however it was capitalised",
                "shouted-genre",
                "Nobody",
                &["METAL"],
                None,
                0.45,
            ),
            (
                "so does an artist",
                "shouted-artist",
                "SABATON",
                &[],
                None,
                0.3,
            ),
            (
                "four genres aren't four times as relevant",
                "four-times-electronic",
                "Nobody",
                &["Electronic", "Electronic", "Electronic", "Electronic"],
                None,
                0.225,
            ),
            (
                "the best match counts wherever it sits",
                "mixed-tags",
                "Nobody",
                &["Ambient", "Metal", "Electronic"],
                None,
                0.45,
            ),
            (
                "favourite everything, always finished, tops the scale",
                "favourite-everything",
                "Sabaton",
                &["Metal"],
                finish(10, 10),
                1.05,
            ),
            (
                "never finished sinks, but its genre still counts",
                "always-skipped",
                "Nobody",
                &["Metal"],
                finish(10, 0),
                0.15,
            ),
            (
                "one skip of one says nothing",
                "barely-judged",
                "Nobody",
                &["Metal"],
                finish(1, 0),
                0.45,
            ),
            (
                "two of two is under the sample floor",
                "two-of-two",
                "Nobody",
                &[],
                finish(2, 2),
                0.0,
            ),
            (
                "three is the first sample either app speaks from",
                "three-of-three",
                "Nobody",
                &[],
                finish(3, 3),
                0.3,
            ),
            (
                "it counts against exactly as far as for",
                "none-of-three",
                "Nobody",
                &[],
                finish(3, 0),
                -0.3,
            ),
            (
                "two thirds finished is a third of the way up",
                "two-of-three",
                "Nobody",
                &[],
                finish(3, 2),
                0.1,
            ),
            (
                "second favourites together lose to a favourite genre alone",
                "second-favourite",
                "Perturbator",
                &["Electronic"],
                None,
                0.345,
            ),
            (
                "the best song, heard an hour ago, falls below the worst that hasn't been",
                "heard-an-hour-ago",
                "Sabaton",
                &["Metal"],
                finish(10, 10),
                -0.35,
            ),
            (
                "nothing familiar and nothing measured is the middle",
                "nothing-known",
                "Nobody",
                &[],
                None,
                0.0,
            ),
        ]
    }

    const ORDER: [&str; 16] = [
        "favourite-everything",
        "barely-judged",
        "metal-stranger",
        "mixed-tags",
        "shouted-genre",
        "second-favourite",
        "shouted-artist",
        "three-of-three",
        "four-times-electronic",
        "always-skipped",
        "two-of-three",
        "ambient-stranger",
        "nothing-known",
        "two-of-two",
        "none-of-three",
        "heard-an-hour-ago",
    ];

    fn rounded(value: f64) -> f64 {
        (value * 1_000_000.0).round() / 1_000_000.0
    }

    fn scored() -> Vec<(LibraryTrack, f64)> {
        let (profile, recent) = (profile(), recent());
        rows()
            .into_iter()
            .map(|(_, id, artist, genres, finish, _)| {
                let track = LibraryTrack {
                    id: id.into(),
                    artist: artist.into(),
                    genres: genres.iter().map(|g| (*g).to_owned()).collect(),
                };
                let s = score(&track, &profile, &recent, finish);
                (track, s)
            })
            .collect()
    }

    #[test]
    fn scores_the_clients_table_identically() {
        let rows = rows();
        assert_eq!(rows.len(), 16);
        for (row, (_, score)) in rows.iter().zip(scored()) {
            assert_eq!(rounded(score), row.5, "{}: scored {score}", row.0);
        }
    }

    #[test]
    fn puts_the_whole_table_in_one_order_ties_broken_on_id() {
        let ids: Vec<String> = best(scored(), |t| &t.id, 16)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, ORDER);
        let five: Vec<String> = best(scored(), |t| &t.id, 5)
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(five, ORDER[..5]);
        assert!(best(scored(), |t| &t.id, 0).is_empty());
    }
}
