//! Where someone is probably singing: the sidecar's `VocalScan`, ported line for line. With no
//! way to isolate a voice, it leans on two things true of most produced records: a lead vocal
//! is mixed to the centre (it survives in the mid channel and cancels in the side), and a
//! voice doesn't hold a pitch (the vocal band's spectrum keeps moving where a pad's doesn't).
//!
//! It's biased towards yes: a client uses it to refuse an overlap of two vocals, so a false
//! positive costs a mix that would have been fine and a false negative is the mistake it
//! exists to prevent. Where the file has no usable stereo image the answer is None, *not
//! known*, which must never be read as "no vocals".

use super::Consumer;
use super::fft::Fft;

const SIZE: usize = 2048;
/// The band a lead vocal lives in.
const LOW_HZ: f64 = 200.0;
const HIGH_HZ: f64 = 4000.0;
/// Under this share of the mid channel, the file has no stereo image to read.
const MONO_THRESHOLD: f64 = 0.005;
/// Over how long the frame-by-frame guess is averaged before it's believed.
const SMOOTHING_SECONDS: f64 = 2.0;
/// How far the sung passages must stand above the rest before any claim is made.
const MINIMUM_CONTRAST: f64 = 0.05;
/// Nothing shorter than this is a vocal passage.
const MINIMUM_SPAN_MS: f64 = 2000.0;
/// A break shorter than this is a breath, not the end of a verse.
const BRIDGE_MS: f64 = 1500.0;

/// A stretch where someone is probably singing, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VocalSpan {
    /// Where the singing starts.
    pub start_ms: f64,
    /// Where it ends.
    pub end_ms: f64,
}

/// Finds where someone sings from how centred and how changing the vocal band is, fed the decoded
/// samples.
pub struct VocalScan {
    channels: usize,
    hop: usize,
    fft: Fft,
    taper: Vec<f64>,
    left: Vec<f32>,
    right: Vec<f32>,
    real: Vec<f64>,
    imaginary: Vec<f64>,
    from: usize,
    to: usize,
    rate: f64,
    /// How much of the vocal band is centre rather than spread, per frame.
    centred: Vec<f32>,
    /// How much the vocal band's spectrum moved since the frame before.
    moving: Vec<f32>,
    previous: Vec<f64>,
    write_at: usize,
    since_hop: usize,
    seen: u64,
    mid_total: f64,
    side_total: f64,
}

/// C#'s `Math.Round(x, 1)`: halves to even.
fn round_tenth(x: f64) -> f64 {
    (x * 10.0).round_ties_even() / 10.0
}

