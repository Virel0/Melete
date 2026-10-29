//! Fitting the beats with straight lines, split where one fit can't describe a tempo change, and
//! the segments as the client sees them.

use super::*;

/// Which beats carry anything, and the first and last that do. Beats that carry nothing
/// (a breakdown, an intro of pad) are left out of the fit, which spans the gap on the beats
/// either side: exactly right when the tempo didn't change.
pub(super) fn sounding(tracked: &[usize], onsets: &[f32]) -> (Vec<bool>, usize, usize) {
    let strength = strength(tracked, onsets);
    let floor = median_of(&strength) * SILENT_SHARE;
    let carries: Vec<bool> = (0..strength.len())
        .map(|i| {
            // Judged over a bar: a real beat can land between two hits.
            let from = i.saturating_sub(2);
            mean(&strength, from, 4.min(strength.len() - from)) >= floor
        })
        .collect();
    let mut first = 0;
    while first < strength.len() - 1 && !carries[first] {
        first += 1;
    }
    let mut last = strength.len() - 1;
    while last > first && !carries[last] {
        last -= 1;
    }
    (carries, first, last)
}

/// Splits a run of beats where that helps most, and only while it helps enough, so a track
/// that changes tempo gets an honest fit for each part.
pub(super) fn split(
    times: &[f64],
    numbers: &[i64],
    carries: &[bool],
    from: usize,
    to: usize,
    depth: u32,
    fits: &mut Vec<Fit>,
) {
    let whole = least_squares(times, numbers, carries, from, to);
    if depth >= MAX_SPLIT_DEPTH
        || whole.residual_ms <= GOOD_RESIDUAL_MS
        || whole.beats < (MINIMUM_SEGMENT_BEATS * 2) as i64
    {
        fits.push(whole);
        return;
    }
    let mut best_at = None;
    let mut best_residual = f64::MAX;
    let mut at = from + MINIMUM_SEGMENT_BEATS;
    while at + MINIMUM_SEGMENT_BEATS <= to {
        let left = least_squares(times, numbers, carries, from, at);
        let right = least_squares(times, numbers, carries, at + 1, to);
        if left.beats >= MINIMUM_SEGMENT_BEATS as i64 && right.beats >= MINIMUM_SEGMENT_BEATS as i64
        {
            // Weighted by length, so a split is judged on the whole run.
            let (l, r) = (left.beats as f64, right.beats as f64);
            let combined = ((left.residual_ms * left.residual_ms * l
                + right.residual_ms * right.residual_ms * r)
                / (l + r))
                .sqrt();
            if combined < best_residual {
                best_residual = combined;
                best_at = Some(at);
            }
        }
        at += 1;
    }
    match best_at {
        Some(at) if best_residual <= whole.residual_ms * WORTH_SPLITTING => {
            split(times, numbers, carries, from, at, depth + 1, fits);
            split(times, numbers, carries, at + 1, to, depth + 1, fits);
        }
        _ => fits.push(whole),
    }
}

/// One straight line through beat number against time, over the beats that carry anything.
/// Sums are about the means: times run to hundreds of thousands of milliseconds and the
/// residual is around ten, so the textbook form would return noise.
pub(super) fn least_squares(
    times: &[f64],
    numbers: &[i64],
    carries: &[bool],
    from: usize,
    to: usize,
) -> Fit {
    let used: Vec<usize> = (from..=to).filter(|&i| carries[i]).collect();
    if used.len() < 2 {
        return Fit {
            from,
            to,
            intercept: times[from],
            ms_per_beat: 60_000.0 / 120.0,
            residual_ms: f64::MAX,
            beats: 0,
        };
    }
    let count = used.len() as f64;
    let mean_number = used.iter().map(|&i| numbers[i] as f64).sum::<f64>() / count;
    let mean_time = used.iter().map(|&i| times[i]).sum::<f64>() / count;
    let (mut covariance, mut spread) = (0.0, 0.0);
    for &i in &used {
        let dn = numbers[i] as f64 - mean_number;
        covariance += dn * (times[i] - mean_time);
        spread += dn * dn;
    }
    let mut ms_per_beat = if spread > 1e-9 {
        covariance / spread
    } else {
        0.0
    };
    if ms_per_beat <= 0.0 {
        ms_per_beat = 60_000.0 / mean_number.max(1.0);
    }
    let intercept = mean_time - ms_per_beat * mean_number;
    let squared: f64 = used
        .iter()
        .map(|&i| {
            let error = times[i] - intercept - ms_per_beat * numbers[i] as f64;
            error * error
        })
        .sum();
    Fit {
        from,
        to,
        intercept,
        ms_per_beat,
        residual_ms: (squared / count).sqrt(),
        beats: used.len() as i64,
    }
}

/// Fits as the client sees them: contiguous, covering the whole track (a client asking which
/// segment holds a moment needs an answer for every moment), and anchored on a downbeat when
/// the meter is known, so bar n is `anchor + n · beats_per_bar · 60000 / bpm` in any segment.
pub(super) fn describe(
    fits: &[Fit],
    times: &[f64],
    numbers: &[i64],
    duration_ms: f64,
    meter: Option<Meter>,
) -> Vec<Segment> {
    fits.iter()
        .enumerate()
        .map(|(i, fit)| {
            let start = if i == 0 {
                0.0
            } else {
                (times[fit.from] + times[fits[i - 1].to]) / 2.0
            };
            let end = if i == fits.len() - 1 {
                duration_ms.max(times[fit.to])
            } else {
                (times[fit.to] + times[fits[i + 1].from]) / 2.0
            };
            let mut origin = numbers[fit.from];
            if let Some(m) = meter {
                let phase = (origin - m.phase).rem_euclid(m.beats_per_bar);
                if phase != 0 {
                    origin += m.beats_per_bar - phase;
                }
            }
            Segment {
                start_ms: round_to(start, 1),
                end_ms: round_to(end, 1),
                anchor_ms: round_to(fit.intercept + fit.ms_per_beat * origin as f64, 1),
                bpm: round_to(60_000.0 / fit.ms_per_beat, 2),
                residual_ms: round_to(fit.residual_ms, 1),
                beats: fit.beats,
            }
        })
        .collect()
}
