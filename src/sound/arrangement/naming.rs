//! Boundaries as sections, named only where level, weight and position settle it.

use super::*;

/// Boundaries as sections, with an energy and a name where the evidence allows.
#[allow(clippy::too_many_arguments)]
pub(super) fn describe(
    boundaries: &[usize],
    beats: &Tracked,
    from: usize,
    to: usize,
    loudness: &[f64],
    density: &[f64],
    timbre: &[Vec<f32>],
    timbre_rate: f64,
    duration_ms: f64,
    grid: &BeatGrid,
) -> Vec<Section> {
    let count = to - from + 1;
    let blend = |a: usize, b: usize| 0.6 * mean(loudness, a, b) + 0.4 * mean(density, a, b);
    let mut raw = Vec::with_capacity(boundaries.len());
    let mut weight = Vec::with_capacity(boundaries.len());
    let mut rising = Vec::with_capacity(boundaries.len());
    for (i, &start) in boundaries.iter().enumerate() {
        let end = boundaries.get(i + 1).copied().unwrap_or(count);
        raw.push(blend(start, end));
        weight.push(bottom(
            timbre,
            timbre_rate,
            beats,
            from + start,
            from + (count - 1).min(end.saturating_sub(1)),
        ));
        // Climbing across itself is what makes a build a build, not just something quieter
        // before something louder.
        let middle = start + (end - start) / 2;
        rising.push(blend(middle, end) > blend(start, middle) * 1.05);
    }
    let lowest = raw.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = raw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = highest - lowest;
    let energy: Vec<f64> = raw
        .iter()
        .map(|v| {
            if span > 1e-9 {
                (v - lowest) / span
            } else {
                0.5
            }
        })
        .collect();
    let heaviest = weight.iter().copied().fold(0.0, f64::max);
    let kinds = name(&energy, &weight, &rising, heaviest);
    (0..boundaries.len())
        .map(|i| {
            // On a bar line of the published grid, which is all a client can check against.
            let start_ms = if i == 0 {
                0.0
            } else {
                bar(beats.times_ms[from + boundaries[i]], grid)
            };
            let end_ms = if i == boundaries.len() - 1 {
                duration_ms.max(beats.times_ms[to])
            } else {
                bar(beats.times_ms[from + boundaries[i + 1]], grid)
            };
            Section {
                start_ms: round_to(start_ms, 1),
                end_ms: round_to(end_ms, 1),
                kind: kinds[i],
                energy: round_to(energy[i], 2),
            }
        })
        .collect()
}

/// Names what level, weight and position settle, and admits the rest.
pub(super) fn name(energy: &[f64], weight: &[f64], rising: &[bool], heaviest: f64) -> Vec<Kind> {
    let quiet_at = |i: usize| energy[i] <= QUIET_ENERGY;
    let mut kinds: Vec<Kind> = (0..energy.len())
        .map(|i| {
            let quiet = quiet_at(i);
            let loud = energy[i] >= LOUD_ENERGY;
            let heavy = heaviest > 1e-9 && weight[i] >= heaviest * 0.8;
            let inner = i > 0 && i < energy.len() - 1;
            // Everything before the music properly starts is the intro, and everything after
            // it stops the outro: not just the first and last sections.
            if quiet && (0..=i).all(quiet_at) {
                Kind::Intro
            } else if quiet && (i..energy.len()).all(quiet_at) {
                Kind::Outro
            } else if loud && heavy {
                Kind::Drop
            } else if inner && energy[i - 1] - energy[i] > 0.15 && energy[i + 1] - energy[i] > 0.15
            {
                // Quieter than both neighbours by a clear margin: relative, because a record
                // with a narrow dynamic range still has a breakdown.
                Kind::Breakdown
            } else if quiet && inner && energy[i - 1] > energy[i] && energy[i + 1] > energy[i] {
                Kind::Breakdown
            } else {
                Kind::Unknown
            }
        })
        .collect();
    // A build sits between something quieter and something louder, and usually climbs too;
    // a breakdown sits between two louder things.
    for i in 0..kinds.len().saturating_sub(1) {
        if kinds[i] != Kind::Unknown || energy[i + 1] - energy[i] <= 0.15 {
            continue;
        }
        let not_falling_into = i == 0 || energy[i] >= energy[i - 1] - 0.05;
        if not_falling_into || rising[i] {
            kinds[i] = Kind::Build;
        }
    }
    kinds
}

/// The nearest bar line of the published grid to a moment.
pub(super) fn bar(ms: f64, grid: &BeatGrid) -> f64 {
    let Some(beats_per_bar) = grid.beats_per_bar.filter(|_| !grid.segments.is_empty()) else {
        return ms;
    };
    let segment = grid
        .segments
        .iter()
        .find(|s| ms >= s.start_ms && ms <= s.end_ms)
        .unwrap_or(&grid.segments[0]);
    let bar_ms = beats_per_bar as f64 * 60_000.0 / segment.bpm;
    let bars = ((ms - segment.anchor_ms) / bar_ms).round_ties_even();
    segment.anchor_ms + bars * bar_ms
}

/// How much of a section sits in the bottom third of the spectrum.
pub(super) fn bottom(
    timbre: &[Vec<f32>],
    rate: f64,
    beats: &Tracked,
    first_beat: usize,
    last_beat: usize,
) -> f64 {
    if timbre.is_empty() || first_beat > last_beat {
        return 0.0;
    }
    let bands = (timbre[0].len() / 3).max(1);
    let first = ((beats.times_ms[first_beat] * rate / 1000.0) as i64).max(0) as usize;
    let last = (((beats.times_ms[last_beat] * rate / 1000.0) as i64 + 1).max(0) as usize)
        .min(timbre.len());
    if first >= last {
        return 0.0;
    }
    let sum: f64 = timbre[first..last]
        .iter()
        .flat_map(|frame| &frame[..bands])
        .map(|&v| f64::from(v))
        .sum();
    sum / (last - first) as f64
}
