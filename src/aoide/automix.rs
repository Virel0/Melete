//! How Auto DJ goes from one record into the next, part by part. The planner's beat-matched
//! mix (held to parity with the apps) stays first; where it refuses, the rules for a pop and
//! rock library take over: a tempo gap too wide to bend one record is
//! met by bending both, and a pair that can't share an overlap is joined by throwing the outgoing
//! singer's last line over a cut. With a track's separated parts, every move is a level for
//! one part on one side over time, so the apps' decks play the track plus each part at
//! `gain − 1` and change only that part. Without parts a plan has no lanes, and a deck plays it
//! as it always has. Rules decide all of it; nothing here is learned.
//!
//! - **Beatmatch:** the planner's plan. The low end swaps on the bar after halfway (two kick
//!   drums at once is mud); the incoming band comes up over the first half; the outgoing band
//!   goes over the second; no two voices ever sound together.
//! - **Ramp:** a tempo gap of 6% to 30%, folded into one octave: both records glide to the
//!   tempo between them (the geometric middle, so each bends by the same proportion) over the
//!   first quarter of the mix, and the incoming one eases back to its own over eight seconds
//!   after it. Sixteen bars, on downbeats and phrase lines.
//! - **Throw:** the outgoing band drops on a beat a bar before its singer's last line ends; the
//!   line rings on alone and echoes out while the incoming record starts whole on the cut.
//!   Needs the outgoing record's parts, and its last line inside the end region they cover.
//! - **Crossfade:** eight seconds, when nothing else fits.
//!
//! Keys: where they clash, the incoming band is shifted toward the outgoing key by two
//! semitones at most, and its voice only when that's one semitone or none; otherwise the key is
//! left as it is.

use serde::Serialize;

use super::planner::{self, MixRecord, Style};

/// The widest tempo gap a ramp meets; beyond it, records aren't beat-matched at all.
const RAMP_LIMIT: f64 = 0.30;
/// Where the planner stops bending one record and a ramp starts bending both.
const RAMP_FROM: f64 = planner::MAXIMUM_STRETCH;
const RAMP_BARS: u32 = 16;
const CROSSFADE_SECONDS: f64 = 8.0;
/// How long a thrown line echoes on after the cut.
const ECHO_BARS: f64 = 2.0;
const MAXIMUM_BAND_SHIFT: i64 = 2;
const MAXIMUM_VOICE_SHIFT: i64 = 1;

/// The version of these rules, sent with every plan as `rules` and sent back with each
/// transition an app reports, so what's learned from transitions compares plans made by
/// the same rules. **Raise it by one whenever the planner's rules change** (here or in
/// `planner`): anything that would plan a pair of records differently.
pub const RULES: i64 = 1;

/// Where a record's band parts exist, in milliseconds of it; its voice covers all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// Where the parts start, in milliseconds of the record.
    pub from_ms: f64,
    /// Where the parts end, in milliseconds of the record.
    pub to_ms: f64,
}

impl Region {
    fn covers(&self, from: f64, to: f64) -> bool {
        from >= self.from_ms - 1.0 && to <= self.to_ms + 1.0
    }
}

/// Which way a transition goes from one record into the next (see the module's list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The planner's beat-matched mix.
    Beatmatch,
    /// Both records glide to the tempo between them, then the incoming one eases back.
    Ramp,
    /// The outgoing singer's last line rings on over a cut into the incoming record.
    Throw,
    /// Eight seconds of plain crossfade, when nothing else fits.
    Crossfade,
}

/// Which record a lane's part belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Outgoing,
    Incoming,
}

/// A part's level over the mix: at so many seconds from its start, this gain (straight lines
/// between, held after the last).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lane {
    /// Which record the part is in.
    pub side: Side,
    /// The part's name: `drums`, `bass`, `guitar`, `other` or `voice`.
    pub part: &'static str,
    /// `(seconds from the mix's start, gain)` pairs, in time order; a gain of 1.0 is the part at
    /// its own level.
    pub points: Vec<(f64, f64)>,
}

