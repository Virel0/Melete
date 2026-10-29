//! How much the track changes at each beat: the checkerboard kernel along the similarity diagonal.

use super::*;

/// How much the track changes at each beat. The similarity matrix is never built: only the
/// band the kernel covers is read.
pub(super) fn novelty(features: &[Vec<f64>]) -> Vec<f64> {
    let kernel = checkerboard(KERNEL_BEATS);
    let scale = scale(features);
    let k = KERNEL_BEATS as i64;
    let n = features.len() as i64;
    (0..n)
        .map(|centre| {
            let mut sum = 0.0;
            for i in -k..k {
                let a = centre + i;
                if !(0..n).contains(&a) {
                    continue;
                }
                for j in -k..k {
                    let b = centre + j;
                    if !(0..n).contains(&b) {
                        continue;
                    }
                    sum += kernel[(i + k) as usize][(j + k) as usize]
                        * similarity(&features[a as usize], &features[b as usize], scale);
                }
            }
            sum
        })
        .collect()
}

/// Positive where both offsets are on the same side of the centre, negative where they
/// straddle it, with the corners faded off.
pub(super) fn checkerboard(half: usize) -> Vec<Vec<f64>> {
    let h = half as i64;
    let width = half as f64 / 2.0;
    (-h..h)
        .map(|i| {
            (-h..h)
                .map(|j| {
                    let taper = (-((i * i + j * j) as f64) / (2.0 * width * width)).exp();
                    let sign = if i >= 0 { 1.0 } else { -1.0 } * if j >= 0 { 1.0 } else { -1.0 };
                    sign * taper
                })
                .collect()
        })
        .collect()
}

/// One where two beats are identical, falling away as they differ.
pub(super) fn similarity(a: &[f64], b: &[f64], scale: f64) -> f64 {
    let squared: f64 = a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum();
    (-squared / (2.0 * scale * scale)).exp()
}

/// What counts as different for this track: the median distance across four bars.
pub(super) fn scale(features: &[Vec<f64>]) -> f64 {
    const LAG: usize = 16;
    let mut distances: Vec<f64> = (0..features.len().saturating_sub(LAG))
        .map(|i| {
            features[i]
                .iter()
                .zip(&features[i + LAG])
                .map(|(x, y)| (x - y) * (x - y))
                .sum::<f64>()
                .sqrt()
        })
        .collect();
    if distances.is_empty() {
        return 1.0;
    }
    distances.sort_by(f64::total_cmp);
    distances[distances.len() / 2].max(1e-6)
}
