//! Ambient mixes: Chill, Focus, Sleep and Energy, chosen from the library by how its
//! tracks sound, as the audio analysis measured them: integrated loudness, tempo, and how
//! much of a track has singing. Rules a listener can follow, not a recommender: each mix is a
//! set of ranges, a track fits it the more squarely it sits inside them, and the listener's
//! taste (as Discover reads it) and a shuffle that changes daily decide among those that fit.
//! Endless: an app asks for the next tracks as the mix plays, saying what it has had.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

/// How a track sounds, as far as it was measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// Integrated loudness (EBU R128), in LUFS.
    pub loudness: f64,
    /// Beats per minute, when the tempo was measured with confidence.
    pub bpm: Option<f64>,
    /// How much of the track has someone singing, 0 to 1, when that could be told.
    pub vocals: Option<f64>,
}

/// A range a mix wants a measure in; `required` leaves out a track whose measure is unknown.
#[derive(Debug, Clone, Copy)]
pub struct Want {
    /// The lowest value that fits.
    pub low: f64,
    /// The highest value that fits.
    pub high: f64,
    /// Whether a track with this measure unknown is left out; otherwise unknown counts as half a
    /// fit.
    pub required: bool,
}

const fn want(low: f64, high: f64, required: bool) -> Want {
    Want {
        low,
        high,
        required,
    }
}

/// One ambient mix.
#[derive(Debug, Clone, Copy)]
pub struct Mix {
    /// Its id in the API: `chill`, `focus`, `sleep` or `energy`.
    pub key: &'static str,
    /// Its name, as apps show it.
    pub name: &'static str,
    /// One line on what it is, as apps show it.
    pub about: &'static str,
    /// The integrated loudness it wants, in LUFS.
    pub loudness: Want,
    /// The tempo it wants, in beats a minute; half or double a track's tempo counts too.
    pub bpm: Want,
    /// How much of a track may have singing, 0 to 1.
    pub vocals: Want,
}

/// Every ambient mix, in the order apps show them.
pub const MIXES: [Mix; 4] = [
    Mix {
        key: "chill",
        name: "Chill",
        about: "Unhurried and not too loud.",
        loudness: want(-30.0, -9.0, true),
        bpm: want(60.0, 110.0, false),
        vocals: want(0.0, 1.0, false),
    },
    Mix {
        key: "focus",
        name: "Focus",
        about: "Mostly without singing, at an even pace.",
        loudness: want(-30.0, -8.0, true),
        bpm: want(70.0, 130.0, false),
        vocals: want(0.0, 0.2, true),
    },
    Mix {
        key: "sleep",
        name: "Sleep",
        about: "Quiet and slow, with little singing.",
        loudness: want(-40.0, -16.0, true),
        bpm: want(40.0, 90.0, false),
        vocals: want(0.0, 0.35, false),
    },
    Mix {
        key: "energy",
        name: "Energy",
        about: "Fast and loud, to move to.",
        loudness: want(-11.0, 0.0, true),
        bpm: want(115.0, 175.0, true),
        vocals: want(0.0, 1.0, false),
    },
];

/// The mix with this id; none when there's no such mix.
pub fn mix(key: &str) -> Option<&'static Mix> {
    MIXES.iter().find(|m| m.key == key)
}

/// How squarely a measure sits in a range: 1 at its middle, falling to 0.5 at its edges, 0
/// outside; `None` when it's outside, or unknown and required.
fn closeness(value: Option<f64>, want: Want) -> Option<f64> {
    let Some(value) = value else {
        // Unknown and not required: neither for nor against.
        return (!want.required).then_some(0.5);
    };
    if value < want.low || value > want.high {
        return None;
    }
    let middle = (want.low + want.high) / 2.0;
    let half = (want.high - want.low) / 2.0;
    if half <= 0.0 {
        return Some(1.0);
    }
    Some(1.0 - 0.5 * ((value - middle).abs() / half))
}

/// How well a track fits a mix, 0.5 to 1; `None` when it doesn't. A tempo read at half or
/// double its real speed is common, so a halved or doubled tempo that fits counts too.
pub fn fit(sound: &Sound, mix: &Mix) -> Option<f64> {
    let loudness = closeness(Some(sound.loudness), mix.loudness)?;
    let bpm = match sound.bpm {
        Some(bpm) => [bpm, bpm / 2.0, bpm * 2.0]
            .into_iter()
            .filter_map(|b| closeness(Some(b), mix.bpm))
            .fold(None, |best: Option<f64>, c| {
                Some(best.map_or(c, |b| b.max(c)))
            })?,
        None => closeness(None, mix.bpm)?,
    };
    let vocals = closeness(sound.vocals, mix.vocals)?;
    Some((loudness + bpm + vocals) / 3.0)
}

