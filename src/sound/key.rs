//! A track's key on the Camelot wheel harmonic mixing goes by: the sidecar's `ChromaScan`,
//! ported line for line. A second, longer transform than the onsets' (186 ms windows, bins
//! 5 Hz apart, so neighbouring semitones are told apart) folds each window's spectrum into
//! twelve pitch classes; the loud windows are summed and correlated against Krumhansl and
//! Kessler's profiles of the 24 keys. It hears which notes a track leans on and nothing more,
//! so it's weakest between a key and its relative major or minor; the confidence (margin over
//! the runner-up, times how well the winner fits) is where that shows.

use super::Consumer;
use super::fft::Fft;

const SIZE: usize = 8192;
/// Below this the bins are too close together to be sure which note is which.
const LOWEST_HZ: f64 = 55.0;
/// Above this there's more harmonic than fundamental.
const HIGHEST_HZ: f64 = 2000.0;
/// Windows quieter than this share of the loudest aren't listened to.
const SILENCE_SHARE: f64 = 0.02;

const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];
/// Camelot numbers by pitch class, C first: C major is 8B, A minor 8A, the rest round the
/// circle of fifths.
const MAJOR_WHEEL: [u8; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];
const MINOR_WHEEL: [u8; 12] = [5, 12, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10];

/// A track's musical key, in Camelot notation.
#[derive(Debug, Clone, PartialEq)]
pub struct MusicalKey {
    /// Such as `8A` for A minor or `8B` for C major.
    pub camelot: String,
    /// Zero to one, to two decimals.
    pub confidence: f64,
}

/// Finds a track's key from the pitch classes in its spectrum, fed the decoded samples.
pub struct KeyScan {
    channels: usize,
    hop: usize,
    fft: Fft,
    taper: Vec<f64>,
    ring: Vec<f32>,
    real: Vec<f64>,
    imaginary: Vec<f64>,
    /// Which pitch class each bin belongs to, if any.
    class_of: Vec<Option<usize>>,
    windows: Vec<[f64; 12]>,
    write_at: usize,
    since_hop: usize,
    seen: u64,
}

impl KeyScan {
    /// A scan for interleaved samples of `channels` channels at `sample_rate` Hz.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let class_of = (0..SIZE / 2)
            .map(|bin| {
                let hz = bin as f64 * f64::from(sample_rate) / SIZE as f64;
                if !(LOWEST_HZ..=HIGHEST_HZ).contains(&hz) {
                    return None;
                }
                // MIDI note 69 is A440; the pitch class is that, modulo the octave.
                let semitones = 69.0 + 12.0 * (hz / 440.0).log2();
                Some((semitones.round_ties_even() as i64).rem_euclid(12) as usize)
            })
            .collect();
        Self {
            channels,
            hop: SIZE / 2,
            fft: Fft::new(SIZE).expect("SIZE, 8192, is a power of two"),
            taper: (0..SIZE)
                .map(|i| {
                    0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (SIZE - 1) as f64).cos())
                })
                .collect(),
            ring: vec![0.0; SIZE],
            real: vec![0.0; SIZE],
            imaginary: vec![0.0; SIZE],
            class_of,
            windows: Vec::new(),
            write_at: 0,
            since_hop: 0,
            seen: 0,
        }
    }

    /// The key, once the whole file has been fed; None without enough pitched material.
    pub fn result(&self) -> Option<MusicalKey> {
        if self.windows.len() < 8 {
            return None;
        }
        // The loud windows only: a long quiet passage would otherwise weigh as much as the
        // chorus, and a fade-out as much as the song.
        let loudest = self
            .windows
            .iter()
            .map(|w| w.iter().sum::<f64>())
            .fold(f64::NEG_INFINITY, f64::max);
        if loudest <= 1e-12 {
            return None;
        }
        let mut chroma = [0.0; 12];
        let mut used = 0;
        for window in &self.windows {
            if window.iter().sum::<f64>() < loudest * SILENCE_SHARE {
                continue;
            }
            used += 1;
            for (total, value) in chroma.iter_mut().zip(window) {
                *total += value;
            }
        }
        if used < 8 || chroma.iter().sum::<f64>() <= 1e-12 {
            return None;
        }
        let (mut best, mut runner_up) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut camelot = None;
        for root in 0..12 {
            for minor in [false, true] {
                let profile = if minor {
                    &MINOR_PROFILE
                } else {
                    &MAJOR_PROFILE
                };
                let score = correlation(&chroma, profile, root);
                if score > best {
                    runner_up = best;
                    best = score;
                    camelot = Some(if minor {
                        format!("{}A", MINOR_WHEEL[root])
                    } else {
                        format!("{}B", MAJOR_WHEEL[root])
                    });
                } else if score > runner_up {
                    runner_up = score;
                }
            }
        }
        let camelot = camelot.filter(|_| best.is_finite())?;
        // Well clear of the runner-up (a tenth of a correlation is a comfortable win), and a
        // good fit in itself: a bare drum loop still has a least-bad key.
        let margin = ((best - runner_up) / 0.1).clamp(0.0, 1.0);
        let fit = (best / 0.6).clamp(0.0, 1.0);
        Some(MusicalKey {
            camelot,
            confidence: (margin * fit * 100.0).round_ties_even() / 100.0,
        })
    }

    fn accumulate(&mut self) {
        let mut read = self.write_at;
        for i in 0..SIZE {
            self.real[i] = f64::from(self.ring[read]) * self.taper[i];
            self.imaginary[i] = 0.0;
            read = if read + 1 == SIZE { 0 } else { read + 1 };
        }
        self.fft.transform(&mut self.real, &mut self.imaginary);
        let mut window = [0.0; 12];
        for bin in 1..self.class_of.len() {
            if let Some(class) = self.class_of[bin] {
                window[class] += self.real[bin].hypot(self.imaginary[bin]);
            }
        }
        self.windows.push(window);
    }
}

