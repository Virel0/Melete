//! Where a track's beats fall, as a mixed transition needs them: the sidecar's beat tracker
//! and grid analyzer (`BeatTracker.cs`, `BeatGridAnalyzer.cs`), ported line for line.
//!
//! The tracker is the standard dynamic-programming one: the chain of onsets with the most
//! combined strength, less `(ln(interval / period))²` for every interval off the expected
//! period. The beats it finds are fitted with straight lines (beat number against time),
//! split where one fit can't describe a tempo change. The result is a grid, not a list of
//! timestamps, because the residual — how far the track's own onsets sit from the fit — is
//! what says whether anything can lock to it. The meter is read from the low bands, where a
//! kick marks the downbeat, and the mix points are the first and last four-bar phrases where
//! every bar carries the groove.
//!
//! Where the original rounds with C#'s `Math.Round`, halves go to even here too.

use super::tempo::{self, Tempo};

mod fitting;
mod phrasing;
mod tracking;

use fitting::*;
use phrasing::*;
use tracking::*;

/// How hard a departure from the expected period is penalised: loose enough that a track
/// that really changes tempo is followed rather than flattened.
const TIGHTNESS: f64 = 90.0;
/// A fit better than this isn't worth splitting.
const GOOD_RESIDUAL_MS: f64 = 12.0;
/// A split must bring the residual under this share of the whole's to be worth making.
const WORTH_SPLITTING: f64 = 0.75;
/// How many times a segment may be split in two.
const MAX_SPLIT_DEPTH: u32 = 3;
/// No segment shorter than this, in beats.
const MINIMUM_SEGMENT_BEATS: usize = 32;
/// Beyond this the fit describes something that isn't a steady beat.
const USELESS_RESIDUAL_MS: f64 = 120.0;
/// Meters tried, in the order they're preferred.
const METERS: [i64; 2] = [4, 3];
/// How much louder a downbeat must be than an average beat to count as found.
const MINIMUM_DOWNBEAT_CONTRAST: f64 = 0.12;
/// How much better three must look than four before a waltz is believed.
const WALTZ_MARGIN: f64 = 1.3;
/// A beat with less than this share of the median isn't part of the groove.
const GROOVE_SHARE: f64 = 0.5;
/// Under this share of the median, a beat at either end isn't a beat at all: the tracker was
/// chaining through nothing.
const SILENT_SHARE: f64 = 0.25;

/// One stretch over which a single fixed grid fits: `beat(n) = anchor_ms + n · 60000 / bpm`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Where the stretch starts, in milliseconds.
    pub start_ms: f64,
    /// Where it ends, in milliseconds.
    pub end_ms: f64,
    /// Beat zero, on a downbeat when the meter is known.
    pub anchor_ms: f64,
    /// Its tempo, in beats a minute.
    pub bpm: f64,
    /// RMS distance between the fitted beats and the ones detected.
    pub residual_ms: f64,
    /// How many beats it holds.
    pub beats: i64,
}

/// Where a track's beats and bars fall, and where a mix may enter and leave it.
#[derive(Debug, Clone, PartialEq)]
pub struct BeatGrid {
    /// One fit, or several where the track really changes tempo; together they cover it.
    pub segments: Vec<Segment>,
    /// Four for nearly everything, three for a waltz; None when the meter wasn't clear.
    pub beats_per_bar: Option<i64>,
    /// Which beat of the first segment begins a bar; 0, or None with the meter.
    pub downbeat_index: Option<i64>,
    /// The first downbeat of the first bar actually playing, past an intro of atmosphere.
    pub mix_in_ms: Option<f64>,
    /// The downbeat of the last bar before the ending stops being part of the groove.
    pub mix_out_ms: Option<f64>,
}

/// C#'s `Math.Round(x)`: halves to even.
fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// C#'s `Math.Round(x, digits)`.
fn round_to(x: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (x * scale).round_ties_even() / scale
}

struct Fit {
    from: usize,
    to: usize,
    intercept: f64,
    ms_per_beat: f64,
    residual_ms: f64,
    beats: i64,
}

#[derive(Clone, Copy)]
struct Meter {
    beats_per_bar: i64,
    phase: i64,
}

impl Meter {
    fn is_downbeat(self, number: i64) -> bool {
        (number - self.phase).rem_euclid(self.beats_per_bar) == 0
    }
}

