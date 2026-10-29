//! Audio fingerprints, and finding the stretch two of them share.
//!
//! The fingerprint is Haitsma and Kalker's: audio at 8 kHz in frames of 1024 samples, 256
//! apart; each frame's energy in 33 bands spaced evenly in pitch from 300 to 2000 Hz; and a
//! 32-bit hash per frame, one bit per pair of neighbouring bands, set when the difference
//! between them grew since the frame before. The same music gives mostly the same bits even
//! after lossy coding or a slightly different alignment, while unrelated audio differs in about
//! half of them.
//!
//! Two episodes share their intro when, at some offset, their hashes stay close for long
//! enough. Offsets are found by voting: each byte of a hash that equals the same byte of a hash
//! in the other episode votes for the offset between them. In a shared stretch about one byte
//! in ten agrees, against one in 256 by chance, so the true offset stands out. The best few
//! offsets are then walked frame by frame, the bit errors smoothed over half a second.

use std::collections::HashMap;

use crate::sound::fft::Fft;

/// The sample rate audio is fingerprinted at, in Hz: 8 kHz, as the hash expects.
pub const RATE: u32 = 8000;
const FRAME: usize = 1024;
const HOP: usize = 256;
const BANDS: usize = 33;
const LOW_HZ: f64 = 300.0;
const HIGH_HZ: f64 = 2000.0;

/// Frames the bit errors are averaged over (half a second).
const WINDOW: usize = 16;
/// The most average bit errors (of 32) for frames to count as the same audio. Unrelated audio
/// averages 16.
const MAX_ERRORS: f64 = 11.0;
/// Unmatched windows a shared stretch may have in it (half a second).
const GAP: usize = 16;
/// Offsets walked, best voted first, each with its neighbours.
const CANDIDATES: usize = 6;

/// The seconds hash `index` of a fingerprint stands for, from the start of its audio: the
/// middle of its frame.
pub fn seconds(index: usize) -> f64 {
    ((index + 1) * HOP + FRAME / 2) as f64 / f64::from(RATE)
}

/// Hashes covering `seconds` of audio, near enough.
pub fn frames(seconds: f64) -> usize {
    (seconds * f64::from(RATE) / HOP as f64).round() as usize
}

/// The fingerprint of mono audio at [`RATE`].
pub fn fingerprint(samples: &[f32]) -> Vec<u32> {
    if samples.len() < FRAME {
        return Vec::new();
    }
    let fft = Fft::new(FRAME).expect("FRAME is a power of two");
    let window: Vec<f64> = (0..FRAME)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FRAME as f64).cos())
        .collect();
    let bin = |hz: f64| (hz * FRAME as f64 / f64::from(RATE)).round() as usize;
    let edges: Vec<usize> = (0..=BANDS)
        .map(|b| bin(LOW_HZ * (HIGH_HZ / LOW_HZ).powf(b as f64 / BANDS as f64)))
        .collect();
    let (mut real, mut imaginary) = (vec![0.0; FRAME], vec![0.0; FRAME]);
    let mut previous: Option<[f64; BANDS]> = None;
    let mut hashes = Vec::with_capacity(samples.len() / HOP);
    for start in (0..=samples.len() - FRAME).step_by(HOP) {
        for (i, (sample, weight)) in samples[start..start + FRAME]
            .iter()
            .zip(&window)
            .enumerate()
        {
            real[i] = f64::from(*sample) * weight;
            imaginary[i] = 0.0;
        }
        fft.transform(&mut real, &mut imaginary);
        let mut energy = [0.0; BANDS];
        for (band, e) in energy.iter_mut().enumerate() {
            *e = (edges[band]..edges[band + 1].max(edges[band] + 1))
                .map(|k| real[k] * real[k] + imaginary[k] * imaginary[k])
                .sum();
        }
        if let Some(before) = previous {
            let mut hash = 0u32;
            for m in 0..BANDS - 1 {
                let now = energy[m] - energy[m + 1];
                let then = before[m] - before[m + 1];
                if now - then > 0.0 {
                    hash |= 1 << m;
                }
            }
            hashes.push(hash);
        }
        previous = Some(energy);
    }
    hashes
}

/// A stretch two fingerprints share: hash indices `[start, end)` in each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shared {
    /// The stretch in the first fingerprint.
    pub a: (usize, usize),
    /// The stretch in the second fingerprint.
    pub b: (usize, usize),
}

