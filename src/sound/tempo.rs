//! A track's tempo, how far to believe it, and whether it holds: the sidecar's estimator
//! (`TempoAnalyzer.cs`), ported line for line. [`TempoScan`] turns decoded samples into an
//! onset-strength signal (spectral flux over log-spaced bands, rises only, Hann-tapered);
//! [`estimate`] autocorrelates it over the lags of plausible tempi, prefers tempi near 120,
//! walks the winner down to the shortest period that explains the signal as well, and
//! reports how far its peak stands above a typical lag as the confidence. Stability measures
//! the tempo again over 20-second windows and counts how many agree.
//!
//! Where the original rounds with C#'s `Math.Round`, halves go to even here too.

use super::Consumer;
use super::fft::Fft;

mod scanning;
mod shaping;

pub use scanning::*;
pub use shaping::*;

/// Slowest tempo considered.
const MIN_BPM: f64 = 60.0;
/// Fastest tempo considered.
const MAX_BPM: f64 = 200.0;
/// Not reported to clients below this confidence.
pub const MINIMUM_CONFIDENCE: f64 = 0.5;
/// Where the octave preference is centred.
const CENTER_BPM: f64 = 120.0;
/// Width of that preference, in octaves.
const OCTAVE_WIDTH: f64 = 0.9;
/// A peak this far above a typical lag is full confidence.
const FULL_CONFIDENCE_PROMINENCE: f64 = 0.30;
/// How much of the winning lag's correlation the half-lag must reach to be the real period.
const HALF_LAG_SHARE: f64 = 0.9;
/// How strong the onsets must be, in log units per band, before anything counts as one.
const MINIMUM_ONSET_STRENGTH: f64 = 0.02;
/// Shorter than this and there isn't enough signal to autocorrelate.
const MINIMUM_SECONDS: f64 = 10.0;
/// Window over which the onset signal's local mean is removed.
const LOCAL_MEAN_SECONDS: f64 = 1.5;
/// Window over which the onset signal is levelled: longer than the mean window, so it
/// flattens one passage against another but not the beat itself.
const LEVELLING_SECONDS: f64 = 6.0;
/// Lags correlated either side of the reported range, never chosen: a peak at the edge needs
/// a neighbour to interpolate against.
const LAG_PADDING: usize = 4;
/// Length of each window the tempo is measured again over, for stability.
const STABILITY_WINDOW_SECONDS: f64 = 20.0;
/// How far a window's tempo may sit from the whole track's and still agree: half a per cent.
const STABILITY_TOLERANCE: f64 = 0.005;

/// A track's tempo, and how sure and how steady it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    /// Beats a minute.
    pub bpm: f64,
    /// Zero to one.
    pub confidence: f64,
    /// How much of the track keeps that tempo, zero to one; None when too short to tell.
    pub stability: Option<f64>,
}

/// C#'s `(int)Math.Round(x)`: halves to even.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The tempo of an onset-strength signal of `rate` values a second, or None when it carries
/// no usable periodicity.
pub fn estimate(onsets: &[f32], rate: f64) -> Option<Tempo> {
    let whole = estimate_range(onsets, rate)?;
    Some(Tempo {
        stability: stability(onsets, rate, whole.bpm),
        ..whole
    })
}

fn estimate_range(onsets: &[f32], rate: f64) -> Option<Tempo> {
    if (onsets.len() as f64) < MINIMUM_SECONDS * rate {
        return None;
    }
    let centred = local_mean_removed(onsets, round(LOCAL_MEAN_SECONDS * rate) as usize);
    // Asked of the onsets as detected, before levelling (which divides by a floor and would
    // turn a silent track's noise into something like signal): does anything ever start?
    let raw: f64 = centred.iter().map(|v| v * v).sum();
    if (raw / centred.len() as f64).sqrt() < MINIMUM_ONSET_STRENGTH {
        return None;
    }
    let strength = smoothed(&levelled(centred, round(LEVELLING_SECONDS * rate) as usize));
    let variance = strength.iter().map(|v| v * v).sum::<f64>() / strength.len() as f64;
    if variance <= 1e-12 {
        return None;
    }
    let fastest = (60.0 * rate / MAX_BPM).floor() as i64;
    let slowest = (60.0 * rate / MIN_BPM).ceil() as i64;
    let first = (fastest - LAG_PADDING as i64).max(1);
    let last = (strength.len() as i64 / 2 - 1).min(slowest + LAG_PADDING as i64);
    if first < 1 || last <= first || fastest >= last {
        return None;
    }
    let (first, last) = (first as usize, last as usize);
    let mut correlation = vec![0.0; last - first + 1];
    let mut weighted = vec![0.0; correlation.len()];
    for lag in first..=last {
        let sum: f64 = strength
            .iter()
            .zip(&strength[lag..])
            .map(|(a, b)| a * b)
            .sum();
        let r = sum / (strength.len() - lag) as f64 / variance;
        correlation[lag - first] = r;
        weighted[lag - first] = r * octave_preference(60.0 * rate / lag as f64);
    }
    // The padding is correlated but never chosen.
    let search_from = fastest as usize - first;
    let search_to = (correlation.len() - 1).min(slowest as usize - first);
    let mut best = search_from;
    for i in search_from + 1..=search_to {
        if weighted[i] > weighted[best] {
            best = i;
        }
    }
    // The prior picked which peak; where it sits is the correlation's business alone.
    let lag_at_peak = halve(
        &correlation,
        (first + best) as f64 + interpolate(&correlation, best),
        first,
        search_from,
    );
    let prominence = value_at(&correlation, lag_at_peak - first as f64)
        - median(&correlation[search_from..=search_to]);
    Some(Tempo {
        bpm: (60.0 * rate / lag_at_peak).clamp(MIN_BPM, MAX_BPM),
        confidence: (prominence / FULL_CONFIDENCE_PROMINENCE).clamp(0.0, 1.0),
        stability: None,
    })
}

