//! What plays next: from the whole library, by arithmetic anyone can read.
//! The sidecar's `NextRanker`, held to the clients' fixture. Not a recommender: five factors,
//! each a sentence, summed with weights. Records are chosen by taste, kinship, similarity and
//! arc — never by how well they mix, since a good record that can only be crossfaded is still
//! the better record — and in Auto DJ mode the chosen ones are then put in the order they mix
//! best, as a chain from what the first of them follows. Deterministic: ties break on id.

use std::collections::{HashMap, HashSet};

use super::planner::{self, Arrangement, Grid, MixRecord, MixScore};
use super::taste::{self, FinishCounts, LibraryTrack, PlayEvent, Recent, TasteProfile};

/// The last this many distinct tracks started count as heard lately…
const RECENT_DEPTH: usize = 40;
/// …and so do the artists of the last this many queued.
const QUEUE_ARTIST_DEPTH: usize = 3;
/// Kinship with the seed: the largest single term after taste, because what plays next is
/// mostly a record of the same kind.
const KINSHIP_WEIGHT: f64 = 0.6;
const SIMILARITY_WEIGHT: f64 = 0.4;
const ARC_WEIGHT: f64 = 0.2;
/// What a pair the planner refuses is still worth: it can be crossfaded.
pub const CROSSFADE_ONLY: f64 = 0.15;
/// Below this a measured tempo doesn't describe its track well enough to compare.
const MINIMUM_STABILITY: f64 = 0.5;

/// Everything measured about a track that the ranking reads.
#[derive(Debug, Default, Clone)]
pub struct Measured {
    /// Beats a minute and stability (None when too short to tell).
    pub tempo: Option<(f64, Option<f64>)>,
    /// Its beat grid; none when it hasn't been measured.
    pub grid: Option<Grid>,
    /// Its Camelot key, as `8A`; none when unknown.
    pub key: Option<String>,
    /// Its sections, phrases and singing; none when it hasn't been measured.
    pub arrangement: Option<Arrangement>,
}

/// What a device asks for: records to follow the seed.
pub struct Request {
    /// The track playing now, which the list follows.
    pub seed: String,
    /// What's queued after the seed, in order.
    pub queue: Vec<String>,
    /// What this device heard lately, newest first.
    pub recent: Vec<String>,
    /// Whether to order the chosen records as a chain that mixes best, not by score.
    pub auto_dj: bool,
    /// How many records to return at most.
    pub limit: usize,
}

/// The five factors behind a candidate's score, each 0 to 1, as the apps show them.
#[derive(Debug, Clone, PartialEq)]
pub struct Factors {
    /// How well it fits the listener's taste, clamped to 0…1.
    pub taste: f64,
    /// 1 when neither it nor its artist was heard lately, 0.5 when only its artist was, 0 when it
    /// was.
    pub freshness: f64,
    /// Whether it's the seed's kind by genre: 1 for the same first genre, 0.5 for another shared
    /// one or when either is untagged, 0 when they share none.
    pub kinship: f64,
    /// How close its tempo, key and energy are to the seed's: the mean of those that can be
    /// compared, an unknown key counting 0.5.
    pub similarity: f64,
    /// From the record before it in the order, in Auto DJ only; None outside it or unread.
    pub mixability: Option<f64>,
    /// How near it lands to the energy the set wants next; 0.5 with nothing known.
    pub arc: f64,
}

/// One record that could play next.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The track's id.
    pub id: String,
    /// Its score: taste, plus the weighted kinship, similarity and arc, less the penalties for
    /// being heard lately. Unclamped.
    pub score: f64,
    /// What the score is made of.
    pub factors: Factors,
}

/// What follows the seed, and what taste stood on.
pub struct Ranked {
    /// The records, best first, or in mixing order in Auto DJ.
    pub candidates: Vec<Candidate>,
    /// Finished plays the taste profile was built from.
    pub profile_events: usize,
    /// Where the taste window began, in milliseconds since the epoch.
    pub profile_since: i64,
}

