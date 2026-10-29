//! Discover: the records a listener owns and hasn't heard through, ranked by
//! their taste, the same all week and on every device. The sidecar's `DiscoverRanker`, held to
//! the clients' fixture. It reads the week as it began — plays from before Monday 00:00 UTC,
//! for the pool and for taste — so a record finished on Tuesday stays until the week turns;
//! flags are read as they are now, so a record marked not interested leaves at once.

use std::collections::{HashMap, HashSet};

use super::taste::{self, LibraryTrack, PlayEvent, TasteProfile};

/// A record started this many times and never finished has been answered.
const ANSWERED_STARTS: u32 = 3;
/// At most this many by one track artist…
const PER_ARTIST: u32 = 2;
/// …and this many from one album.
const PER_ALBUM: u32 = 3;
const DAY_MS: i64 = 86_400_000;

/// A track as Discover sees it.
pub struct DiscoverTrack {
    /// Id, album artist (the taste rule) and genres.
    pub track: LibraryTrack,
    /// The track's own first artist, for the per-artist cap: a compilation is capped as an
    /// album, not as one artist called "Various Artists".
    pub track_artist: String,
    /// The album's title, for the per-album cap; none caps nothing.
    pub album: Option<String>,
}

/// One record on the week's list.
#[derive(Debug, PartialEq)]
pub struct Candidate {
    /// The track's id.
    pub id: String,
    /// How well it fits the listener's taste, unclamped.
    pub score: f64,
    /// The score clamped to 0…1, as reported.
    pub taste: f64,
}

/// A listener's Discover list for one week.
#[derive(Debug, PartialEq)]
pub struct Discovered {
    /// The ISO 8601 week it's for, as `2026-W43`.
    pub week: String,
    /// The records, best first, at most the limit asked for.
    pub candidates: Vec<Candidate>,
    /// Finished plays taste stood on, and from when.
    pub profile_events: usize,
    /// Where the taste window began, in milliseconds since 1970.
    pub profile_since: i64,
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil year of a day since 1970-01-01.
fn year_of(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    yoe + era * 400 + i64::from(month <= 2)
}

/// The ISO 8601 week holding `now_ms`, in UTC, as `2026-W43`, and its Monday 00:00 UTC.
pub fn week(now_ms: i64) -> (String, i64) {
    let day = now_ms.div_euclid(DAY_MS);
    // 1970-01-01 was a Thursday.
    let monday = day - (day + 3).rem_euclid(7);
    // A week belongs to the year its Thursday is in.
    let thursday = monday + 3;
    let year = year_of(thursday);
    let number = (thursday - days_from_civil(year, 1, 1)) / 7 + 1;
    (format!("{year:04}-W{number:02}"), monday * DAY_MS)
}

/// A track's place in the week's shuffle: the lowercase hex SHA-256 of `week:user:track`,
/// compared as a string. Different every week, the same on every device.
pub fn shuffle(week: &str, user: &str, track: &str) -> String {
    super::library::sha256_hex(format!("{week}:{user}:{track}").as_bytes())
}

/// The week's list for `user`, at most `limit` long.
pub fn rank(
    user: &str,
    library: &[DiscoverTrack],
    history: &[PlayEvent],
    not_interested: &HashSet<String>,
    limit: usize,
    now_ms: i64,
) -> Discovered {
    let (week, week_start) = week(now_ms);
    let before: Vec<PlayEvent> = history
        .iter()
        .filter(|e| e.started_at < week_start)
        .cloned()
        .collect();
    let mut by_id: HashMap<&str, &LibraryTrack> = HashMap::new();
    for entry in library {
        by_id.entry(entry.track.id.as_str()).or_insert(&entry.track);
    }
    let since = week_start - taste::WINDOW_MS;
    let (genres, artists, events) = taste::listened(&before, &by_id, since);
    let profile = TasteProfile { genres, artists };
    let mut finished = HashSet::new();
    let mut starts: HashMap<&str, u32> = HashMap::new();
    for play in &before {
        if play.completed {
            finished.insert(play.track.as_str());
        }
        *starts.entry(play.track.as_str()).or_default() += 1;
    }
    let mut scored: Vec<(&DiscoverTrack, f64, String)> = library
        .iter()
        .filter(|entry| {
            let id = entry.track.id.as_str();
            !not_interested.contains(id)
                && !finished.contains(id)
                && starts.get(id).copied().unwrap_or(0) < ANSWERED_STARTS
        })
        .map(|entry| {
            // No finish bias: nothing in the pool has three decided listens.
            let score = taste::taste(&entry.track.artist, &entry.track.genres, &profile, None);
            (entry, score, shuffle(&week, user, &entry.track.id))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.0.track.id.cmp(&b.0.track.id))
    });
    let mut per_artist: HashMap<String, u32> = HashMap::new();
    let mut per_album: HashMap<&str, u32> = HashMap::new();
    let mut candidates = Vec::new();
    for (entry, score, _) in scored {
        if candidates.len() >= limit {
            break;
        }
        let artist = entry.track_artist.to_lowercase();
        if per_artist.get(&artist).copied().unwrap_or(0) >= PER_ARTIST
            || entry
                .album
                .as_deref()
                .is_some_and(|album| per_album.get(album).copied().unwrap_or(0) >= PER_ALBUM)
        {
            continue;
        }
        *per_artist.entry(artist).or_default() += 1;
        if let Some(album) = entry.album.as_deref() {
            *per_album.entry(album).or_default() += 1;
        }
        candidates.push(Candidate {
            id: entry.track.id.clone(),
            score,
            taste: score.clamp(0.0, 1.0),
        });
    }
    Discovered {
        week,
        candidates,
        profile_events: events,
        profile_since: since,
    }
}