/// The beats a grid was fitted to, for measuring per beat rather than per hop.
pub struct Tracked {
    /// Each beat's time, in milliseconds.
    pub times_ms: Vec<f64>,
    /// Each beat's number, which skips where the tracker missed one.
    pub numbers: Vec<i64>,
    /// The first and last beats that carried anything.
    pub first: usize,
    /// The last beat that carried anything, as an index into `times_ms`.
    pub last: usize,
    /// Beats in a bar; none when the meter wasn't clear.
    pub beats_per_bar: Option<i64>,
    /// Which beat number begins a bar.
    pub phase: i64,
}

/// The grid of a track, or None when there's none worth having. `onsets` and `low_onsets`
/// are the tempo scan's, `rate` values a second from `first_onset_ms` on.
#[cfg(test)]
pub fn analyze(
    onsets: &[f32],
    low_onsets: &[f32],
    rate: f64,
    first_onset_ms: f64,
    duration_ms: f64,
    tempo: Option<&Tempo>,
) -> Option<BeatGrid> {
    analyze_with_beats(onsets, low_onsets, rate, first_onset_ms, duration_ms, tempo)
        .map(|(grid, _)| grid)
}

/// The grid, with the beats it was built from.
pub fn analyze_with_beats(
    onsets: &[f32],
    low_onsets: &[f32],
    rate: f64,
    first_onset_ms: f64,
    duration_ms: f64,
    tempo: Option<&Tempo>,
) -> Option<(BeatGrid, Tracked)> {
    // A tracker given a wrong period finds a confident grid for the wrong thing.
    let tempo = tempo.filter(|t| t.confidence >= tempo::MINIMUM_CONFIDENCE)?;
    let period = 60.0 * rate / tempo.bpm;
    let tracked = track(&tempo::centred(onsets, rate), period);
    if tracked.len() < MINIMUM_SEGMENT_BEATS {
        return None;
    }
    let ms_per_hop = 1000.0 / rate;
    let times: Vec<f64> = tracked
        .iter()
        .map(|&t| first_onset_ms + t as f64 * ms_per_hop)
        .collect();
    // Beat numbers, not positions in the list: a missed beat leaves a hole, and a fit that
    // ignored it would report a tempo slower than the track's.
    let expected = 60_000.0 / tempo.bpm;
    let mut numbers = vec![0i64; times.len()];
    for i in 1..times.len() {
        numbers[i] = numbers[i - 1] + (round((times[i] - times[i - 1]) / expected) as i64).max(1);
    }
    // The tracker chains from the first hop to the last, so an intro of pad gets beats that
    // aren't there; fitting them would make a segment describing nothing.
    let (carries, first, last) = sounding(&tracked, onsets);
    if last - first + 1 < MINIMUM_SEGMENT_BEATS {
        return None;
    }
    let mut fits = Vec::new();
    split(&times, &numbers, &carries, first, last, 0, &mut fits);
    if fits.is_empty() {
        return None;
    }
    let meter = meter(&tracked, &numbers, low_onsets);
    let segments = describe(&fits, &times, &numbers, duration_ms, meter);
    // A grid nothing could lock to is still returned, residual and all, unless every part of
    // it is beyond use.
    if segments.is_empty() || segments.iter().all(|s| s.residual_ms > USELESS_RESIDUAL_MS) {
        return None;
    }
    let (mix_in_ms, mix_out_ms) = mix_points(&tracked, &times, &numbers, onsets, &segments, meter);
    let grid = BeatGrid {
        segments,
        beats_per_bar: meter.map(|m| m.beats_per_bar),
        downbeat_index: meter.map(|_| 0),
        mix_in_ms,
        mix_out_ms,
    };
    let beats = Tracked {
        times_ms: times,
        numbers,
        first,
        last,
        beats_per_bar: meter.map(|m| m.beats_per_bar),
        phase: meter.map_or(0, |m| m.phase),
    };
    Some((grid, beats))
}

fn strength(tracked: &[usize], onsets: &[f32]) -> Vec<f64> {
    tracked
        .iter()
        .map(|&t| onsets.get(t).map_or(0.0, |&v| f64::from(v).max(0.0)))
        .collect()
}