impl Consumer for KeyScan {
    fn feed(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            let mono: f64 = frame.iter().map(|&v| f64::from(v)).sum();
            self.ring[self.write_at] = (mono / self.channels as f64) as f32;
            self.write_at = if self.write_at + 1 == SIZE {
                0
            } else {
                self.write_at + 1
            };
            self.seen += 1;
            self.since_hop += 1;
            if self.since_hop < self.hop {
                continue;
            }
            self.since_hop = 0;
            if self.seen >= SIZE as u64 {
                self.accumulate();
            }
        }
    }
}

/// Pearson correlation of the pitch distribution against one key's profile.
fn correlation(chroma: &[f64; 12], profile: &[f64; 12], root: usize) -> f64 {
    let mean_chroma = chroma.iter().sum::<f64>() / 12.0;
    let mean_profile = profile.iter().sum::<f64>() / 12.0;
    let (mut covariance, mut spread_chroma, mut spread_profile) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let dc = chroma[i] - mean_chroma;
        let dp = profile[(i + 12 - root) % 12] - mean_profile;
        covariance += dc * dp;
        spread_chroma += dc * dc;
        spread_profile += dp * dp;
    }
    let denominator = (spread_chroma * spread_profile).sqrt();
    if denominator <= 1e-12 {
        0.0
    } else {
        covariance / denominator
    }
}

#[cfg(test)]
mod tests {
    //! The sidecar's `ChromaScanTests`, over progressions whose key isn't in question.

    use super::super::tempo::tests::Random;
    use super::*;

    const SAMPLE_RATE: u32 = 44_100;
    const CHANNELS: usize = 2;

    fn measure(samples: &[f32]) -> Option<MusicalKey> {
        let mut scan = KeyScan::new(CHANNELS, SAMPLE_RATE);
        scan.feed(samples);
        scan.result()
    }

    /// 1.5 s a chord, each a triad with its octave harmonics, tonic first and last; minor
    /// chords on the tonic and subdominant of a minor key, major on its dominant.
    fn progression(roots: [i32; 4], minor: bool) -> Vec<f32> {
        let chord_frames = (f64::from(SAMPLE_RATE) * 1.5) as usize;
        let frames = chord_frames * roots.len() * 8;
        (0..frames)
            .flat_map(|frame| {
                let chord = (frame / chord_frames) % roots.len();
                let third = if minor && chord != 2 { 3 } else { 4 };
                let mut value = 0.0;
                for semitone in [0, third, 7] {
                    // 261.63 Hz is middle C, so pitch class 0 really is C.
                    let hz = 261.626 * 2f64.powf(f64::from(roots[chord] + semitone) / 12.0);
                    let t = frame as f64 / f64::from(SAMPLE_RATE);
                    value += 0.22 * (2.0 * std::f64::consts::PI * hz * t).sin();
                    value += 0.07 * (2.0 * std::f64::consts::PI * hz * 2.0 * t).sin();
                }
                let v = value.clamp(-1.0, 1.0) as f32;
                [v, v]
            })
            .collect()
    }

    /// C major and A minor are the hard pair: the same seven notes.
    #[test]
    fn a_progression_is_read_as_its_key() {
        for (name, expected, roots, minor) in [
            ("C major: C F G C", "8B", [0, 5, 7, 0], false),
            ("A minor: Am Dm E Am", "8A", [9, 2, 4, 9], true),
            ("G major: G C D G", "9B", [7, 0, 2, 7], false),
            ("E minor: Em Am B Em", "9A", [4, 9, 11, 4], true),
        ] {
            let key = measure(&progression(roots, minor)).unwrap();
            assert_eq!(key.camelot, expected, "{name}: {key:?}");
        }
    }

    #[test]
    fn silence_has_no_key() {
        assert_eq!(
            measure(&vec![0.0; SAMPLE_RATE as usize * 30 * CHANNELS]),
            None
        );
    }

    /// It may name a key for noise, but mustn't be sure of it.
    #[test]
    fn noise_is_not_a_confident_key() {
        let mut random = Random::new(5);
        let samples: Vec<f32> = (0..SAMPLE_RATE as usize * 30 * CHANNELS)
            .map(|_| ((random.next() * 2.0 - 1.0) * 0.3) as f32)
            .collect();
        let key = measure(&samples);
        assert!(key.as_ref().is_none_or(|k| k.confidence < 0.5), "{key:?}");
    }
}
