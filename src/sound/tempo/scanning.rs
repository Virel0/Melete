//! Building the onset-strength signal from decoded samples: spectral flux over log-spaced bands,
//! rises only, Hann-tapered.

use super::*;

/// One onset value per 10 ms of audio.
pub(super) const TARGET_RATE: f64 = 100.0;
/// How hard quiet bands are pulled up before differencing, so a loud chorus doesn't drown
/// the beat of a quiet verse.
pub(super) const COMPRESSION_GAIN: f64 = 1e6;
/// Below this there's more rumble than rhythm.
pub(super) const LOWEST_BAND_HZ: f64 = 30.0;
/// Above this nothing carries a beat.
pub(super) const HIGHEST_BAND_HZ: f64 = 16_000.0;
/// Bands below this are where a downbeat is found.
pub(super) const LOW_BAND_TOP_HZ: f64 = 250.0;
/// Hops averaged into one timbre frame: structure moves over bars, not milliseconds.
pub(super) const TIMBRE_DECIMATION: usize = 5;
/// How many bands to aim for, before bins force them apart.
pub(super) const TARGET_BANDS: usize = 24;

/// Builds the onset-strength signal from decoded samples: each 23 ms window's spectrum folded
/// into log-spaced bands, each band's log energy compared with the hop before, rises summed.
/// Only rises count (a note ending isn't an onset), and the window is Hann-tapered, without
/// which a held tone's energy wobbles periodically and reads as a tempo.
pub struct TempoScan {
    pub(super) channels: usize,
    pub(super) hop: usize,
    pub(super) size: usize,
    /// Onset values a second actually used.
    pub rate: f64,
    pub(super) fft: Fft,
    pub(super) taper: Vec<f64>,
    pub(super) taper_power: f64,
    pub(super) ring: Vec<f32>,
    pub(super) real: Vec<f64>,
    pub(super) imaginary: Vec<f64>,
    pub(super) edges: Vec<usize>,
    /// How many of the bottom bands are under 250 Hz, where a downbeat's kick is.
    pub(super) low_bands: usize,
    pub(super) previous: Vec<f64>,
    /// Onset strength, one value per hop.
    pub onsets: Vec<f32>,
    /// The same from the low bands alone, for finding downbeats.
    pub low_onsets: Vec<f32>,
    /// Where the first onset value sits, in frames; None before there is one.
    pub(super) first_onset_frame: Option<f64>,
    /// The log energy of every band, one frame per [`TIMBRE_DECIMATION`] hops: what a
    /// passage is made of, which tells one section from another.
    pub timbre: Vec<Vec<f32>>,
    pub(super) timbre_sum: Vec<f64>,
    pub(super) timbre_count: usize,
    pub(super) sample_rate: u32,
    pub(super) have_previous: bool,
    pub(super) write_at: usize,
    pub(super) since_hop: usize,
    pub(super) seen: u64,
}