/// How well one record follows another: the planner's score, the crossfade figure when it
/// refuses, None when either is unread.
pub trait Mixability {
    fn score(&self, from: &str, to: &str) -> Option<f64>;
}

/// The planner's opinion of each pair, over what's measured.
pub struct PlannerMixability<'a>(pub &'a HashMap<String, Measured>);

impl Mixability for PlannerMixability<'_> {
    fn score(&self, from: &str, to: &str) -> Option<f64> {
        let (outgoing, incoming) = (self.0.get(from)?, self.0.get(to)?);
        let (outgoing, incoming) = (mix_record(outgoing)?, mix_record(incoming)?);
        Some(planner::plan(outgoing, incoming, 0.0).map_or(CROSSFADE_ONLY, |p| p.score.total()))
    }
}

/// A record the planner can read: with a grid and an arrangement.
pub(crate) fn mix_record(measured: &Measured) -> Option<MixRecord<'_>> {
    Some(MixRecord {
        grid: measured.grid.as_ref()?,
        key: measured.key.as_deref(),
        arrangement: Some(measured.arrangement.as_ref()?),
    })
}

/// The library ranked for what should follow the seed.
pub fn rank(
    request: &Request,
    library: &[LibraryTrack],
    history: &[PlayEvent],
    not_interested: &HashSet<String>,
    measured: &HashMap<String, Measured>,
    mixability: &dyn Mixability,
    now_ms: i64,
) -> Ranked {
    let mut by_id: HashMap<&str, &LibraryTrack> = HashMap::new();
    for track in library {
        by_id.entry(track.id.as_str()).or_insert(track);
    }
    let since = now_ms - taste::WINDOW_MS;
    let (genres, artists, events) = taste::listened(history, &by_id, since);
    let profile = TasteProfile { genres, artists };
    let recent = recent(history, &by_id, request);
    let finish = finish_counts(history);
    let seed = by_id.get(request.seed.as_str()).copied();
    let nothing = Measured::default();
    let seed_measured = measured.get(&request.seed).unwrap_or(&nothing);
    let target = arc_target(request, measured);
    let mut excluded: HashSet<&str> = HashSet::from([request.seed.as_str()]);
    excluded.extend(request.queue.iter().map(String::as_str));
    excluded.extend(request.recent.iter().map(String::as_str));
    let scored: Vec<(Candidate, f64)> = library
        .iter()
        .filter(|t| !excluded.contains(t.id.as_str()) && !not_interested.contains(&t.id))
        .map(|track| {
            let taste = taste::taste(
                &track.artist,
                &track.genres,
                &profile,
                finish.get(track.id.as_str()).copied(),
            );
            let heard_lately = recent.tracks.contains(&track.id);
            let artist_heard_lately = recent.artists.contains(&track.artist.to_lowercase());
            let candidate_measured = measured.get(&track.id).unwrap_or(&nothing);
            let kinship = kinship(seed, track);
            let similarity = similarity(seed_measured, candidate_measured);
            let arc = arc(target, candidate_measured);
            let score = taste
                + kinship * KINSHIP_WEIGHT
                + similarity * SIMILARITY_WEIGHT
                + arc * ARC_WEIGHT
                - if heard_lately {
                    taste::RECENT_PENALTY
                } else {
                    0.0
                }
                - if artist_heard_lately {
                    taste::SAME_ARTIST_PENALTY
                } else {
                    0.0
                };
            let factors = Factors {
                taste: taste.clamp(0.0, 1.0),
                freshness: if heard_lately {
                    0.0
                } else if artist_heard_lately {
                    0.5
                } else {
                    1.0
                },
                kinship,
                similarity,
                mixability: None,
                arc,
            };
            let id = track.id.clone();
            (Candidate { id, score, factors }, score)
        })
        .collect();
    let chosen = taste::best(scored, |c| &c.id, request.limit);
    let candidates = if request.auto_dj {
        chain(request, chosen, mixability)
    } else {
        chosen
    };
    Ranked {
        candidates,
        profile_events: events,
        profile_since: since,
    }
}