impl VocalScan {
    /// A scan for interleaved samples of `channels` channels at `sample_rate` Hz.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let from = ((LOW_HZ * SIZE as f64 / rate) as usize).max(1);
        let to = (SIZE / 2 - 1).min((HIGH_HZ * SIZE as f64 / rate) as usize);
        Self {
            channels,
            hop: SIZE / 2,
            fft: Fft::new(SIZE).expect("SIZE, 2048, is a power of two"),
            taper: (0..SIZE)
                .map(|i| {
                    0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (SIZE - 1) as f64).cos())
                })
                .collect(),
            left: vec![0.0; SIZE],
            right: vec![0.0; SIZE],
            real: vec![0.0; SIZE],
            imaginary: vec![0.0; SIZE],
            from,
            to,
            rate: rate / (SIZE / 2) as f64,
            centred: Vec::new(),
            moving: Vec::new(),
            previous: vec![0.0; (to + 1).saturating_sub(from).max(1)],
            write_at: 0,
            since_hop: 0,
            seen: 0,
            mid_total: 0.0,
            side_total: 0.0,
        }
    }

    fn stereo(&self) -> bool {
        self.channels > 1
            && self.mid_total > 0.0
            && self.side_total / self.mid_total > MONO_THRESHOLD
    }

    /// The stretches where someone is probably singing; empty where none were found; None
    /// where it couldn't be told.
    pub fn spans(&self) -> Option<Vec<VocalSpan>> {
        if !self.stereo() || self.centred.len() < 40 {
            // Mono, or stereo mixed so narrowly it may as well be.
            return None;
        }
        let reference = median(&self.moving);
        let likelihood: Vec<f64> = self
            .centred
            .iter()
            .zip(&self.moving)
            .map(|(&centred, &moving)| {
                let restless = if reference > 1e-9 {
                    (f64::from(moving) / reference).clamp(0.0, 2.0) / 2.0
                } else {
                    0.5
                };
                f64::from(centred).max(0.0) * (0.5 + 0.5 * restless)
            })
            .collect();
        let smoothed = smooth(
            &likelihood,
            (SMOOTHING_SECONDS * self.rate).round_ties_even() as usize,
        );
        let mut sorted = smoothed.clone();
        sorted.sort_by(f64::total_cmp);
        let middle = sorted[sorted.len() / 2];
        let high = sorted[(sorted.len() as f64 * 0.9) as usize];
        if high - middle < MINIMUM_CONTRAST {
            // The whole track reads the same: sung throughout or not at all, and this can't
            // tell which.
            return None;
        }
        let threshold = middle + (high - middle) * 0.4;
        let mut spans = Vec::new();
        let mut start = None;
        for (i, &value) in smoothed.iter().enumerate() {
            if value >= threshold {
                start.get_or_insert(i);
            } else if let Some(at) = start.take() {
                self.close(&mut spans, at, i);
            }
        }
        if let Some(at) = start {
            self.close(&mut spans, at, smoothed.len());
        }
        Some(spans)
    }

    /// Adds a span, joined to the one before across a gap too short to be a real break.
    fn close(&self, spans: &mut Vec<VocalSpan>, from: usize, to: usize) {
        let start_ms = from as f64 * 1000.0 / self.rate;
        let end_ms = to as f64 * 1000.0 / self.rate;
        if end_ms - start_ms < MINIMUM_SPAN_MS {
            return;
        }
        if let Some(previous) = spans.last_mut()
            && start_ms - previous.end_ms <= BRIDGE_MS
        {
            previous.end_ms = round_tenth(end_ms);
            return;
        }
        spans.push(VocalSpan {
            start_ms: round_tenth(start_ms),
            end_ms: round_tenth(end_ms),
        });
    }

    fn measure(&mut self) {
        let mid = self.spectrum(true);
        let side = self.spectrum(false);
        let (mut mid_energy, mut side_energy, mut moved) = (0.0, 0.0, 0.0);
        for i in 0..mid.len() {
            mid_energy += mid[i];
            side_energy += side[i];
            moved += (mid[i] - self.previous[i]).abs();
        }
        // One for a voice sitting in the middle, towards zero for a wide pad.
        let total = mid_energy + side_energy;
        self.centred.push(if total <= 1e-12 {
            0.0
        } else {
            ((mid_energy - side_energy) / total) as f32
        });
        // Scaled by its own level, so a loud passage isn't automatically a moving one.
        self.moving.push(if mid_energy <= 1e-12 {
            0.0
        } else {
            (moved / mid_energy) as f32
        });
        self.previous = mid;
    }

    /// Magnitudes across the vocal band of the mid or the side channel.
    fn spectrum(&mut self, centre: bool) -> Vec<f64> {
        let mut read = self.write_at;
        for i in 0..SIZE {
            let (left, right) = (self.left[read], self.right[read]);
            let value = if centre {
                (left + right) / 2.0
            } else {
                (left - right) / 2.0
            };
            self.real[i] = f64::from(value) * self.taper[i];
            self.imaginary[i] = 0.0;
            read = if read + 1 == SIZE { 0 } else { read + 1 };
        }
        self.fft.transform(&mut self.real, &mut self.imaginary);
        (self.from..=self.to)
            .map(|bin| self.real[bin].hypot(self.imaginary[bin]))
            .collect()
    }
}

impl Consumer for VocalScan {
    fn feed(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            let left = frame[0];
            let right = if self.channels > 1 { frame[1] } else { left };
            self.left[self.write_at] = left;
            self.right[self.write_at] = right;
            self.write_at = if self.write_at + 1 == SIZE {
                0
            } else {
                self.write_at + 1
            };
            self.seen += 1;
            let (mid, side) = ((left + right) / 2.0, (left - right) / 2.0);
            self.mid_total += f64::from(mid * mid);
            self.side_total += f64::from(side * side);
            self.since_hop += 1;
            if self.since_hop < self.hop {
                continue;
            }
            self.since_hop = 0;
            if self.seen >= SIZE as u64 {
                self.measure();
            }
        }
    }
}

fn smooth(values: &[f64], window: usize) -> Vec<f64> {
    if window < 2 {
        return values.to_vec();
    }
    let mut prefix = vec![0.0; values.len() + 1];
    for (i, v) in values.iter().enumerate() {
        prefix[i + 1] = prefix[i] + v;
    }
    let half = window / 2;
    (0..values.len())
        .map(|i| {
            let low = i.saturating_sub(half);
            let high = values.len().min(i + half + 1);
            (prefix[high] - prefix[low]) / (high - low) as f64
        })
        .collect()
}

fn median(values: &[f32]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    f64::from(sorted[sorted.len() / 2])
}