/// One transition, for a deck to play.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mix {
    /// Which way the mix goes.
    pub kind: Kind,
    /// The version of the rules that made it ([`RULES`]).
    pub rules: i64,
    /// Where the mix starts in the outgoing record, in milliseconds of it.
    pub outgoing_start_ms: f64,
    /// Where the incoming record starts playing, in milliseconds of it.
    pub incoming_start_ms: f64,
    /// How far into the mix the incoming record starts: 0 but for a throw, where it starts
    /// on the cut.
    pub incoming_after_seconds: f64,
    /// How long the mix lasts, in seconds.
    pub seconds: f64,
    /// The rate the outgoing record runs at during the mix (pitch held); 1.0 is its own.
    pub outgoing_rate: f64,
    /// The rate the incoming record runs at during the mix (pitch held).
    pub incoming_rate: f64,
    /// Seconds the incoming record takes after the mix to ease back to its own rate; 0 when it
    /// never left it.
    pub restore_seconds: f64,
    /// Semitones the incoming record's band is shifted by, toward the outgoing key.
    pub band_semitones: i64,
    /// Semitones the incoming record's voice is shifted by; 0 where a shift would be more
    /// than one semitone.
    pub voice_semitones: i64,
    /// The planner's style, for a deck playing it without parts.
    pub style: Option<&'static str>,
    /// For a throw: how long the outgoing line echoes on after the cut.
    pub echo_seconds: Option<f64>,
    /// Each part's level, when both records' parts cover the mix; none means play it whole.
    pub lanes: Vec<Lane>,
}

/// A record and where its parts are, if it's been split.
#[derive(Clone, Copy)]
pub struct Deck<'a> {
    /// The record's beat grid, key and arrangement, as the planner reads them; none when it hasn't
    /// been measured.
    pub record: Option<MixRecord<'a>>,
    /// Where its parts cover the record; none when it hasn't been split into parts.
    pub parts: Option<Region>,
    /// Its length, when known.
    pub length_ms: Option<f64>,
}

/// The tempo ratio folded into one octave: 1.0 for the same tempo or a double-time one.
fn folded(target: f64, bpm: f64) -> Option<f64> {
    if !(bpm > 0.0 && target > 0.0 && bpm.is_finite() && target.is_finite()) {
        return None;
    }
    let mut ratio = target / bpm;
    while ratio > 1.5 {
        ratio /= 2.0;
    }
    while ratio < 1.0 / 1.5 {
        ratio *= 2.0;
    }
    Some(ratio)
}

/// A Camelot key's number and letter.
fn camelot(key: &str) -> Option<(i64, char)> {
    let letter = key.chars().last()?.to_ascii_uppercase();
    let number: i64 = key.get(..key.len().checked_sub(1)?)?.parse().ok()?;
    ((1..=12).contains(&number) && matches!(letter, 'A' | 'B')).then_some((number, letter))
}

/// The smallest shift, in semitones, that makes `incoming`'s key compatible with `outgoing`'s,
/// within `limit`; 0 when they already are or can't be told. Up a semitone is seven steps round
/// the wheel.
pub fn key_shift(outgoing: Option<&str>, incoming: Option<&str>, limit: i64) -> Option<i64> {
    let (Some(out), Some((n, letter))) = (outgoing, incoming.and_then(camelot)) else {
        return Some(0);
    };
    if camelot(out).is_none() || planner::compatible(Some(out), incoming) {
        return Some(0);
    }
    let mut shifts: Vec<i64> = (1..=limit).flat_map(|s| [s, -s]).collect();
    shifts.sort_by_key(|s| s.abs());
    shifts.into_iter().find(|s| {
        let moved = (n - 1 + 7 * s).rem_euclid(12) + 1;
        planner::compatible(Some(out), Some(&format!("{moved}{letter}")))
    })
}