/// The chosen records in the order they should play: from what the first follows, each next
/// is the one that mixes best out of the last. A refused or unread pair counts the crossfade
/// figure; ties go to the better-chosen record.
fn chain(request: &Request, chosen: Vec<Candidate>, mixability: &dyn Mixability) -> Vec<Candidate> {
    let mut remaining: Vec<(usize, Candidate)> = chosen.into_iter().enumerate().collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    let mut last = request.queue.last().unwrap_or(&request.seed).clone();
    while !remaining.is_empty() {
        let mut best: Option<(usize, f64, Option<f64>)> = None;
        for (at, (rank, candidate)) in remaining.iter().enumerate() {
            let reported = mixability.score(&last, &candidate.id);
            let counts = reported.unwrap_or(CROSSFADE_ONLY);
            let better = match best {
                None => true,
                Some((b, best_counts, _)) => {
                    counts > best_counts || (counts == best_counts && *rank < remaining[b].0)
                }
            };
            if better {
                best = Some((at, counts, reported));
            }
        }
        let (at, _, reported) = best.expect(
            "the loop only runs while remaining has a candidate, and the first one sets best",
        );
        let (_, mut next) = remaining.remove(at);
        next.factors.mixability = reported;
        last.clone_from(&next.id);
        ordered.push(next);
    }
    ordered
}

/// Heard lately: the request's own list (sync lags by minutes; a set is judged in seconds)
/// and the last [`RECENT_DEPTH`] distinct tracks started on any device, finished or not; and
/// their artists, with the artists of the last [`QUEUE_ARTIST_DEPTH`] queued.
fn recent(
    history: &[PlayEvent],
    by_id: &HashMap<&str, &LibraryTrack>,
    request: &Request,
) -> Recent {
    let mut latest: Vec<(&str, i64)> = Vec::new();
    let mut at: HashMap<&str, usize> = HashMap::new();
    for play in history {
        match at.get(play.track.as_str()) {
            Some(&i) => latest[i].1 = latest[i].1.max(play.started_at),
            None => {
                at.insert(&play.track, latest.len());
                latest.push((&play.track, play.started_at));
            }
        }
    }
    // Stable, as LINQ's ordering is: equal times keep the order first heard.
    latest.sort_by_key(|&(_, started)| std::cmp::Reverse(started));
    let mut tracks: HashSet<String> = request.recent.iter().cloned().collect();
    tracks.extend(
        latest
            .iter()
            .take(RECENT_DEPTH)
            .map(|(id, _)| (*id).to_owned()),
    );
    let queued = &request.queue[request.queue.len().saturating_sub(QUEUE_ARTIST_DEPTH)..];
    let artists = tracks
        .iter()
        .chain(queued)
        .filter_map(|id| by_id.get(id.as_str()))
        .filter(|t| !t.artist.is_empty())
        .map(|t| t.artist.to_lowercase())
        .collect();
    Recent { tracks, artists }
}

/// Listens that reached a verdict, per track, over all time.
fn finish_counts(history: &[PlayEvent]) -> HashMap<&str, FinishCounts> {
    let mut counts: HashMap<&str, FinishCounts> = HashMap::new();
    for play in history.iter().filter(|p| p.completed || p.skipped) {
        let entry = counts.entry(&play.track).or_insert(FinishCounts {
            starts: 0,
            completed: 0,
        });
        entry.starts += 1;
        entry.completed += u32::from(play.completed);
    }
    counts
}

/// Whether the candidate is the seed's kind: 1 for the same first genre, 0.5 for any other
/// shared genre, 0 when both are tagged and share none, 0.5 when either is untagged.
fn kinship(seed: Option<&LibraryTrack>, candidate: &LibraryTrack) -> f64 {
    let Some(seed) = seed.filter(|s| !s.genres.is_empty()) else {
        return 0.5;
    };
    if candidate.genres.is_empty() {
        return 0.5;
    }
    if seed.genres[0].to_lowercase() == candidate.genres[0].to_lowercase() {
        return 1.0;
    }
    let theirs: HashSet<String> = candidate.genres.iter().map(|g| g.to_lowercase()).collect();
    if seed
        .genres
        .iter()
        .any(|g| theirs.contains(&g.to_lowercase()))
    {
        0.5
    } else {
        0.0
    }
}

