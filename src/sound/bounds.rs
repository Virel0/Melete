//! Where the sound in a file starts and stops: the phone's silence measurement
//! (the phone app's `SilenceBounds`), ported from the sidecar line for line so a track trims
//! the same wherever it plays. A frame is sound when any channel's absolute value exceeds a
//! fixed threshold; the first and last such frames become milliseconds by schoolbook rounding,
//! margins are added, and a trim too small to be worth a seek is no trim at all.

use super::Consumer;

/// Quieter than this is silence: −55 dBFS, under a fade's tail but above the noise floor of a
/// vinyl rip.
pub const THRESHOLD: f32 = 0.00178;
/// Kept before the first sound, so a note's attack is never clipped.
pub const LEAD_MARGIN_MS: i64 = 60;
/// Kept after the last sound, so a reverb tail isn't cut mid-decay.
pub const TAIL_MARGIN_MS: i64 = 200;
/// Trimming less than this at both ends isn't worth the seek.
pub const MINIMUM_TRIM_MS: i64 = 300;

/// Where the sound in a file starts and stops, margins included, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoundBounds {
    /// Where the sound starts, less the lead margin; never below 0.
    pub start_ms: i64,
    /// Where the sound stops, plus the tail margin; never past the file's end.
    pub end_ms: i64,
}

/// Swift's `(frames / rate * 1000).rounded()`: halves away from zero, as Rust's `round` does.
fn to_ms(frames: i64, sample_rate: u32) -> i64 {
    (frames as f64 / f64::from(sample_rate) * 1000.0).round() as i64
}

/// Finds the first and last frames above [`THRESHOLD`], fed the decoded samples.
pub struct BoundsScan {
    channels: usize,
    sample_rate: u32,
    total: i64,
    first_loud: Option<i64>,
    last_loud: Option<i64>,
}

impl BoundsScan {
    /// A scan for interleaved samples of `channels` channels at `sample_rate` Hz.
    pub fn new(channels: usize, sample_rate: u32) -> Self {
        Self {
            channels,
            sample_rate,
            total: 0,
            first_loud: None,
            last_loud: None,
        }
    }

    /// The sounding span, or None when there's nothing worth trimming: a file that starts and
    /// ends with sound, or one with no sound at all.
    pub fn result(&self) -> Option<SoundBounds> {
        let (Some(first), Some(last)) = (self.first_loud, self.last_loud) else {
            return None;
        };
        let length = to_ms(self.total, self.sample_rate);
        let start = (to_ms(first, self.sample_rate) - LEAD_MARGIN_MS).max(0);
        let end = (to_ms(last, self.sample_rate) + TAIL_MARGIN_MS).min(length);
        if start < MINIMUM_TRIM_MS && length - end < MINIMUM_TRIM_MS {
            return None;
        }
        Some(SoundBounds {
            start_ms: start,
            end_ms: end,
        })
    }
}

impl Consumer for BoundsScan {
    fn feed(&mut self, samples: &[f32]) {
        for (frame, values) in samples.chunks_exact(self.channels).enumerate() {
            if values.iter().any(|v| v.abs() > THRESHOLD) {
                let index = self.total + frame as i64;
                self.first_loud.get_or_insert(index);
                self.last_loud = Some(index);
            }
        }
        self.total += (samples.len() / self.channels) as i64;
    }
}

#[cfg(test)]
mod tests {
    //! The sidecar's `SoundBoundsAnalyzerTests`, case for case.

    use super::super::pump;
    use super::*;

    const RATE: u32 = 44_100;

    fn analyze(samples: &[f32], channels: usize, rate: u32) -> Option<SoundBounds> {
        let mut scan = BoundsScan::new(channels, rate);
        scan.feed(samples);
        scan.result()
    }

    /// Stereo: a 440 Hz tone between silences, in seconds.
    fn file(leading: f64, sound: f64, trailing: f64, amplitude: f32) -> Vec<f32> {
        let rate = f64::from(RATE);
        let total = ((leading + sound + trailing) * rate) as usize;
        let (from, to) = (
            (leading * rate) as usize,
            ((leading + sound) * rate) as usize,
        );
        (0..total)
            .flat_map(|i| {
                let v = if (from..to).contains(&i) {
                    amplitude * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin() as f32
                } else {
                    0.0
                };
                [v, v]
            })
            .collect()
    }

    fn close(a: i64, b: i64) -> bool {
        (a - b).abs() <= 60
    }