/// A track as a mix sees it.
pub struct Candidate {
    /// The track's id.
    pub id: i64,
    /// How it sounds.
    pub sound: Sound,
    /// The listener's taste for it, as Discover scores it (about −0.3 to 1.05).
    pub taste: f64,
    /// Its album and first artist, so one batch doesn't lean on either.
    pub album: Option<i64>,
    /// Its first artist, for the per-artist cap.
    pub artist: String,
}

/// A number from 0 to 1 that's the same for a user, day and track, and different otherwise:
/// the mix's shuffle.
fn shuffle(user: i64, day: i64, track: i64) -> f64 {
    let digest = Sha256::digest(format!("{user}:{day}:{track}"));
    f64::from(u32::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3],
    ])) / f64::from(u32::MAX)
}

/// How much fitting the mix counts against taste and the shuffle.
const FIT_WEIGHT: f64 = 0.6;
const TASTE_WEIGHT: f64 = 0.4;
const SHUFFLE_WEIGHT: f64 = 0.25;
/// What having heard a track in the last day costs.
const HEARD_PENALTY: f64 = 1.0;
/// At most this many from one album, and by one artist, in one batch.
const PER_ALBUM: usize = 2;
const PER_ARTIST: usize = 2;
/// When everything that fits has been had, the oldest come round again, but never these
/// last few.
const KEEP_FRESH: usize = 50;

/// Who a mix is for: the user, today (days since 1970, for the shuffle), what they heard in
/// the last day and what they flagged not interested.
#[derive(Clone, Copy)]
pub struct Listener<'a> {
    /// The listener's user id.
    pub user: i64,
    /// Today, in days since 1970: the shuffle changes with it.
    pub day: i64,
    /// Tracks they heard in the last day, which are put behind the rest.
    pub heard_lately: &'a HashSet<i64>,
    /// Tracks they flagged not interested, which are left out.
    pub flagged: &'a HashSet<i64>,
}