impl Shared {
    /// How many hashes the stretch is long (the same in both).
    pub fn len(&self) -> usize {
        self.a.1 - self.a.0
    }
}

/// The longest stretch `a` and `b` share, if any is at least `min_len` hashes long.
pub fn shared(a: &[u32], b: &[u32], min_len: usize) -> Option<Shared> {
    if a.len() < WINDOW || b.len() < WINDOW {
        return None;
    }
    // Where each byte of each hash is in `b`, keyed by the byte and which of the four it is.
    let mut positions: HashMap<u16, Vec<usize>> = HashMap::new();
    for (j, hash) in b.iter().enumerate() {
        for (n, byte) in hash.to_le_bytes().into_iter().enumerate() {
            positions
                .entry(((n as u16) << 8) | u16::from(byte))
                .or_default()
                .push(j);
        }
    }
    // A byte far commoner than chance (silence, a held tone) says nothing about where.
    let most = b.len() / 256 * 4 + 16;
    // Offset d = i - j, kept as d + b.len() so it's never negative.
    let mut votes = vec![0u32; a.len() + b.len()];
    for (i, hash) in a.iter().enumerate() {
        if *hash == 0 || *hash == u32::MAX {
            continue;
        }
        for (n, byte) in hash.to_le_bytes().into_iter().enumerate() {
            let key = ((n as u16) << 8) | u16::from(byte);
            if let Some(js) = positions.get(&key).filter(|js| js.len() <= most) {
                for j in js {
                    votes[i + b.len() - j] += 1;
                }
            }
        }
    }
    // The best voted offsets, each at least a few frames from the others.
    let mut ranked: Vec<usize> = (0..votes.len()).filter(|&d| votes[d] > 0).collect();
    ranked.sort_by(|x, y| votes[*y].cmp(&votes[*x]).then(x.cmp(y)));
    let mut peaks: Vec<usize> = Vec::new();
    for d in ranked {
        if peaks.len() == CANDIDATES {
            break;
        }
        if peaks.iter().all(|p| p.abs_diff(d) > 2) {
            peaks.push(d);
        }
    }
    let mut tried = Vec::new();
    let mut best: Option<Shared> = None;
    for d in peaks {
        for d in d.saturating_sub(1)..=(d + 1).min(votes.len() - 1) {
            if tried.contains(&d) {
                continue;
            }
            tried.push(d);
            let offset = d as isize - b.len() as isize;
            if let Some(found) = along(a, b, offset)
                && best.is_none_or(|b| found.len() > b.len())
            {
                best = Some(found);
            }
        }
    }
    best.filter(|s| s.len() >= min_len)
}