/// How much of the track holds the tempo the whole averaged out to: the tempo measured again
/// over overlapping windows, counting those that agree (half and double included). None
/// under three windows, which is no evidence either way.
fn stability(onsets: &[f32], rate: f64, reference: f64) -> Option<f64> {
    let window = round(STABILITY_WINDOW_SECONDS * rate) as usize;
    let step = window / 2;
    if step < 1 || onsets.len() < window * 2 {
        return None;
    }
    let (mut windows, mut agreed) = (0, 0);
    let mut from = 0;
    while from + window <= onsets.len() {
        windows += 1;
        if let Some(here) = estimate_range(&onsets[from..from + window], rate)
            && here.confidence >= MINIMUM_CONFIDENCE
            && agrees(here.bpm, reference)
        {
            agreed += 1;
        }
        from += step;
    }
    (windows > 0).then(|| f64::from(agreed) / f64::from(windows))
}

/// Whether two tempi describe the same grid, one perhaps read at half or double the other.
fn agrees(bpm: f64, reference: f64) -> bool {
    let mut ratio = bpm / reference;
    if !ratio.is_finite() || ratio <= 0.0 {
        return false;
    }
    while ratio < 0.5f64.sqrt() {
        ratio *= 2.0;
    }
    while ratio >= 2f64.sqrt() {
        ratio /= 2.0;
    }
    ratio.log2().abs() <= (1.0 + STABILITY_TOLERANCE).log2()
}

/// Walks a peak down to the shortest lag that explains the signal nearly as well: where the
/// half-lag correlates almost as strongly, the half-lag is the period and the winner was its
/// second harmonic. Both read off the curve at their true fractional positions, and never
/// below the fastest tempo reported.
fn halve(correlation: &[f64], mut lag: f64, first: usize, floor: usize) -> f64 {
    loop {
        let half = lag / 2.0;
        if half - (first as f64) < floor as f64 {
            return lag;
        }
        if value_at(correlation, half - first as f64)
            < HALF_LAG_SHARE * value_at(correlation, lag - first as f64)
        {
            return lag;
        }
        lag = half;
    }
}

/// The curve's value at a fractional index, by the parabola through its nearest samples.
fn value_at(values: &[f64], index: f64) -> f64 {
    let whole = round(index);
    if whole <= 0 || whole >= values.len() as i64 - 1 {
        return values[whole.clamp(0, values.len() as i64 - 1) as usize];
    }
    let w = whole as usize;
    let offset = index - whole as f64;
    let curvature = (values[w - 1] - 2.0 * values[w] + values[w + 1]) / 2.0;
    let slope = (values[w + 1] - values[w - 1]) / 2.0;
    curvature * offset * offset + slope * offset + values[w]
}

/// How much a tempo is preferred over the same tempo halved or doubled.
fn octave_preference(bpm: f64) -> f64 {
    let octaves = (bpm / CENTER_BPM).log2() / OCTAVE_WIDTH;
    (-0.5 * octaves * octaves).exp()
}