/// The levels for a mix where both records play together for `seconds` of `bars` bars: the
/// band in over the first half and out over the second, the low end swapped on the bar after
/// halfway, and one voice at a time (the outgoing out by halfway, the incoming in after it).
fn blend_lanes(seconds: f64, bars: u32) -> Vec<Lane> {
    let half = seconds / 2.0;
    let bar = seconds / f64::from(bars.max(1));
    let lane = |side, part, points: &[(f64, f64)]| Lane {
        side,
        part,
        points: points.to_vec(),
    };
    let mut lanes = Vec::new();
    for part in ["drums", "guitar", "other"] {
        lanes.push(lane(
            Side::Outgoing,
            part,
            &[(0.0, 1.0), (half, 1.0), (seconds, 0.0)],
        ));
        lanes.push(lane(Side::Incoming, part, &[(0.0, 0.0), (half, 1.0)]));
    }
    lanes.push(lane(
        Side::Outgoing,
        "bass",
        &[(0.0, 1.0), (half, 1.0), (half + bar, 0.0)],
    ));
    lanes.push(lane(
        Side::Incoming,
        "bass",
        &[(0.0, 0.0), (half, 0.0), (half + bar, 1.0)],
    ));
    lanes.push(lane(Side::Outgoing, "voice", &[(0.0, 1.0), (half, 0.0)]));
    lanes.push(lane(
        Side::Incoming,
        "voice",
        &[(0.0, 0.0), (half, 0.0), (half + bar, 1.0)],
    ));
    lanes
}

/// Shifts for the incoming record where keys clash: its band by up to two semitones, its voice
/// only by one or none (else neither moves).
fn shifts(outgoing: &MixRecord<'_>, incoming: &MixRecord<'_>) -> (i64, i64) {
    match key_shift(outgoing.key, incoming.key, MAXIMUM_BAND_SHIFT) {
        Some(s) if s.abs() <= MAXIMUM_VOICE_SHIFT => (s, s),
        Some(s) => (s, 0),
        None => (0, 0),
    }
}

/// How Auto DJ goes from `outgoing` into `incoming`.
pub fn mix(outgoing: Deck<'_>, incoming: Deck<'_>) -> Mix {
    if let (Some(out), Some(inc)) = (outgoing.record, incoming.record) {
        if let Some(mix) = beatmatch(&out, &inc, outgoing.parts, incoming.parts) {
            return mix;
        }
        if let Some(mix) = ramp(&out, &inc, outgoing.parts, incoming.parts) {
            return mix;
        }
    }
    if let Some(mix) = throw(&outgoing, &incoming) {
        return mix;
    }
    crossfade(&outgoing, &incoming)
}

fn beatmatch(
    out: &MixRecord<'_>,
    inc: &MixRecord<'_>,
    out_parts: Option<Region>,
    in_parts: Option<Region>,
) -> Option<Mix> {
    let t = planner::plan(*out, *inc, 0.0)?;
    let (band, voice) = shifts(out, inc);
    let fits = out_parts.is_some_and(|r| {
        r.covers(
            t.outgoing_start_ms,
            t.outgoing_start_ms + t.seconds * 1000.0,
        )
    }) && in_parts.is_some_and(|r| {
        r.covers(
            t.incoming_start_ms,
            t.incoming_start_ms + t.seconds * 1000.0 * t.incoming_rate,
        )
    });
    Some(Mix {
        kind: Kind::Beatmatch,
        rules: RULES,
        outgoing_start_ms: t.outgoing_start_ms,
        incoming_start_ms: t.incoming_start_ms,
        incoming_after_seconds: 0.0,
        seconds: t.seconds,
        outgoing_rate: t.outgoing_rate,
        incoming_rate: t.incoming_rate,
        restore_seconds: t.restore_seconds,
        band_semitones: if fits { band } else { 0 },
        voice_semitones: if fits { voice } else { 0 },
        style: Some(match t.style {
            Style::Blend => "blend",
            Style::FilterFade => "filter_fade",
        }),
        echo_seconds: None,
        lanes: if fits {
            blend_lanes(t.seconds, t.bars)
        } else {
            Vec::new()
        },
    })
}

