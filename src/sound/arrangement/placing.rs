//! Where sections begin: phrase length and offset, and boundaries moved to a phrase or bar line.

use super::*;

/// How many bars a phrase runs to and where one begins: each length and offset scored by how
/// much of the novelty lands on the bar lines it predicts. Nothing standing out is a real
/// answer, for a live recording or anything through-composed.
pub(super) fn phrase(
    novelty: &[f64],
    beats: &Tracked,
    from: usize,
    grid: &BeatGrid,
) -> (Option<i64>, Option<f64>) {
    let Some(beats_per_bar) = beats.beats_per_bar.filter(|_| !grid.segments.is_empty()) else {
        return (None, None);
    };
    // (bar, novelty, time) of every downbeat; C#'s division truncates, as Rust's does.
    let downbeats: Vec<(i64, f64, f64)> = novelty
        .iter()
        .enumerate()
        .filter_map(|(i, &value)| {
            let number = beats.numbers[from + i];
            ((number - beats.phase).rem_euclid(beats_per_bar) == 0).then(|| {
                (
                    (number - beats.phase) / beats_per_bar,
                    value,
                    beats.times_ms[from + i],
                )
            })
        })
        .collect();
    if (downbeats.len() as i64) < PHRASE_LENGTHS[0] * 3 {
        return (None, None);
    }
    let average = downbeats.iter().map(|d| d.1).sum::<f64>() / downbeats.len() as f64;
    if average.abs() < 1e-9 {
        return (None, None);
    }
    let (mut best_bars, mut best_anchor, mut best_score) = (None, None, 0.0);
    for bars in PHRASE_LENGTHS {
        for offset in 0..bars {
            let on: Vec<&(i64, f64, f64)> = downbeats
                .iter()
                .filter(|d| (d.0 - offset).rem_euclid(bars) == 0)
                .collect();
            if on.len() < 3 {
                continue;
            }
            let mean = on.iter().map(|d| d.1).sum::<f64>() / on.len() as f64;
            let contrast = (mean - average) / average.abs();
            let required = if best_bars.is_none() {
                best_score
            } else {
                best_score * LONGER_PHRASE_MARGIN
            };
            if contrast > required {
                best_score = contrast;
                best_bars = Some(bars);
                best_anchor = Some(on[0].2);
            }
        }
    }
    if best_score >= PHRASE_CONTRAST {
        (best_bars, best_anchor.map(|a| round_to(a, 1)))
    } else {
        (None, None)
    }
}

/// The beats where the track changes, each moved to a phrase line where one is close and to
/// a bar line otherwise.
pub(super) fn boundaries(
    novelty: &[f64],
    beats: &Tracked,
    from: usize,
    to: usize,
    phrase_bars: Option<i64>,
    phrase_anchor_ms: Option<f64>,
) -> Vec<usize> {
    let n = novelty.len() as f64;
    let mean = novelty.iter().sum::<f64>() / n;
    let deviation = (novelty.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n).sqrt();
    let floor = mean + BOUNDARY_THRESHOLD * deviation;
    let mut picked = vec![0];
    for i in KERNEL_BEATS..novelty.len().saturating_sub(KERNEL_BEATS) {
        if novelty[i] < floor
            || novelty[i] < novelty[i - 1]
            || novelty[i] < novelty[i + 1]
            || i - picked.last().expect("picked starts with 0 and only grows")
                < MINIMUM_SECTION_BEATS
        {
            continue;
        }
        picked.push(i);
    }
    let Some(beats_per_bar) = beats.beats_per_bar else {
        return picked;
    };
    let mut anchor_number = beats.phase;
    if let Some(anchor_ms) = phrase_anchor_ms {
        let mut nearest = 0;
        for i in from..=to {
            if (beats.times_ms[i] - anchor_ms).abs()
                < (beats.times_ms[nearest + from] - anchor_ms).abs()
            {
                nearest = i - from;
            }
        }
        anchor_number = beats.numbers[from + nearest];
    }
    let phrase_stride = phrase_bars.map_or(0, |bars| beats_per_bar * bars);
    let mut snapped: Vec<usize> = Vec::new();
    for at in picked {
        let mut best = at;
        if phrase_stride > 0 {
            best = nearest(
                novelty.len(),
                beats,
                from,
                at,
                phrase_stride,
                anchor_number,
                beats_per_bar * PHRASE_NUDGE_BARS,
            );
        }
        if best == at {
            best = nearest(
                novelty.len(),
                beats,
                from,
                at,
                beats_per_bar,
                beats.phase,
                beats_per_bar,
            );
        }
        if snapped
            .last()
            .is_none_or(|&last| best >= last + MINIMUM_SECTION_BEATS)
        {
            snapped.push(best);
        }
    }
    // The first section starts the track. Where snapping moved its start onto a bar line a
    // beat or two in, the sidecar put 0 back in front and so made a section shorter than a
    // bar; here that first boundary becomes 0 instead.
    match snapped.first() {
        Some(&first) if first < MINIMUM_SECTION_BEATS => snapped[0] = 0,
        _ => snapped.insert(0, 0),
    }
    snapped
}

/// The nearest beat to `at` on a line of `stride` beats from `anchor_number`, within
/// `reach`; `at` itself when none is.
pub(super) fn nearest(
    count: usize,
    beats: &Tracked,
    from: usize,
    at: usize,
    stride: i64,
    anchor_number: i64,
    reach: i64,
) -> usize {
    let reach_beats = reach as usize;
    let mut best = at;
    let mut best_distance = reach_beats + 1;
    for i in at.saturating_sub(reach_beats)..=(count - 1).min(at + reach_beats) {
        if (beats.numbers[from + i] - anchor_number).rem_euclid(stride) != 0 {
            continue;
        }
        let distance = i.abs_diff(at);
        if distance < best_distance {
            best_distance = distance;
            best = i;
        }
    }
    best
}

pub(super) fn mean(values: &[f64], from: usize, to: usize) -> f64 {
    let to = to.min(values.len());
    if from >= to {
        return 0.0;
    }
    values[from..to].iter().sum::<f64>() / (to - from) as f64
}