/// Sub-lag position of a peak, by the parabola through it and its neighbours.
fn interpolate(values: &[f64], peak: usize) -> f64 {
    if peak == 0 || peak >= values.len() - 1 {
        return 0.0;
    }
    let denominator = values[peak - 1] - 2.0 * values[peak] + values[peak + 1];
    if denominator.abs() < 1e-12 {
        return 0.0;
    }
    (0.5 * (values[peak - 1] - values[peak + 1]) / denominator).clamp(-1.0, 1.0)
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

#[cfg(test)]
pub(super) mod tests {
    //! The sidecar's `TempoAnalyzerTests`, through [`TempoScan`] so the hop size and the
    //! compression are under test too. Noise comes from a seeded generator of our own, not
    //! .NET's `Random`, so the samples differ; every assertion has the original's tolerance.

    use super::*;

    const SAMPLE_RATE: u32 = 44_100;
    const RATE: f64 = SAMPLE_RATE as f64;
    const CHANNELS: usize = 2;

    /// SplitMix64: seeded and repeatable.
    pub struct Random(u64);

    impl Random {
        pub fn new(seed: u64) -> Self {
            Self(seed)
        }

        /// In [0, 1).
        pub fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) as f64 / (1u64 << 63) as f64 / 2.0
        }

        fn signed(&mut self) -> f64 {
            self.next() * 2.0 - 1.0
        }
    }

    fn measure(samples: &[f32]) -> Option<Tempo> {
        let mut scan = TempoScan::new(CHANNELS, SAMPLE_RATE);
        scan.feed(samples);
        scan.result()
    }

    fn stereo(frames: usize, mut value: impl FnMut(usize) -> f64) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                let v = value(frame) as f32;
                [v, v]
            })
            .collect()
    }

    /// Percussive hits at a fixed interval, decaying, over a quiet floor.
    fn beats(bpm: f64, seconds: usize) -> Vec<f32> {
        let period = 60.0 / bpm * RATE;
        let mut random = Random::new(7);
        stereo(SAMPLE_RATE as usize * seconds, |frame| {
            let since = frame as f64 % period;
            let hit = (-since / (RATE * 0.04)).exp() * 0.8 * random.signed();
            hit + random.signed() * 0.002
        })
    }

    /// Beats whose spacing slides from one tempo to another, as a band's does.
    fn ramp(from_bpm: f64, to_bpm: f64, seconds: usize) -> Vec<f32> {
        let frames = SAMPLE_RATE as usize * seconds;
        let mut random = Random::new(7);
        let (mut phase, mut since_beat) = (0.0, 0.0);
        stereo(frames, |frame| {
            let bpm = from_bpm + (to_bpm - from_bpm) * frame as f64 / frames as f64;
            phase += bpm / 60.0 / RATE;
            if phase >= 1.0 {
                phase -= 1.0;
                since_beat = 0.0;
            }
            let envelope = (-since_beat / (RATE * 0.05)).exp();
            let tone = 0.7 * (2.0 * std::f64::consts::PI * 70.0 * frame as f64 / RATE).sin();
            since_beat += 1.0;
            0.5 * envelope * (tone + 0.3 * random.signed())
        })
    }

    fn confident(tempo: &Tempo) -> bool {
        tempo.confidence >= MINIMUM_CONFIDENCE
    }

    /// The whole range, including the tempi that once broke it (185 read as 92; 198 pinned to
    /// the edge of the search).
    #[test]
    fn a_steady_beat_is_found_to_within_a_fifth_of_a_beat_per_minute() {
        for bpm in [
            62.0, 75.0, 90.0, 100.0, 120.0, 128.0, 140.0, 155.0, 170.0, 185.0, 198.0,
        ] {
            let tempo = measure(&beats(bpm, 60)).unwrap();
            assert!(
                (tempo.bpm - bpm).abs() <= 0.2,
                "expected about {bpm}, measured {tempo:?}"
            );
            assert!(confident(&tempo), "{bpm}: {tempo:?}");
        }
    }

    /// A beat every half second is 120 BPM, not the 60 that correlates exactly as well.
    #[test]
    fn the_faster_reading_wins_when_both_explain_the_beat_equally() {
        let tempo = measure(&beats(120.0, 40)).unwrap();
        assert!((119.5..=120.5).contains(&tempo.bpm), "{tempo:?}");
    }

    #[test]
    fn noise_has_no_tempo_worth_reporting() {
        let mut random = Random::new(1);
        let samples: Vec<f32> = (0..SAMPLE_RATE as usize * 40 * CHANNELS)
            .map(|_| (random.signed() * 0.2) as f32)
            .collect();
        let tempo = measure(&samples);
        assert!(tempo.is_none_or(|t| !confident(&t)), "{tempo:?}");
    }

    /// A held tone's energy on a fixed grid wobbles with its own frequency: a 220 Hz drone
    /// read as a confident 120 BPM before the window was tapered.
    #[test]
    fn a_held_tone_has_no_tempo_at_all() {
        for frequency in [55.0, 110.0, 220.0, 440.0, 1000.0] {
            let samples = stereo(SAMPLE_RATE as usize * 40, |frame| {
                0.4 * (2.0 * std::f64::consts::PI * frequency * frame as f64 / RATE).sin()
            });
            let tempo = measure(&samples);
            assert!(
                tempo.is_none_or(|t| !confident(&t)),
                "{frequency} Hz: {tempo:?}"
            );
        }
    }

    #[test]
    fn silence_has_no_tempo_at_all() {
        assert_eq!(
            measure(&vec![0.0; SAMPLE_RATE as usize * 40 * CHANNELS]),
            None
        );
    }

    /// Ten seconds is the floor: a jingle has no tempo to order by.
    #[test]
    fn something_too_short_to_autocorrelate_reports_nothing() {
        assert_eq!(measure(&beats(120.0, 6)), None);
    }

    /// Thirty seconds of loud sung syllables at 800 ms inside three and a half minutes of
    /// 128 BPM once came back as 73.
    #[test]
    fn a_loud_passage_does_not_decide_the_tempo_of_the_whole_track() {
        let mut samples = beats(128.0, 210);
        for frame in 0..samples.len() / CHANNELS {
            let ms = frame as f64 * 1000.0 / RATE;
            if !(150_000.0..180_000.0).contains(&ms) {
                continue;
            }
            let into = (ms - 150_000.0) % 800.0;
            if into >= 560.0 {
                continue;
            }
            let shape = (into / 60.0).min(1.0) * ((560.0 - into) / 80.0).min(1.0);
            let value =
                shape * 0.9 * (2.0 * std::f64::consts::PI * 300.0 * frame as f64 / RATE).sin();
            for channel in 0..CHANNELS {
                let at = frame * CHANNELS + channel;
                samples[at] = (f64::from(samples[at]) + value).clamp(-1.0, 1.0) as f32;
            }
        }
        let tempo = measure(&samples).unwrap();
        assert!((tempo.bpm - 128.0).abs() <= 1.0, "pulled to {tempo:?}");
    }

    /// A beat under a chorus ten times as loud as the verse.
    #[test]
    fn a_beat_survives_a_change_in_level() {
        let mut samples = beats(128.0, 40);
        for (i, sample) in samples.iter_mut().enumerate() {
            if i / CHANNELS <= SAMPLE_RATE as usize * 20 {
                *sample *= 0.1;
            }
        }
        let tempo = measure(&samples).unwrap();
        assert!((125.0..=131.0).contains(&tempo.bpm), "{tempo:?}");
        assert!(confident(&tempo), "{tempo:?}");
    }

    /// A loud bass note that never stops, and the beat a quiet tick in the top octaves:
    /// invisible in the total energy, obvious band by band.
    #[test]
    fn a_beat_carried_only_by_quiet_high_frequencies_is_still_found() {
        let period = 60.0 / 128.0 * RATE;
        let mut random = Random::new(11);
        let samples = stereo(SAMPLE_RATE as usize * 40, |frame| {
            let bass = 0.5 * (2.0 * std::f64::consts::PI * 55.0 * frame as f64 / RATE).sin();
            let since = frame as f64 % period;
            bass + (-since / (RATE * 0.008)).exp() * 0.05 * random.signed()
        });
        let tempo = measure(&samples).unwrap();
        assert!((127.3..=128.7).contains(&tempo.bpm), "{tempo:?}");
        assert!(confident(&tempo), "{tempo:?}");
    }

    #[test]
    fn a_track_that_never_changes_tempo_is_completely_stable() {
        assert_eq!(measure(&beats(128.0, 180)).unwrap().stability, Some(1.0));
    }

    /// Including a four per cent drift that still reports a confident tempo: exactly the
    /// track a fixed grid would ruin.
    #[test]
    fn a_track_that_drifts_is_not_stable_however_confident_the_tempo_is() {
        for (from, to, ceiling) in [
            (120.0, 125.0, 0.6),
            (110.0, 130.0, 0.2),
            (105.0, 145.0, 0.1),
        ] {
            let stability = measure(&ramp(from, to, 180)).unwrap().stability.unwrap();
            assert!(
                stability < ceiling,
                "{from} to {to}: stability {stability:.2}"
            );
        }
    }

    /// Under three windows there's nothing to compare.
    #[test]
    fn a_track_too_short_to_compare_windows_reports_no_stability() {
        assert_eq!(measure(&beats(128.0, 30)).unwrap().stability, None);
    }
}