fn ramp(
    out: &MixRecord<'_>,
    inc: &MixRecord<'_>,
    out_parts: Option<Region>,
    in_parts: Option<Region>,
) -> Option<Mix> {
    let (out_arr, in_arr) = (out.arrangement?, inc.arrangement?);
    let (exit, entry) = (out.grid.mix_out_ms?, inc.grid.mix_in_ms?);
    let out_bpm = out.grid.segment(exit)?.bpm;
    let in_bpm = inc.grid.segment(entry)?.bpm;
    // The incoming tempo as it would sit beside the outgoing one, folded into one octave.
    let ratio = folded(out_bpm, in_bpm)?;
    let gap = (ratio - 1.0).abs();
    if !(gap > RAMP_FROM && gap <= RAMP_LIMIT) {
        return None;
    }
    // The incoming tempo in the outgoing one's octave (a double-time record counts at half).
    let in_beside = out_bpm / ratio;
    // Meet in the middle, each bent by the same proportion.
    let middle = (out_bpm * in_beside).sqrt();
    let (outgoing_rate, incoming_rate) = (middle / out_bpm, middle / in_beside);
    let bar_at = |bpm: f64, beats: i64| 60_000.0 / bpm * beats as f64;
    let out_bar = bar_at(out_bpm, out.grid.beats_per_bar?);
    let mix_ms = bar_at(middle, out.grid.beats_per_bar?) * f64::from(RAMP_BARS);
    let start = out
        .grid
        .downbeat_at_or_before(exit - out_bar * f64::from(RAMP_BARS))?;
    let in_bar = inc.grid.bar_ms(entry)?;
    let entry = in_arr
        .phrase_line_near(entry, in_bar)
        .or_else(|| inc.grid.downbeat_at_or_before(entry))
        .unwrap_or(entry);
    let seconds = mix_ms / 1000.0;
    // Two voices across the overlap can't be ramped together.
    if out_arr.may_sing(start, start + seconds * 1000.0 * outgoing_rate)
        && in_arr.may_sing(entry, entry + seconds * 1000.0 * incoming_rate)
    {
        return None;
    }
    let fits = out_parts.is_some_and(|r| r.covers(start, start + seconds * 1000.0 * outgoing_rate))
        && in_parts.is_some_and(|r| r.covers(entry, entry + seconds * 1000.0 * incoming_rate));
    let (band, voice) = shifts(out, inc);
    Some(Mix {
        kind: Kind::Ramp,
        rules: RULES,
        outgoing_start_ms: start,
        incoming_start_ms: entry,
        incoming_after_seconds: 0.0,
        seconds,
        outgoing_rate,
        incoming_rate,
        restore_seconds: planner::RESTORE_SECONDS,
        band_semitones: if fits { band } else { 0 },
        voice_semitones: if fits { voice } else { 0 },
        style: None,
        echo_seconds: None,
        lanes: if fits {
            blend_lanes(seconds, RAMP_BARS)
        } else {
            Vec::new()
        },
    })
}