fn median_of(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

fn mean(values: &[f64], from: usize, count: usize) -> f64 {
    values[from..from + count].iter().sum::<f64>() / count as f64
}

#[cfg(test)]
mod tests {
    //! The sidecar's `BeatGridTests`, over synthesised music with beats known to the sample.
    //! Noise comes from our own seeded generator, not .NET's `Random`.

    use super::super::Consumer;
    use super::super::tempo::TempoScan;
    use super::super::tempo::tests::Random;
    use super::*;

    const SAMPLE_RATE: u32 = 44_100;
    const RATE: f64 = SAMPLE_RATE as f64;
    const CHANNELS: usize = 2;

    fn measure(samples: &[f32]) -> Option<BeatGrid> {
        let mut scan = TempoScan::new(CHANNELS, SAMPLE_RATE);
        scan.feed(samples);
        analyze(
            &scan.onsets,
            &scan.low_onsets,
            scan.rate,
            scan.first_onset_ms(),
            samples.len() as f64 / CHANNELS as f64 / RATE * 1000.0,
            scan.result().as_ref(),
        )
    }

    fn stereo(frames: usize, mut value: impl FnMut(usize) -> f64) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let v = value(frame).clamp(-1.0, 1.0) as f32;
                [v, v]
            })
            .collect()
    }

    /// A loud low kick on the downbeat, a quieter tick on the rest, and a pad underneath so
    /// it's never silent between hits.
    fn groove(bpm: f64, seconds: usize, first_beat_ms: f64, beats_per_bar: i64) -> Vec<f32> {
        let beat_frames = 60.0 / bpm * RATE;
        let first_frame = first_beat_ms / 1000.0 * RATE;
        let mut random = Random::new(13);
        stereo(SAMPLE_RATE as usize * seconds, |frame| {
            let beat = (frame as f64 - first_frame) / beat_frames;
            let index = beat.floor() as i64;
            let since = (frame as f64 - first_frame - index as f64 * beat_frames) / RATE;
            let mut value = 0.05 * (2.0 * std::f64::consts::PI * 330.0 * frame as f64 / RATE).sin();
            if index >= 0 && since >= 0.0 {
                if index.rem_euclid(beats_per_bar) == 0 {
                    value += 0.8
                        * (-since / 0.06).exp()
                        * (2.0 * std::f64::consts::PI * 55.0 * since).sin();
                } else {
                    value += 0.12 * (-since / 0.01).exp() * (random.next() * 2.0 - 1.0);
                }
            }
            value
        })
    }

    /// A loud sustained pad: plenty of level, nothing starting.
    fn pad(seconds: usize) -> Vec<f32> {
        stereo(SAMPLE_RATE as usize * seconds, |frame| {
            let t = frame as f64 / RATE;
            0.4 * (0.6 * (2.0 * std::f64::consts::PI * 110.0 * t).sin()
                + 0.4 * (2.0 * std::f64::consts::PI * 165.0 * t).sin())
        })
    }

    fn joined(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    #[test]
    fn a_sequenced_track_fits_one_grid_tightly() {
        let grid = measure(&groove(128.0, 180, 500.0, 4)).unwrap();
        let [segment] = grid.segments[..] else {
            panic!("{:?}", grid.segments);
        };
        assert!((127.9..=128.1).contains(&segment.bpm), "{segment:?}");
        assert!(segment.residual_ms <= 12.0, "{segment:?}");
        assert!(segment.beats > 300, "{segment:?}");
    }

    /// A tempo says how far apart the beats are; only the anchor says where.
    #[test]
    fn the_anchor_lands_on_a_real_downbeat() {
        let first_beat_ms = 500.0;
        let grid = measure(&groove(128.0, 180, first_beat_ms, 4)).unwrap();
        let segment = grid.segments[0];
        let bar_ms = 4.0 * 60_000.0 / segment.bpm;
        let offset = (segment.anchor_ms - first_beat_ms) % bar_ms;
        let error = offset.abs().min((bar_ms - offset.abs()).abs());
        assert!(
            error <= 20.0,
            "the anchor was {error:.1} ms off a real downbeat"
        );
    }

    #[test]
    fn four_four_is_recognised_and_so_is_its_downbeat() {
        let grid = measure(&groove(128.0, 180, 500.0, 4)).unwrap();
        assert_eq!(
            (grid.beats_per_bar, grid.downbeat_index),
            (Some(4), Some(0))
        );
    }

    #[test]
    fn a_waltz_is_recognised_as_three() {
        let grid = measure(&groove(150.0, 180, 250.0, 3)).unwrap();
        assert_eq!(grid.beats_per_bar, Some(3));
    }

    #[test]
    fn a_tempo_change_comes_back_as_two_segments() {
        let grid = measure(&joined(&[
            groove(128.0, 90, 500.0, 4),
            groove(140.0, 90, 0.0, 4),
        ]))
        .unwrap();
        assert_eq!(grid.segments.len(), 2, "{:?}", grid.segments);
        assert!(
            (127.5..=128.5).contains(&grid.segments[0].bpm),
            "{:?}",
            grid.segments
        );
        assert!(
            (139.5..=140.5).contains(&grid.segments[1].bpm),
            "{:?}",
            grid.segments
        );
        assert!(
            grid.segments.iter().all(|s| s.residual_ms <= 15.0),
            "{:?}",
            grid.segments
        );
    }

    #[test]
    fn segments_are_contiguous_and_cover_the_whole_track() {
        let grid = measure(&joined(&[
            groove(128.0, 90, 500.0, 4),
            groove(140.0, 90, 0.0, 4),
        ]))
        .unwrap();
        assert_eq!(grid.segments[0].start_ms, 0.0);
        assert!(grid.segments.last().unwrap().end_ms >= 179_000.0);
        for pair in grid.segments.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms);
        }
    }

    /// An intro of pure atmosphere is loud, but nothing in it starts.
    #[test]
    fn a_blend_does_not_begin_in_an_intro_that_is_only_atmosphere() {
        let grid = measure(&joined(&[pad(20), groove(128.0, 150, 0.0, 4)])).unwrap();
        let (mix_in, mix_out) = (grid.mix_in_ms.unwrap(), grid.mix_out_ms.unwrap());
        assert!(
            mix_in >= 18_000.0,
            "a blend would have started at {mix_in} ms, inside the pad"
        );
        assert!(
            mix_in <= 30_000.0,
            "a blend wouldn't start until {mix_in} ms"
        );
        assert!(mix_out > mix_in);
    }

    /// Beats the tracker chains through a pad are dropped before anything is fitted.
    #[test]
    fn beats_imagined_in_an_intro_do_not_become_a_segment_of_their_own() {
        let grid = measure(&joined(&[pad(20), groove(128.0, 150, 0.0, 4)])).unwrap();
        let [segment] = grid.segments[..] else {
            panic!("{:?}", grid.segments);
        };
        assert!((127.9..=128.1).contains(&segment.bpm), "{segment:?}");
        assert!(segment.residual_ms <= 12.0, "{segment:?}");
        assert_eq!(segment.start_ms, 0.0);
        assert!(segment.end_ms >= 169_000.0);
    }

    #[test]
    fn mix_points_land_on_bar_lines() {
        let grid = measure(&groove(128.0, 180, 500.0, 4)).unwrap();
        for point in [grid.mix_in_ms.unwrap(), grid.mix_out_ms.unwrap()] {
            let segment = grid
                .segments
                .iter()
                .find(|s| point >= s.start_ms && point <= s.end_ms)
                .unwrap();
            let beats = (point - segment.anchor_ms) / (60_000.0 / segment.bpm);
            assert!(
                (beats - beats.round()).abs() < 0.02,
                "{point} ms is {beats:.2} beats from the anchor"
            );
            assert!(
                beats.round() % grid.beats_per_bar.unwrap() as f64 == 0.0,
                "{point} ms isn't a bar line"
            );
        }
    }

    #[test]
    fn something_with_no_beat_has_no_grid() {
        let tone = stereo(SAMPLE_RATE as usize * 60, |frame| {
            0.4 * (2.0 * std::f64::consts::PI * 220.0 * frame as f64 / RATE).sin()
        });
        assert_eq!(measure(&tone), None);
    }

    #[test]
    fn silence_has_no_grid() {
        assert_eq!(
            measure(&vec![0.0; SAMPLE_RATE as usize * 60 * CHANNELS]),
            None
        );
    }
}