/// A track's energy weighted by section length; None without an arrangement.
fn mean_energy(arrangement: Option<&Arrangement>) -> Option<f64> {
    let sections = &arrangement.filter(|a| !a.sections.is_empty())?.sections;
    let (mut weighted, mut length) = (0.0, 0.0);
    for section in sections {
        let span = (section.end_ms - section.start_ms).max(0.0);
        weighted += section.energy * span;
        length += span;
    }
    Some(if length > 0.0 {
        weighted / length
    } else {
        sections.iter().map(|s| s.energy).sum::<f64>() / sections.len() as f64
    })
}

/// Tempo, key and energy against the seed: the mean of the parts that can be answered.
fn similarity(seed: &Measured, candidate: &Measured) -> f64 {
    let holds = |stability: Option<f64>| stability.is_none_or(|s| s >= MINIMUM_STABILITY);
    let mut parts = Vec::with_capacity(3);
    if let (Some((seed_bpm, seed_stability)), Some((bpm, stability))) =
        (seed.tempo, candidate.tempo)
        && holds(seed_stability)
        && holds(stability)
        && seed_bpm > 0.0
        && bpm > 0.0
    {
        let mut ratio = bpm / seed_bpm;
        while ratio > 1.4 {
            ratio /= 2.0;
        }
        while ratio < 0.7 {
            ratio *= 2.0;
        }
        parts.push(MixScore::tempo_factor(ratio));
    }
    parts.push(MixScore::key_factor(
        seed.key.as_deref(),
        candidate.key.as_deref(),
    ));
    if let (Some(a), Some(b)) = (
        mean_energy(seed.arrangement.as_ref()),
        mean_energy(candidate.arrangement.as_ref()),
    ) {
        parts.push(1.0 - (a - b).abs());
    }
    parts.iter().sum::<f64>() / parts.len() as f64
}

/// The energy the set wants next, from the last four records it has been through (recent
/// oldest first, the seed, the queue): after three rises a breather, after two falls a
/// return, otherwise hold.
fn arc_target(request: &Request, measured: &HashMap<String, Measured>) -> Option<f64> {
    let sequence: Vec<&String> = request
        .recent
        .iter()
        .rev()
        .chain(std::iter::once(&request.seed))
        .chain(&request.queue)
        .collect();
    let energies: Vec<f64> = sequence[sequence.len().saturating_sub(4)..]
        .iter()
        .filter_map(|id| mean_energy(measured.get(*id)?.arrangement.as_ref()))
        .collect();
    let reference = *energies.last()?;
    let steps: Vec<f64> = energies.windows(2).map(|w| w[1] - w[0]).collect();
    if steps.len() >= 3 && steps[steps.len() - 3..].iter().all(|&s| s > 0.0) {
        return Some(reference - 0.25);
    }
    if steps.len() >= 2 && steps[steps.len() - 2..].iter().all(|&s| s < 0.0) {
        return Some(reference + 0.2);
    }
    Some(reference)
}

/// How near the candidate lands to what the set wants; 0.5 with nothing known.
fn arc(target: Option<f64>, candidate: &Measured) -> f64 {
    match (target, mean_energy(candidate.arrangement.as_ref())) {
        (Some(wanted), Some(energy)) => (1.0 - (energy - wanted).abs()).clamp(0.0, 1.0),
        _ => 0.5,
    }
}

#[cfg(test)]
mod tests {
    //! The clients' `next-ranking-fixture.json`, byte for byte: twenty records, sixteen
    //! listens on a fixed clock, one flag, a table of the planner's pair scores, and eight
    //! requests with every candidate's score and factors to six decimals, in order.

    use serde_json::Value;

    use super::planner::Section;
    use super::*;