fn throw(outgoing: &Deck<'_>, incoming: &Deck<'_>) -> Option<Mix> {
    let out = outgoing.record?;
    let region = outgoing.parts?;
    let spans = out.arrangement?.vocals.as_ref()?;
    let beats = out.grid.beats_per_bar?;
    // The last line that ends inside the end region, with a bar of band before it to drop.
    let line_end = spans
        .iter()
        .map(|v| v.end_ms)
        .filter(|&end| end <= region.to_ms)
        .fold(None::<f64>, |last, end| {
            Some(last.map_or(end, |l| l.max(end)))
        })?;
    let bpm = out.grid.segment(line_end)?.bpm;
    let beat = 60_000.0 / bpm;
    let bar = beat * beats as f64;
    let drop = out.grid.downbeat_at_or_before(line_end - bar)?;
    if !region.covers(drop, line_end) {
        return None;
    }
    let cut = (line_end - drop) / 1000.0;
    let echo = ECHO_BARS * bar / 1000.0;
    let entry = incoming
        .record
        .and_then(|r| r.grid.mix_in_ms)
        .unwrap_or(0.0);
    let mut lanes: Vec<Lane> = ["drums", "bass", "guitar", "other"]
        .into_iter()
        .map(|part| Lane {
            side: Side::Outgoing,
            part,
            points: vec![(0.0, 1.0), (beat / 1000.0, 0.0)],
        })
        .collect();
    lanes.push(Lane {
        side: Side::Outgoing,
        part: "voice",
        points: vec![(0.0, 1.0), (cut, 1.0), (cut + echo, 0.0)],
    });
    Some(Mix {
        kind: Kind::Throw,
        rules: RULES,
        outgoing_start_ms: drop,
        // The incoming record starts whole on the cut.
        incoming_start_ms: entry,
        incoming_after_seconds: cut,
        seconds: cut + echo,
        outgoing_rate: 1.0,
        incoming_rate: 1.0,
        restore_seconds: 0.0,
        band_semitones: 0,
        voice_semitones: 0,
        style: None,
        echo_seconds: Some(echo),
        lanes,
    })
}