#[cfg(test)]
mod tests {
    //! The clients' `discover-fixture.json`: 22 records, plays either side of a Monday, one
    //! flag, five cases to six decimals, and ISO weeks across a year's turn.

    use serde_json::Value;

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/aoide/discover-fixture.json"
        ))
        .unwrap()
    }

    fn rounded(value: f64) -> f64 {
        (value * 1_000_000.0).round() / 1_000_000.0
    }

    #[test]
    fn ranks_the_fixture_identically() {
        let root = fixture();
        let library: Vec<DiscoverTrack> = root["library"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                let artist = t["artist"].as_str().unwrap().to_owned();
                DiscoverTrack {
                    track_artist: t["trackArtist"]
                        .as_str()
                        .map_or_else(|| artist.clone(), str::to_owned),
                    album: t["album"].as_str().map(str::to_owned),
                    track: LibraryTrack {
                        id: t["id"].as_str().unwrap().to_owned(),
                        artist,
                        genres: t["genres"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|g| g.as_str().unwrap().to_owned())
                            .collect(),
                    },
                }
            })
            .collect();
        let cases = root["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 5);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let plays: Vec<PlayEvent> = case
                .get("plays")
                .unwrap_or(&root["plays"])
                .as_array()
                .unwrap()
                .iter()
                .map(|p| PlayEvent {
                    track: p["id"].as_str().unwrap().to_owned(),
                    started_at: p["startedAt"].as_i64().unwrap(),
                    ended_at: None,
                    completed: p["completed"].as_bool().unwrap(),
                    skipped: p["skipped"].as_bool().unwrap(),
                })
                .collect();
            let flags: HashSet<String> = case
                .get("notInterested")
                .unwrap_or(&root["notInterested"])
                .as_array()
                .unwrap()
                .iter()
                .map(|f| f.as_str().unwrap().to_owned())
                .collect();
            let result = rank(
                root["userId"].as_str().unwrap(),
                &library,
                &plays,
                &flags,
                case["limit"].as_u64().unwrap() as usize,
                case["now"].as_i64().unwrap(),
            );
            let expected = &case["expected"];
            assert_eq!(result.week, expected["week"].as_str().unwrap(), "{name}");
            assert_eq!(
                result.profile_events as u64,
                expected["profile"]["events"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                result.profile_since,
                expected["profile"]["since"].as_i64().unwrap(),
                "{name}"
            );
            let want = expected["candidates"].as_array().unwrap();
            let ids: Vec<&str> = result.candidates.iter().map(|c| c.id.as_str()).collect();
            let want_ids: Vec<&str> = want.iter().map(|w| w["id"].as_str().unwrap()).collect();
            assert_eq!(ids, want_ids, "{name}");
            for (got, want) in result.candidates.iter().zip(want) {
                assert_eq!(
                    rounded(got.score),
                    rounded(want["score"].as_f64().unwrap()),
                    "{name}: {}",
                    got.id
                );
                assert_eq!(
                    rounded(got.taste),
                    rounded(want["factors"]["taste"].as_f64().unwrap()),
                    "{name}: {}",
                    got.id
                );
            }
        }
    }

    #[test]
    fn names_the_iso_week_in_utc() {
        let root = fixture();
        for row in root["weeks"].as_array().unwrap() {
            let (name, start) = week(row["now"].as_i64().unwrap());
            assert_eq!(name, row["week"].as_str().unwrap(), "{}", row["at"]);
            assert_eq!(start, row["startMs"].as_i64().unwrap(), "{}", row["at"]);
        }
    }
}