/// The longest stretch `a` and `b` share with `a[i]` against `b[i - offset]`.
fn along(a: &[u32], b: &[u32], offset: isize) -> Option<Shared> {
    let first = offset.max(0) as usize;
    let last = a.len().min((b.len() as isize + offset).max(0) as usize);
    if last < first + WINDOW {
        return None;
    }
    let mut sums = Vec::with_capacity(last - first + 1);
    sums.push(0u32);
    for (i, hash) in a.iter().enumerate().take(last).skip(first) {
        let j = (i as isize - offset) as usize;
        let errors = (hash ^ b[j]).count_ones();
        sums.push(sums.last().copied().unwrap_or(0) + errors);
    }
    let windows = last - first - WINDOW + 1;
    let close = |k: usize| f64::from(sums[k + WINDOW] - sums[k]) / WINDOW as f64 <= MAX_ERRORS;
    let mut best: Option<(usize, usize)> = None;
    let mut run: Option<(usize, usize)> = None;
    for k in 0..windows {
        if !close(k) {
            continue;
        }
        run = match run {
            Some((start, end)) if k - end <= GAP + 1 => Some((start, k)),
            _ => Some((k, k)),
        };
        let (start, end) = run.expect("both arms above set it");
        if best.is_none_or(|(s, e)| end - start > e - s) {
            best = Some((start, end));
        }
    }
    let (start, end) = best?;
    let (a_start, a_end) = (first + start, first + end + WINDOW);
    let shift = |i: usize| (i as isize - offset) as usize;
    Some(Shared {
        a: (a_start, a_end),
        b: (shift(a_start), shift(a_end)),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny deterministic generator, so tests make the same "music" every time.
    pub(crate) struct Noise(u64);

    impl Noise {
        pub(crate) fn new(seed: u64) -> Self {
            Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
        }

        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// Between 0 and 1.
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }

        /// `seconds` of a tune at [`RATE`]: three random notes at a time with their overtones,
        /// changing every quarter second, over a rustle of its own (a drum kit's, say), so
        /// every band has something in it, as music does.
        pub(crate) fn tune(&mut self, seconds: f64) -> Vec<f32> {
            let total = (seconds * f64::from(RATE)) as usize;
            let note = RATE as usize / 4;
            let mut out = Vec::with_capacity(total);
            let mut pitches = [0.0; 3];
            let mut loudness = 0.0;
            for n in 0..total {
                if n % note == 0 {
                    for p in &mut pitches {
                        *p = 150.0 * 2f64.powf(self.unit() * 3.0);
                    }
                    loudness = 0.05 + 0.15 * self.unit();
                }
                let t = n as f64 / f64::from(RATE);
                let mut v = 0.0;
                for p in pitches {
                    for h in 1..=12 {
                        let f = p * f64::from(h);
                        if f < 3900.0 {
                            v += (2.0 * std::f64::consts::PI * f * t).sin() / f64::from(h);
                        }
                    }
                }
                let rustle = (self.unit() - 0.5) * 2.0 * loudness;
                out.push((v / 12.0 + rustle) as f32);
            }
            out
        }

        /// Quiet hiss added to `samples`, as a second encoding might leave.
        pub(crate) fn hiss(&mut self, samples: &mut [f32], level: f64) {
            for s in samples {
                *s += ((self.unit() - 0.5) * 2.0 * level) as f32;
            }
        }
    }

    /// An "episode": `before` seconds of its own audio, the shared `intro`, then `after`.
    pub(crate) fn episode(seed: u64, before: f64, intro: &[f32], after: f64) -> Vec<f32> {
        let mut noise = Noise::new(seed);
        let mut samples = noise.tune(before);
        samples.extend_from_slice(intro);
        samples.extend(noise.tune(after));
        noise.hiss(&mut samples, 0.02);
        samples
    }

    #[test]
    fn finds_the_stretch_two_episodes_share() {
        let intro = Noise::new(99).tune(40.0);
        // Offsets that aren't whole frames apart, so the frames fall differently on the intro.
        let a = fingerprint(&episode(1, 12.0, &intro, 30.0));
        let b = fingerprint(&episode(2, 47.3, &intro, 20.0));
        let found = shared(&a, &b, frames(15.0)).expect("the intro is found");
        let (a_start, a_end) = (seconds(found.a.0), seconds(found.a.1));
        let (b_start, b_end) = (seconds(found.b.0), seconds(found.b.1));
        assert!((a_start - 12.0).abs() < 0.6, "{a_start}");
        assert!((a_end - 52.0).abs() < 0.6, "{a_end}");
        assert!((b_start - 47.3).abs() < 0.6, "{b_start}");
        assert!((b_end - 87.3).abs() < 0.6, "{b_end}");
    }

    #[test]
    fn unrelated_audio_shares_nothing() {
        let a = fingerprint(&Noise::new(5).tune(90.0));
        let b = fingerprint(&Noise::new(6).tune(90.0));
        assert_eq!(shared(&a, &b, frames(5.0)), None);
        let quiet = vec![0.0f32; 8000 * 60];
        assert_eq!(
            shared(&fingerprint(&quiet), &fingerprint(&quiet), frames(5.0)),
            None,
            "silence isn't an intro"
        );
    }

    #[test]
    fn too_short_a_stretch_is_no_intro() {
        let intro = Noise::new(42).tune(8.0);
        let a = fingerprint(&episode(3, 10.0, &intro, 20.0));
        let b = fingerprint(&episode(4, 20.0, &intro, 10.0));
        assert!(shared(&a, &b, frames(5.0)).is_some());
        assert_eq!(shared(&a, &b, frames(15.0)), None);
    }

    #[test]
    fn seconds_and_frames_agree() {
        assert_eq!(frames(1.0), 31);
        assert!((seconds(frames(60.0)) - 60.1).abs() < 0.1);
    }
}
