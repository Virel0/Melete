//! Each beat of the grid described by its timbre, level and density.

use super::*;

/// Each beat from `from` to `to` as (start, end) in ms; the last runs half a second.
pub(super) fn beat_spans(beats: &Tracked, from: usize, to: usize) -> Vec<(f64, f64)> {
    (from..=to)
        .map(|i| {
            let start = beats.times_ms[i];
            let end = beats.times_ms.get(i + 1).copied().unwrap_or(start + 500.0);
            (start, end)
        })
        .collect()
}

/// The frames from `start` to `end` ms at `rate` a second, within `count`.
pub(super) fn frames(start: f64, end: f64, rate: f64, count: usize) -> std::ops::Range<usize> {
    let first = ((start * rate / 1000.0).floor() as i64).max(0) as usize;
    let last = ((end * rate / 1000.0).ceil() as i64).clamp(0, count as i64) as usize;
    first..last.max(first)
}

/// One vector per beat of what the spectrum was made of. Not normalised to unit length:
/// level is most of what separates a breakdown from the drop either side of it.
pub(super) fn per_beat(timbre: &[Vec<f32>], rate: f64, spans: &[(f64, f64)]) -> Vec<Vec<f64>> {
    let bands = timbre[0].len();
    spans
        .iter()
        .map(|&(start, end)| {
            let mut vector = vec![0.0; bands];
            let range = frames(start, end, rate, timbre.len());
            let count = range.len();
            for frame in &timbre[range] {
                for (total, &value) in vector.iter_mut().zip(frame) {
                    *total += f64::from(value);
                }
            }
            if count > 0 {
                vector.iter_mut().for_each(|v| *v /= count as f64);
            }
            vector
        })
        .collect()
}

/// How loud each beat was, before any normalising.
pub(super) fn per_beat_level(timbre: &[Vec<f32>], rate: f64, spans: &[(f64, f64)]) -> Vec<f64> {
    spans
        .iter()
        .map(|&(start, end)| {
            let range = frames(start, end, rate, timbre.len());
            let count = range.len();
            let sum: f64 = timbre[range].iter().flatten().map(|&v| f64::from(v)).sum();
            if count > 0 { sum / count as f64 } else { 0.0 }
        })
        .collect()
}

/// How much was starting during each beat.
pub(super) fn per_beat_density(
    onsets: &[f32],
    rate: f64,
    first_onset_ms: f64,
    spans: &[(f64, f64)],
) -> Vec<f64> {
    spans
        .iter()
        .map(|&(start, end)| {
            let range = frames(
                start - first_onset_ms,
                end - first_onset_ms,
                rate,
                onsets.len(),
            );
            let count = range.len();
            let sum: f64 = onsets[range].iter().map(|&v| f64::from(v)).sum();
            if count > 0 { sum / count as f64 } else { 0.0 }
        })
        .collect()
}
