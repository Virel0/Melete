//! The onset signal as the autocorrelation sees it: its local mean removed, levelled so every
//! passage gets an equal say, and smoothed.

use super::*;

/// A whole onset signal less its local mean, as the autocorrelation sees it: what the beat
/// tracker wants too.
pub fn centred(onsets: &[f32], rate: f64) -> Vec<f64> {
    local_mean_removed(onsets, round(LOCAL_MEAN_SECONDS * rate) as usize)
}

/// The onset signal less its local mean (prefix sums, so the window costs nothing).
pub(super) fn local_mean_removed(onsets: &[f32], window: usize) -> Vec<f64> {
    if window < 2 {
        return onsets.iter().map(|&v| f64::from(v)).collect();
    }
    let mut prefix = vec![0.0; onsets.len() + 1];
    for (i, &v) in onsets.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(v);
    }
    let half = window / 2;
    (0..onsets.len())
        .map(|i| {
            let low = i.saturating_sub(half);
            let high = onsets.len().min(i + half + 1);
            f64::from(onsets[i]) - (prefix[high] - prefix[low]) / (high - low) as f64
        })
        .collect()
}

/// Levels the signal so every passage gets an equal say: autocorrelation weights by energy,
/// and a short loud stretch would otherwise decide the tempo of a whole track. The floor keeps
/// a silent stretch from being amplified into noise that looks like signal.
pub(super) fn levelled(strength: Vec<f64>, window: usize) -> Vec<f64> {
    if window < 2 || strength.is_empty() {
        return strength;
    }
    let mut squares = vec![0.0; strength.len() + 1];
    for (i, v) in strength.iter().enumerate() {
        squares[i + 1] = squares[i] + v * v;
    }
    let overall = (squares[strength.len()] / strength.len() as f64).sqrt();
    let floor = overall * 0.25;
    if floor <= 1e-12 {
        return strength;
    }
    let half = window / 2;
    (0..strength.len())
        .map(|i| {
            let low = i.saturating_sub(half);
            let high = strength.len().min(i + half + 1);
            let local = ((squares[high] - squares[low]) / (high - low) as f64).sqrt();
            strength[i] / local.max(floor) * overall
        })
        .collect()
}

/// Spreads each onset over its neighbours with a narrow Gaussian, so a beat period that
/// isn't a whole number of hops isn't read far too low at whole lags.
pub(super) fn smoothed(strength: &[f64]) -> Vec<f64> {
    const KERNEL: [f64; 5] = [0.0545, 0.2442, 0.4026, 0.2442, 0.0545];
    (0..strength.len() as i64)
        .map(|i| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for (k, w) in KERNEL.iter().enumerate() {
                let at = i + k as i64 - 2;
                if at >= 0 && (at as usize) < strength.len() {
                    sum += strength[at as usize] * w;
                    weight += w;
                }
            }
            sum / weight
        })
        .collect()
}