impl TempoScan {
    /// A scan for interleaved samples of `channels` channels at `sample_rate` Hz.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        let hop = (round(f64::from(sample_rate) / TARGET_RATE) as usize).max(1);
        // At least twice the hop, so windows overlap, and a power of two: 1024 at any
        // ordinary sample rate.
        let mut size = 1024;
        while size < hop * 2 {
            size <<= 1;
        }
        let taper: Vec<f64> = (0..size)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (size - 1) as f64).cos())
            })
            .collect();
        let edges = bands(sample_rate, size);
        // A downbeat is a kick, and a kick is far more reliably the loudest thing under
        // 250 Hz than the loudest thing overall.
        let mut low_bands = 0;
        while low_bands + 2 < edges.len()
            && edges[low_bands + 1] as f64 * f64::from(sample_rate) / size as f64 <= LOW_BAND_TOP_HZ
        {
            low_bands += 1;
        }
        let low_bands = low_bands.max(1);
        Self {
            channels,
            hop,
            size,
            rate: f64::from(sample_rate) / hop as f64,
            fft: Fft::new(size)
                .expect("size starts at 1024 and only doubles, so it is a power of two"),
            taper_power: taper.iter().map(|t| t * t).sum(),
            taper,
            ring: vec![0.0; size],
            real: vec![0.0; size],
            imaginary: vec![0.0; size],
            previous: vec![0.0; edges.len() - 1],
            timbre_sum: vec![0.0; edges.len() - 1],
            edges,
            low_bands,
            low_onsets: Vec::new(),
            first_onset_frame: None,
            timbre: Vec::new(),
            timbre_count: 0,
            sample_rate,
            onsets: Vec::new(),
            have_previous: false,
            write_at: 0,
            since_hop: 0,
            seen: 0,
        }
    }

    /// Timbre frames a second.
    pub fn timbre_rate(&self) -> f64 {
        self.rate / TIMBRE_DECIMATION as f64
    }

    /// Where the first onset value sits in the track, in milliseconds.
    pub fn first_onset_ms(&self) -> f64 {
        self.first_onset_frame
            .map_or(0.0, |frame| frame * 1000.0 / f64::from(self.sample_rate))
    }

    /// The estimate, once the whole file has been fed.
    pub fn result(&self) -> Option<Tempo> {
        estimate(&self.onsets, self.rate)
    }

    /// Adds up how much each band rose since the previous hop.
    pub(super) fn flux(&mut self) {
        let mut read = self.write_at;
        for i in 0..self.size {
            self.real[i] = f64::from(self.ring[read]) * self.taper[i];
            self.imaginary[i] = 0.0;
            read = if read + 1 == self.size { 0 } else { read + 1 };
        }
        self.fft.transform(&mut self.real, &mut self.imaginary);
        let (mut risen, mut low) = (0.0, 0.0);
        for band in 0..self.previous.len() {
            let (from, to) = (self.edges[band], self.edges[band + 1]);
            let power: f64 = (from..to)
                .map(|bin| {
                    self.real[bin] * self.real[bin] + self.imaginary[bin] * self.imaginary[bin]
                })
                .sum();
            // Mean density, so a wide band isn't louder for being wide, then compressed so a
            // quiet band still gets a vote.
            let density = power / (self.size as f64 * self.taper_power * (to - from) as f64);
            let energy = (1.0 + COMPRESSION_GAIN * density).ln();
            let rise = (energy - self.previous[band]).max(0.0);
            risen += rise;
            if band < self.low_bands {
                low += rise;
            }
            self.previous[band] = energy;
            self.timbre_sum[band] += energy;
        }
        self.timbre_count += 1;
        if self.timbre_count == TIMBRE_DECIMATION {
            self.timbre.push(
                self.timbre_sum
                    .iter_mut()
                    .map(|sum| (std::mem::take(sum) / TIMBRE_DECIMATION as f64) as f32)
                    .collect(),
            );
            self.timbre_count = 0;
        }
        if self.have_previous {
            // At the midpoint of the two windows whose difference made it: a fixed offset of a
            // few milliseconds for every track, which moves an anchor and never a residual.
            self.first_onset_frame
                .get_or_insert(self.seen as f64 - self.size as f64 / 2.0 - self.hop as f64 / 2.0);
            self.onsets
                .push((risen / self.previous.len() as f64) as f32);
            self.low_onsets.push((low / self.low_bands as f64) as f32);
        }
        self.have_previous = true;
    }
}

impl Consumer for TempoScan {
    fn feed(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            let mono: f64 = frame.iter().map(|&v| f64::from(v)).sum();
            self.ring[self.write_at] = (mono / self.channels as f64) as f32;
            self.write_at = if self.write_at + 1 == self.size {
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
            if self.seen >= self.size as u64 {
                self.flux();
            }
        }
    }
}

/// Log-spaced band edges as bin indices, at least one bin apart. Low frequencies run out of
/// bins first, so there are usually a few under the target.
pub(super) fn bands(sample_rate: u32, size: usize) -> Vec<usize> {
    let top = HIGHEST_BAND_HZ.min(f64::from(sample_rate) * 0.45);
    let bins = size / 2;
    let mut edges: Vec<usize> = Vec::new();
    for i in 0..=TARGET_BANDS {
        let hz = LOWEST_BAND_HZ * (top / LOWEST_BAND_HZ).powf(i as f64 / TARGET_BANDS as f64);
        let bin = round(hz * size as f64 / f64::from(sample_rate)).clamp(1, bins as i64) as usize;
        if edges.last().is_none_or(|&last| bin > last) {
            edges.push(bin);
        }
    }
    if edges.len() >= 2 {
        edges
    } else {
        vec![1, bins]
    }
}