    struct Table(HashMap<String, f64>);

    impl Mixability for Table {
        fn score(&self, from: &str, to: &str) -> Option<f64> {
            self.0.get(&format!("{from}>{to}")).copied()
        }
    }

    fn rounded(value: f64) -> f64 {
        (value * 1_000_000.0).round() / 1_000_000.0
    }

    #[test]
    fn ranks_the_clients_fixture_identically() {
        let root: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/aoide/next-ranking-fixture.json"
        ))
        .unwrap();
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_owned())
                .collect()
        };
        let mut library = Vec::new();
        let mut measured = HashMap::new();
        for record in root["library"].as_array().unwrap() {
            let id = record["id"].as_str().unwrap().to_owned();
            library.push(LibraryTrack {
                id: id.clone(),
                artist: record["artist"].as_str().unwrap().to_owned(),
                genres: record
                    .get("genres")
                    .filter(|g| g.is_array())
                    .map(strings)
                    .unwrap_or_default(),
            });
            let arrangement = record["sections"].as_array().map(|sections| Arrangement {
                sections: sections
                    .iter()
                    .map(|s| Section {
                        start_ms: s["startMs"].as_f64().unwrap(),
                        end_ms: s["endMs"].as_f64().unwrap(),
                        kind: "unknown".into(),
                        energy: s["energy"].as_f64().unwrap(),
                    })
                    .collect(),
                phrase_bars: None,
                phrase_anchor_ms: None,
                vocals: None,
            });
            measured.insert(
                id,
                Measured {
                    tempo: record["bpm"]
                        .as_f64()
                        .map(|bpm| (bpm, record["bpmStability"].as_f64())),
                    grid: None,
                    key: record["key"].as_str().map(str::to_owned),
                    arrangement,
                },
            );
        }
        let plays: Vec<PlayEvent> = root["plays"]
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
        let not_interested: HashSet<String> = strings(&root["notInterested"]).into_iter().collect();
        let table = Table(
            root["mixability"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_f64().unwrap()))
                .collect(),
        );
        let cases = root["cases"].as_array().unwrap();
        assert_eq!((cases.len(), library.len(), plays.len()), (8, 20, 16));
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let request = Request {
                seed: case["seed"].as_str().unwrap().to_owned(),
                queue: strings(&case["queue"]),
                recent: strings(&case["recent"]),
                auto_dj: case["mode"] == "autodj",
                limit: case["limit"].as_u64().unwrap() as usize,
            };
            let ranked = rank(
                &request,
                &library,
                &plays,
                &not_interested,
                &measured,
                &table,
                root["now"].as_i64().unwrap(),
            );
            let expected = &case["expected"];
            assert_eq!(
                ranked.profile_events as u64,
                expected["events"].as_u64().unwrap(),
                "{name}"
            );
            let want = expected["candidates"].as_array().unwrap();
            let got_ids: Vec<&str> = ranked.candidates.iter().map(|c| c.id.as_str()).collect();
            let want_ids: Vec<&str> = want.iter().map(|w| w["id"].as_str().unwrap()).collect();
            assert_eq!(got_ids, want_ids, "{name}");
            for (got, want) in ranked.candidates.iter().zip(want) {
                let f = &want["factors"];
                let id = &got.id;
                assert_eq!(
                    rounded(got.score),
                    rounded(want["score"].as_f64().unwrap()),
                    "{name}: {id} score"
                );
                for (factor, value) in [
                    ("taste", got.factors.taste),
                    ("freshness", got.factors.freshness),
                    ("kinship", got.factors.kinship),
                    ("similarity", got.factors.similarity),
                    ("arc", got.factors.arc),
                ] {
                    assert_eq!(
                        rounded(value),
                        rounded(f[factor].as_f64().unwrap()),
                        "{name}: {id} {factor}"
                    );
                }
                assert_eq!(
                    got.factors.mixability.map(rounded),
                    f["mixability"].as_f64().map(rounded),
                    "{name}: {id} mixability"
                );
            }
        }
    }
}
