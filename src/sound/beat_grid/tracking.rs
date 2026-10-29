//! The beat tracker: the chain of onsets with the most combined strength, less a penalty for every
//! interval off the expected period.

use super::*;

/// The beats through an onset signal (local mean removed), with `period` hops expected
/// between them: indices, ascending; empty when nothing could be tracked.
pub(super) fn track(onsets: &[f64], period: f64) -> Vec<usize> {
    let count = onsets.len();
    if count < 4 || period < 2.0 || period * 4.0 > count as f64 {
        return Vec::new();
    }
    let strength = normalised(onsets);
    // From half the period back to twice it; outside that is a different tempo, not a wobble.
    let earliest = ((period / 2.0).floor() as usize).max(1);
    let latest = ((period * 2.0).ceil() as usize).max(earliest + 1);
    let mut score = vec![0.0; count];
    let mut previous: Vec<Option<usize>> = vec![None; count];
    for t in 0..count {
        let mut best = f64::NEG_INFINITY;
        let mut from = None;
        for back in earliest..=latest {
            let Some(candidate) = t.checked_sub(back) else {
                break;
            };
            let departure = (back as f64 / period).ln();
            let here = score[candidate] - TIGHTNESS * departure * departure;
            if here > best {
                best = here;
                from = Some(candidate);
            }
        }
        match from {
            // Too early to have a predecessor: a possible first beat.
            None => score[t] = strength[t],
            Some(_) => score[t] = strength[t] + best,
        }
        previous[t] = from;
    }
    // The chain ends near the end of the track: the score only grows, so an early maximum
    // would mean the tracker gave up.
    let mut last = None;
    let mut highest = f64::NEG_INFINITY;
    let start = count.saturating_sub(latest);
    for (offset, &value) in score[start..].iter().enumerate() {
        if value > highest {
            highest = value;
            last = Some(start + offset);
        }
    }
    let mut beats = Vec::new();
    let mut at = last;
    while let Some(t) = at {
        beats.push(t);
        at = previous[t];
    }
    beats.reverse();
    beats
}

/// The onset strength on a scale the penalty can be balanced against.
pub(super) fn normalised(onsets: &[f64]) -> Vec<f64> {
    let n = onsets.len() as f64;
    let mean = onsets.iter().sum::<f64>() / n;
    let deviation = (onsets.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n).sqrt();
    let scale = if deviation > 1e-12 {
        1.0 / deviation
    } else {
        0.0
    };
    onsets.iter().map(|v| (v - mean) * scale).collect()
}