    #[test]
    fn finds_the_sound_between_leading_and_trailing_silence_with_margins() {
        let found = analyze(&file(2.0, 3.0, 2.5, 0.5), 2, RATE).unwrap();
        assert!(close(found.start_ms, 2000 - LEAD_MARGIN_MS));
        assert!(close(found.end_ms, 5000 + TAIL_MARGIN_MS));
        // Deterministic over the same samples, so the spec's "≈ 1.94 s and ≈ 5.2 s" is exact.
        assert_eq!((found.start_ms, found.end_ms), (1940, 5200));
    }

    #[test]
    fn a_file_that_starts_and_ends_with_sound_is_left_alone() {
        assert_eq!(analyze(&file(0.0, 3.0, 0.0, 0.5), 2, RATE), None);
    }

    #[test]
    fn a_trim_too_small_to_matter_is_left_alone() {
        assert_eq!(analyze(&file(0.1, 3.0, 0.1, 0.5), 2, RATE), None);
    }

    #[test]
    fn silence_at_one_end_only_is_trimmed_at_that_end() {
        let found = analyze(&file(0.0, 2.0, 3.0, 0.5), 2, RATE).unwrap();
        assert_eq!(found.start_ms, 0);
        assert!(close(found.end_ms, 2000 + TAIL_MARGIN_MS));
    }

    #[test]
    fn a_file_with_no_sound_at_all_is_nothing_to_trim() {
        assert_eq!(analyze(&file(2.0, 0.0, 0.0, 0.5), 2, RATE), None);
    }

    #[test]
    fn a_quiet_ending_above_the_threshold_is_kept() {
        // −40 dBFS: quiet, but music.
        assert_eq!(analyze(&file(0.0, 3.0, 0.0, 0.01), 2, RATE), None);
    }

    #[test]
    fn a_sample_exactly_at_the_threshold_is_silence() {
        let samples: Vec<f32> = (0..4 * RATE)
            .flat_map(|_| [THRESHOLD, -THRESHOLD])
            .collect();
        assert_eq!(analyze(&samples, 2, RATE), None);
    }

    #[test]
    fn one_loud_sample_bounds_a_span_around_itself() {
        let samples: Vec<f32> = (0..4 * RATE)
            .flat_map(|i| [if i == RATE { 1.0 } else { 0.0 }, 0.0])
            .collect();
        assert_eq!(
            analyze(&samples, 2, RATE),
            Some(SoundBounds {
                start_ms: 1000 - 60,
                end_ms: 1000 + 200
            })
        );
    }

    #[test]
    fn any_channel_counts() {
        let samples: Vec<f32> = (0..4 * RATE)
            .flat_map(|i| [0.0, if i == RATE { 0.5 } else { 0.0 }])
            .collect();
        assert!(analyze(&samples, 2, RATE).is_some());
    }

    #[test]
    fn half_milliseconds_round_away_from_zero_as_on_the_phone() {
        // At 8 kHz, frame 4 is 0.5 ms: 1, where banker's rounding would make it 0.
        let samples: Vec<f32> = (0..5_000).map(|i| if i == 4 { 0.5 } else { 0.0 }).collect();
        let found = analyze(&samples, 1, 8_000).unwrap();
        assert_eq!((found.start_ms, found.end_ms), (0, 1 + TAIL_MARGIN_MS));
    }

    #[test]
    fn an_empty_stream_is_nothing() {
        assert_eq!(analyze(&[], 2, RATE), None);
    }

    #[tokio::test]
    async fn frames_split_across_reads_are_not_torn() {
        // A stream that hands back awkward byte counts must still see every sample whole.
        let bytes: Vec<u8> = file(2.0, 3.0, 2.5, 0.5)
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let dribble = tokio_test_reader(bytes, 7);
        let (frames, scan) = pump(dribble, 2, BoundsScan::new(2, RATE)).await.unwrap();
        assert_eq!(frames, (7.5 * f64::from(RATE)) as i64);
        assert_eq!(
            scan.result(),
            Some(SoundBounds {
                start_ms: 1940,
                end_ms: 5200
            })
        );
    }

    /// A reader that returns at most `max` bytes per read.
    fn tokio_test_reader(bytes: Vec<u8>, max: usize) -> impl tokio::io::AsyncRead + Unpin {
        struct Dribble(std::io::Cursor<Vec<u8>>, usize);
        impl tokio::io::AsyncRead for Dribble {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                let max = self.1.min(buf.remaining());
                let mut chunk = vec![0; max];
                let n = std::io::Read::read(&mut self.0, &mut chunk)?;
                buf.put_slice(&chunk[..n]);
                std::task::Poll::Ready(Ok(()))
            }
        }
        Dribble(std::io::Cursor::new(bytes), max)
    }
}