fn crossfade(outgoing: &Deck<'_>, incoming: &Deck<'_>) -> Mix {
    let exit = outgoing
        .record
        .and_then(|r| r.grid.mix_out_ms)
        .or_else(|| {
            outgoing
                .length_ms
                .map(|l| (l - CROSSFADE_SECONDS * 1000.0).max(0.0))
        })
        .unwrap_or(0.0);
    let entry = incoming
        .record
        .and_then(|r| r.grid.mix_in_ms)
        .unwrap_or(0.0);
    Mix {
        kind: Kind::Crossfade,
        rules: RULES,
        outgoing_start_ms: exit,
        incoming_start_ms: entry,
        incoming_after_seconds: 0.0,
        seconds: CROSSFADE_SECONDS,
        outgoing_rate: 1.0,
        incoming_rate: 1.0,
        restore_seconds: 0.0,
        band_semitones: 0,
        voice_semitones: 0,
        style: None,
        echo_seconds: None,
        lanes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aoide::planner::{Arrangement, Grid, Section, Segment, VocalSpan};

    const MINUTES_4: f64 = 240_000.0;

    #[test]
    fn every_plan_says_which_rules_made_it() {
        let unknown = Deck {
            record: None,
            parts: None,
            length_ms: None,
        };
        let plan = mix(unknown, unknown);
        assert_eq!((plan.kind, plan.rules), (Kind::Crossfade, RULES));
        let sent = serde_json::to_value(&plan).unwrap();
        assert_eq!(sent["rules"], RULES);
    }

    /// A four-minute record at a tempo, in 4/4 from 0, mixable from bar 8 to two bars before
    /// the end, read as one unremarkable section with sixteen-bar phrases.
    fn grid(bpm: f64) -> Grid {
        let bar = 60_000.0 / bpm * 4.0;
        Grid {
            segments: vec![Segment {
                start_ms: 0.0,
                end_ms: MINUTES_4,
                anchor_ms: 0.0,
                bpm,
                residual_ms: 5.0,
            }],
            beats_per_bar: Some(4),
            mix_in_ms: Some(bar * 8.0),
            mix_out_ms: Some(MINUTES_4 - bar * 2.0),
        }
    }

    fn arrangement(vocals: Vec<(f64, f64)>) -> Arrangement {
        Arrangement {
            sections: vec![Section {
                start_ms: 0.0,
                end_ms: MINUTES_4,
                kind: "unknown".into(),
                energy: 0.5,
            }],
            phrase_bars: Some(16),
            phrase_anchor_ms: Some(0.0),
            vocals: Some(
                vocals
                    .into_iter()
                    .map(|(s, e)| VocalSpan {
                        start_ms: s,
                        end_ms: e,
                    })
                    .collect(),
            ),
        }
    }

    /// Parts over the whole record, so any mix fits.
    const EVERYWHERE: Region = Region {
        from_ms: 0.0,
        to_ms: MINUTES_4,
    };

    fn deck<'a>(
        grid: &'a Grid,
        arrangement: &'a Arrangement,
        key: Option<&'a str>,
        parts: Option<Region>,
    ) -> Deck<'a> {
        Deck {
            record: Some(MixRecord {
                grid,
                key,
                arrangement: Some(arrangement),
            }),
            parts,
            length_ms: Some(MINUTES_4),
        }
    }

    fn lane<'a>(mix: &'a Mix, side: Side, part: &str) -> &'a [(f64, f64)] {
        &mix.lanes
            .iter()
            .find(|l| l.side == side && l.part == part)
            .unwrap()
            .points
    }

    #[test]
    fn a_pair_that_beat_matches_swaps_its_parts_bar_by_bar() {
        let (g, a) = (grid(128.0), arrangement(vec![]));
        let with = mix(
            deck(&g, &a, None, Some(EVERYWHERE)),
            deck(&g, &a, None, Some(EVERYWHERE)),
        );
        assert_eq!((with.kind, with.style), (Kind::Beatmatch, Some("blend")));
        let (half, bar) = (with.seconds / 2.0, 60.0 / 128.0 * 4.0);
        assert_eq!(
            lane(&with, Side::Outgoing, "bass"),
            [(0.0, 1.0), (half, 1.0), (half + bar, 0.0)],
            "the low end swaps on the bar after halfway"
        );
        assert_eq!(
            lane(&with, Side::Incoming, "bass"),
            [(0.0, 0.0), (half, 0.0), (half + bar, 1.0)]
        );
        assert_eq!(
            lane(&with, Side::Incoming, "drums"),
            [(0.0, 0.0), (half, 1.0)]
        );
        // One voice at a time: the outgoing gone by halfway, the incoming in after it.
        assert_eq!(
            lane(&with, Side::Outgoing, "voice").last(),
            Some(&(half, 0.0))
        );
        assert_eq!(lane(&with, Side::Incoming, "voice")[1], (half, 0.0));
        // Without parts covering the mix, the same plan plays whole.
        let without = mix(
            deck(&g, &a, None, None),
            deck(&g, &a, None, Some(EVERYWHERE)),
        );
        assert_eq!((without.kind, without.lanes.len()), (Kind::Beatmatch, 0));
        let short = Region {
            from_ms: MINUTES_4 - 1000.0,
            to_ms: MINUTES_4,
        };
        assert!(
            mix(
                deck(&g, &a, None, Some(short)),
                deck(&g, &a, None, Some(EVERYWHERE))
            )
            .lanes
            .is_empty()
        );
    }

    #[test]
    fn a_tempo_gap_too_wide_for_one_record_bends_both() {
        let (fast, slow, a) = (grid(128.0), grid(100.0), arrangement(vec![]));
        let ramp = mix(
            deck(&fast, &a, None, Some(EVERYWHERE)),
            deck(&slow, &a, None, Some(EVERYWHERE)),
        );
        assert_eq!(ramp.kind, Kind::Ramp);
        let middle = (128.0f64 * 100.0).sqrt();
        assert!(
            (ramp.outgoing_rate - middle / 128.0).abs() < 1e-9
                && (ramp.incoming_rate - middle / 100.0).abs() < 1e-9
        );
        assert_eq!(ramp.restore_seconds, planner::RESTORE_SECONDS);
        assert!(
            (ramp.seconds - 60.0 / middle * 4.0 * 16.0).abs() < 1e-9,
            "sixteen bars at the meeting tempo"
        );
        // Double time counts at half: 200 beside 128 is 100 beside it.
        let double = grid(200.0);
        assert_eq!(
            mix(deck(&fast, &a, None, None), deck(&double, &a, None, None)).kind,
            Kind::Ramp
        );
    }

    #[test]
    fn a_pair_too_far_apart_throws_the_last_line_or_crossfades() {
        // 92 beside 128 is 39% apart, past what a ramp meets.
        let (fast, slow) = (grid(128.0), grid(92.0));
        // Sung until 3:50, a bar or so before the two-bar mix-out.
        let sung = arrangement(vec![(30_000.0, 230_000.0)]);
        let end = Region {
            from_ms: MINUTES_4 - 60_000.0,
            to_ms: MINUTES_4,
        };
        let throw = mix(
            deck(&fast, &sung, None, Some(end)),
            deck(&slow, &arrangement(vec![]), None, None),
        );
        assert_eq!(throw.kind, Kind::Throw);
        let beat = 60.0 / 128.0;
        assert!(throw.outgoing_start_ms <= 230_000.0 - beat * 4.0 * 1000.0 + 1e-6);
        assert_eq!(
            lane(&throw, Side::Outgoing, "drums"),
            [(0.0, 1.0), (beat, 0.0)],
            "the band drops on a beat"
        );
        let voice = lane(&throw, Side::Outgoing, "voice");
        assert_eq!(
            voice[1],
            (throw.incoming_after_seconds, 1.0),
            "the line rings to the cut"
        );
        assert_eq!(voice[2].1, 0.0);
        assert!(
            (throw.incoming_after_seconds - (230_000.0 - throw.outgoing_start_ms) / 1000.0).abs()
                < 1e-9
        );
        assert_eq!(throw.echo_seconds, Some(beat * 4.0 * 2.0));
        // Without the outgoing record's parts, it's a crossfade.
        let plain = mix(
            deck(&fast, &sung, None, None),
            deck(&slow, &arrangement(vec![]), None, None),
        );
        assert_eq!(
            (plain.kind, plain.seconds, plain.lanes.len()),
            (Kind::Crossfade, 8.0, 0)
        );
        // Unread records crossfade from near the end.
        let unread = mix(
            Deck {
                record: None,
                parts: None,
                length_ms: Some(200_000.0),
            },
            Deck {
                record: None,
                parts: None,
                length_ms: None,
            },
        );
        assert_eq!(
            (
                unread.kind,
                unread.outgoing_start_ms,
                unread.incoming_start_ms
            ),
            (Kind::Crossfade, 192_000.0, 0.0)
        );
    }

    #[test]
    fn keys_meet_by_moving_the_band_two_semitones_at_most_and_the_voice_one() {
        assert_eq!(key_shift(Some("8A"), Some("8A"), 2), Some(0));
        assert_eq!(
            key_shift(Some("8A"), Some("9A"), 2),
            Some(0),
            "already compatible"
        );
        // 3A down a semitone is 8A (seven steps round the wheel each way).
        assert_eq!(key_shift(Some("8A"), Some("3A"), 2), Some(-1));
        assert_eq!(key_shift(Some("8A"), Some("10A"), 2), Some(-2));
        assert_eq!(
            key_shift(Some("8A"), Some("2A"), 2),
            Some(1),
            "2A up a semitone is 9A"
        );
        assert_eq!(
            key_shift(Some("8A"), Some("5B"), 2),
            None,
            "too far to move"
        );
        assert_eq!(
            key_shift(None, Some("2A"), 2),
            Some(0),
            "unknown keys aren't moved"
        );
        let (g, a) = (grid(128.0), arrangement(vec![]));
        let two = mix(
            deck(&g, &a, Some("8A"), Some(EVERYWHERE)),
            deck(&g, &a, Some("10A"), Some(EVERYWHERE)),
        );
        assert_eq!(
            (two.band_semitones, two.voice_semitones),
            (-2, 0),
            "the voice isn't moved two"
        );
        let one = mix(
            deck(&g, &a, Some("8A"), Some(EVERYWHERE)),
            deck(&g, &a, Some("3A"), Some(EVERYWHERE)),
        );
        assert_eq!((one.band_semitones, one.voice_semitones), (-1, -1));
    }
}