/// The next `limit` tracks of a mix: those that fit, best first, leaving out the ones the app
/// has had (`queue`, oldest first) and those flagged not interested. When too few are left,
/// what was had longest ago comes round again, so the mix never runs dry.
pub fn next(
    mix: &Mix,
    candidates: &[Candidate],
    queue: &[i64],
    listener: &Listener,
    limit: usize,
) -> Vec<i64> {
    let Listener {
        user,
        day,
        heard_lately,
        flagged,
    } = *listener;
    let mut scored: Vec<(&Candidate, f64)> = candidates
        .iter()
        .filter(|c| !flagged.contains(&c.id))
        .filter_map(|c| {
            let fit = fit(&c.sound, mix)?;
            let mut score = FIT_WEIGHT * fit
                + TASTE_WEIGHT * c.taste.clamp(0.0, 1.0)
                + SHUFFLE_WEIGHT * shuffle(user, day, c.id);
            if heard_lately.contains(&c.id) {
                score -= HEARD_PENALTY;
            }
            Some((c, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
    let had: HashSet<i64> = queue.iter().copied().collect();
    let fresh: HashSet<i64> = queue.iter().rev().take(KEEP_FRESH).copied().collect();
    // What was had, by how long ago: the oldest first.
    let age: HashMap<i64, usize> = queue.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut picked = Vec::new();
    let mut albums: HashMap<i64, usize> = HashMap::new();
    let mut artists: HashMap<String, usize> = HashMap::new();
    let mut take = |c: &Candidate, picked: &mut Vec<i64>, capped: bool| {
        if picked.contains(&c.id) || picked.len() >= limit {
            return;
        }
        let album = c
            .album
            .map(|a| albums.get(&a).copied().unwrap_or(0))
            .unwrap_or(0);
        let artist = artists.get(&c.artist.to_lowercase()).copied().unwrap_or(0);
        if capped && (album >= PER_ALBUM || artist >= PER_ARTIST) {
            return;
        }
        if let Some(a) = c.album {
            *albums.entry(a).or_default() += 1;
        }
        *artists.entry(c.artist.to_lowercase()).or_default() += 1;
        picked.push(c.id);
    };
    for (c, _) in scored.iter().filter(|(c, _)| !had.contains(&c.id)) {
        take(c, &mut picked, true);
    }
    // Few fit: the caps give way before anything comes round again…
    for (c, _) in scored.iter().filter(|(c, _)| !had.contains(&c.id)) {
        take(c, &mut picked, false);
    }
    // …then what was had longest ago, never the last few.
    let mut again: Vec<&Candidate> = scored
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| had.contains(&c.id) && !fresh.contains(&c.id))
        .collect();
    again.sort_by_key(|c| age.get(&c.id).copied().unwrap_or(0));
    for c in again {
        take(c, &mut picked, false);
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(loudness: f64, bpm: Option<f64>, vocals: Option<f64>) -> Sound {
        Sound {
            loudness,
            bpm,
            vocals,
        }
    }

    fn who<'a>(heard: &'a HashSet<i64>, flagged: &'a HashSet<i64>) -> Listener<'a> {
        Listener {
            user: 1,
            day: 100,
            heard_lately: heard,
            flagged,
        }
    }

    fn candidate(id: i64, sound: Sound, taste: f64, album: i64, artist: &str) -> Candidate {
        Candidate {
            id,
            sound,
            taste,
            album: Some(album),
            artist: artist.into(),
        }
    }

    #[test]
    fn a_track_fits_the_mixes_its_sound_falls_in() {
        let fits = |s: &Sound| -> Vec<&str> {
            MIXES
                .iter()
                .filter(|m| fit(s, m).is_some())
                .map(|m| m.key)
                .collect()
        };
        // A quiet, slow piano piece without singing.
        assert_eq!(
            fits(&sound(-20.0, Some(70.0), Some(0.0))),
            ["chill", "focus", "sleep"]
        );
        // A loud, fast song with singing throughout.
        assert_eq!(fits(&sound(-7.0, Some(128.0), Some(0.8))), ["energy"]);
        // Its tempo read at half speed still counts.
        assert_eq!(fits(&sound(-7.0, Some(64.0), Some(0.8))), ["energy"]);
        // Unknown singing keeps it out of Focus, which needs to know; unknown tempo out of
        // Energy.
        assert_eq!(fits(&sound(-12.0, None, None)), ["chill"]);
        let middle = fit(&sound(-19.5, Some(85.0), Some(0.0)), &MIXES[0]).unwrap();
        let edge = fit(&sound(-9.0, Some(110.0), Some(0.0)), &MIXES[0]).unwrap();
        assert!(middle > edge, "{middle} > {edge}");
    }

    #[test]
    fn the_next_tracks_fit_suit_the_listener_and_never_run_dry() {
        let focus = mix("focus").unwrap();
        let candidates = vec![
            candidate(1, sound(-18.0, Some(100.0), Some(0.0)), 1.0, 1, "Alder"),
            candidate(2, sound(-18.0, Some(100.0), Some(0.0)), 1.0, 1, "Alder"),
            candidate(3, sound(-18.0, Some(100.0), Some(0.0)), 1.0, 1, "Alder"),
            candidate(4, sound(-18.0, Some(100.0), Some(0.05)), 0.0, 2, "Birch"),
            // Loud and sung: not Focus.
            candidate(5, sound(-6.0, Some(128.0), Some(0.9)), 1.0, 3, "Cedar"),
            candidate(6, sound(-18.0, Some(100.0), Some(0.0)), 0.9, 4, "Dogwood"),
        ];
        let none = HashSet::new();
        let first = next(focus, &candidates, &[], &who(&none, &none), 3);
        assert_eq!(first.len(), 3);
        assert!(!first.contains(&5), "only what fits");
        assert!(
            first.iter().filter(|id| [1, 2, 3].contains(id)).count() <= 2,
            "two at most from one album: {first:?}"
        );
        // The same day gives the same batch; what's flagged or had is left out.
        assert_eq!(next(focus, &candidates, &[], &who(&none, &none), 3), first);
        let flagged = HashSet::from([6]);
        let second = next(focus, &candidates, &first, &who(&none, &flagged), 3);
        assert!(
            second
                .iter()
                .all(|id| !first.contains(id) && *id != 6 && *id != 5)
        );
        // Heard in the last day, a track waits behind the rest.
        let heard = HashSet::from([1, 2, 3, 6]);
        assert_eq!(next(focus, &candidates, &[], &who(&heard, &none), 1), [4]);
        // Everything had: the oldest come round again, the latest never.
        let queue: Vec<i64> = (0..60).map(|i| 1000 + i).chain([1, 2, 3, 4, 6]).collect();
        assert!(next(focus, &candidates, &queue, &who(&none, &none), 3).is_empty());
        let old: Vec<i64> = [4, 1]
            .into_iter()
            .chain((0..60).map(|i| 1000 + i))
            .collect();
        assert_eq!(
            next(focus, &candidates, &old, &who(&none, &none), 5)[..3].len(),
            3
        );
        let again = next(focus, &candidates, &old, &who(&none, &none), 5);
        assert_eq!(&again[again.len() - 2..], [4, 1], "the oldest first");
    }
}
