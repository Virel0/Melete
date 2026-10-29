//! The meter, read from the low bands, and the mix points: the first and last four-bar phrases
//! where every bar carries the groove.

use super::*;

/// How many beats to a bar and which comes first, from the low bands. Four is preferred;
/// three must beat it by a clear margin, and when neither stands out the meter isn't known.
pub(super) fn meter(tracked: &[usize], numbers: &[i64], low_onsets: &[f32]) -> Option<Meter> {
    if low_onsets.is_empty() {
        return None;
    }
    let strength: Vec<f64> = tracked
        .iter()
        .map(|&t| low_onsets.get(t).map_or(0.0, |&v| f64::from(v)))
        .collect();
    let mean = strength.iter().sum::<f64>() / tracked.len() as f64;
    if mean <= 1e-9 {
        return None;
    }
    let mut best: Option<Meter> = None;
    let mut best_contrast = 0.0;
    for beats_per_bar in METERS {
        let bar = beats_per_bar as usize;
        let mut sums = vec![0.0; bar];
        let mut counts = vec![0usize; bar];
        for (i, s) in strength.iter().enumerate() {
            let phase = numbers[i].rem_euclid(beats_per_bar) as usize;
            sums[phase] += s;
            counts[phase] += 1;
        }
        let mut strongest = 0;
        for phase in 1..bar {
            if counts[phase] > 0
                && sums[phase] / counts[phase] as f64
                    > sums[strongest] / counts[strongest].max(1) as f64
            {
                strongest = phase;
            }
        }
        if counts[strongest] == 0 {
            continue;
        }
        let contrast = (sums[strongest] / counts[strongest] as f64 - mean) / mean;
        let required = if best.is_none() {
            MINIMUM_DOWNBEAT_CONTRAST
        } else {
            best_contrast * WALTZ_MARGIN
        };
        if contrast >= MINIMUM_DOWNBEAT_CONTRAST && contrast >= required {
            best = Some(Meter {
                beats_per_bar,
                phase: strongest as i64,
            });
            best_contrast = contrast;
        }
    }
    best
}

/// Where a blend may bring the track in and take it out: the first and last downbeats with a
/// four-bar phrase in which every bar carries the groove. Judged on onsets, not loudness, so
/// a loud intro of pure pad isn't playing yet.
pub(super) fn mix_points(
    tracked: &[usize],
    times: &[f64],
    numbers: &[i64],
    onsets: &[f32],
    segments: &[Segment],
    meter: Option<Meter>,
) -> (Option<f64>, Option<f64>) {
    let Some(meter) = meter.filter(|_| tracked.len() >= MINIMUM_SEGMENT_BEATS) else {
        return (None, None);
    };
    let strength = strength(tracked, onsets);
    let median = median_of(&strength);
    if median <= 1e-9 {
        return (None, None);
    }
    let bar = meter.beats_per_bar as usize;
    let phrase = bar * 4;
    let floor = median * GROOVE_SHARE;
    // Every bar, not the phrase on average: a strong second half mustn't carry a silent first.
    let grooving = |at: usize| (0..4).all(|b| mean(&strength, at + b * bar, bar) >= floor);
    let first = (0..strength.len())
        .take_while(|&i| i + phrase <= strength.len())
        .find(|&i| meter.is_downbeat(numbers[i]) && grooving(i));
    let last = (phrase..strength.len())
        .rev()
        .find(|&i| meter.is_downbeat(numbers[i]) && grooving(i - phrase));
    match (first, last) {
        (Some(first), Some(last)) if last > first => (
            Some(snap(times[first], segments)),
            Some(snap(times[last], segments)),
        ),
        _ => (None, None),
    }
}

/// A time moved onto the nearest beat of whichever segment holds it, so a mix point is where
/// the published grid says a beat falls.
pub(super) fn snap(ms: f64, segments: &[Segment]) -> f64 {
    let segment = segments
        .iter()
        .find(|s| ms >= s.start_ms && ms <= s.end_ms)
        .unwrap_or(&segments[0]);
    let ms_per_beat = 60_000.0 / segment.bpm;
    let beats = round((ms - segment.anchor_ms) / ms_per_beat);
    round_to(segment.anchor_ms + beats * ms_per_beat, 1)
}
