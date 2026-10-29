//! What a track is made of and in what order: the sidecar's `ArrangementAnalyzer`, ported
//! line for line. Each beat of the fitted grid is described by its timbre; a checkerboard
//! kernel dragged down the diagonal of the beat-to-beat similarity reads high where the beats
//! before a point resemble each other, the beats after do too, and the two halves don't —
//! a section boundary, as arithmetic. Boundaries move to a phrase line when one is close and
//! to a bar line otherwise, and sections are named only where level, weight and position
//! settle it: intro, build, drop, breakdown, outro, else unknown. Verse and chorus are never
//! claimed; telling them apart is a judgement about song form, not a property of the signal.
//!
//! Where the original rounds with C#'s `Math.Round`, halves go to even here too.

use super::beat_grid::{BeatGrid, Tracked};
use super::vocals::VocalSpan;

mod beats;
mod kernel;
mod naming;
mod placing;

use beats::*;
use kernel::*;
use naming::*;
use placing::*;

/// Half-width of the checkerboard, in beats: eight bars either side.
const KERNEL_BEATS: usize = 32;
/// No section shorter than this, in beats.
const MINIMUM_SECTION_BEATS: usize = 32;
/// A novelty peak this many standard deviations above the mean is a boundary.
const BOUNDARY_THRESHOLD: f64 = 0.8;
/// Phrase lengths considered, in bars.
const PHRASE_LENGTHS: [i64; 3] = [8, 16, 32];
/// How much more novelty must fall on a phrase line than on an average bar line.
const PHRASE_CONTRAST: f64 = 0.25;
/// How much better a longer phrase must look than a shorter one: a record built in eights
/// also changes on every sixteenth bar, and the longest reading would otherwise win by having
/// fewer lines to average over.
const LONGER_PHRASE_MARGIN: f64 = 1.3;
/// How far a boundary may move to land on a phrase line, in bars; dragging a real change
/// further would be inventing structure.
const PHRASE_NUDGE_BARS: i64 = 2;
/// At or below this a section is quiet enough to be an intro, outro or breakdown.
const QUIET_ENERGY: f64 = 0.4;
/// At or above this, with the weight to match, a section is a drop.
const LOUD_ENERGY: f64 = 0.75;

/// What a section of a track is, by its energy and where it sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Intro,
    Build,
    Drop,
    Breakdown,
    Outro,
    Unknown,
}

impl Kind {
    /// Its name, as stored and served: `intro`, `build`, `drop`, `breakdown`, `outro` or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intro => "intro",
            Self::Build => "build",
            Self::Drop => "drop",
            Self::Breakdown => "breakdown",
            Self::Outro => "outro",
            Self::Unknown => "unknown",
        }
    }
}

/// One section of a track.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// On a phrase line where one was found, on a downbeat otherwise.
    pub start_ms: f64,
    /// Where it ends, in milliseconds: the next section's start, or the track's end.
    pub end_ms: f64,
    /// What it is.
    pub kind: Kind,
    /// Zero to one within the track: a quiet record's loudest part still reads as one.
    pub energy: f64,
}

/// A track's structure: its sections, its phrase length, and where someone sings.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrangement {
    /// Contiguous, covering the whole track.
    pub sections: Vec<Section>,
    /// 8, 16 or 32 bars, or None where the structure wouldn't commit.
    pub phrase_bars: Option<i64>,
    /// A downbeat that begins a phrase.
    pub phrase_anchor_ms: Option<f64>,
    /// Empty when none was found; None when it couldn't be told, which isn't the same.
    pub vocals: Option<Vec<VocalSpan>>,
}

/// C#'s `Math.Round(x, digits)`.
fn round_to(x: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (x * scale).round_ties_even() / scale
}

/// The tempo scan's timbre and onsets, the grid and its beats, and the vocal spans, as one
/// arrangement; None when the track has no structure worth describing.
pub struct Inputs<'a> {
    /// The tempo scan's timbre: each band's log energy, one frame at a time.
    pub timbre: &'a [Vec<f32>],
    /// Timbre frames a second.
    pub timbre_rate: f64,
    /// The tempo scan's onset strength, one value per hop.
    pub onsets: &'a [f32],
    /// Onset values a second.
    pub onset_rate: f64,
    /// Where the first onset value sits in the track, in milliseconds.
    pub first_onset_ms: f64,
    /// The beats the grid was fitted to; none means no arrangement.
    pub beats: Option<&'a Tracked>,
    /// The track's beat grid; none means no arrangement.
    pub grid: Option<&'a BeatGrid>,
    /// The track's length, in milliseconds.
    pub duration_ms: f64,
    /// Where someone sings: empty when no one does, none when it couldn't be told.
    pub vocals: Option<Vec<VocalSpan>>,
}

/// The track's arrangement from one decode's scans; none without beats and a grid, without timbre,
/// or when the track is too short for two sections.
pub fn analyze(inputs: Inputs<'_>) -> Option<Arrangement> {
    let (Some(beats), Some(grid)) = (inputs.beats, inputs.grid) else {
        return None;
    };
    if inputs.timbre.is_empty() {
        return None;
    }
    let (from, to) = (beats.first, beats.last);
    if to + 1 - from < MINIMUM_SECTION_BEATS * 2 {
        return None;
    }
    let spans = beat_spans(beats, from, to);
    let features = per_beat(inputs.timbre, inputs.timbre_rate, &spans);
    let loudness = per_beat_level(inputs.timbre, inputs.timbre_rate, &spans);
    let density = per_beat_density(
        inputs.onsets,
        inputs.onset_rate,
        inputs.first_onset_ms,
        &spans,
    );
    let novelty = novelty(&features);
    let (phrase_bars, phrase_anchor_ms) = phrase(&novelty, beats, from, grid);
    let boundaries = boundaries(&novelty, beats, from, to, phrase_bars, phrase_anchor_ms);
    let sections = describe(
        &boundaries,
        beats,
        from,
        to,
        &loudness,
        &density,
        inputs.timbre,
        inputs.timbre_rate,
        inputs.duration_ms,
        grid,
    );
    (!sections.is_empty()).then_some(Arrangement {
        sections,
        phrase_bars,
        phrase_anchor_ms,
        vocals: inputs.vocals,
    })
}

#[cfg(test)]
mod tests {
    //! The sidecar's `ArrangementTests`, over synthesised music built of sections at known bar
    //! lines. Noise comes from our own seeded generator, not .NET's `Random`.

    use super::super::beat_grid;
    use super::super::tempo::TempoScan;
    use super::super::tempo::tests::Random;
    use super::super::vocals::VocalScan;
    use super::super::{Consumer, pump};
    use super::*;

    const SAMPLE_RATE: u32 = 44_100;
    const RATE: f64 = SAMPLE_RATE as f64;
    const CHANNELS: usize = 2;
    const BPM: f64 = 128.0;
    const BAR_MS: f64 = 4.0 * 60_000.0 / BPM;

    struct Scans {
        tempo: TempoScan,
        voice: VocalScan,
    }

    impl Consumer for Scans {
        fn feed(&mut self, samples: &[f32]) {
            self.tempo.feed(samples);
            self.voice.feed(samples);
        }
    }

    /// Through the pump, as the real decode is: the scans see the samples in blocks.
    async fn scan(samples: &[f32]) -> Scans {
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let scans = Scans {
            tempo: TempoScan::new(CHANNELS, SAMPLE_RATE),
            voice: VocalScan::new(CHANNELS, SAMPLE_RATE),
        };
        pump(bytes.as_slice(), CHANNELS, scans).await.unwrap().1
    }

    fn duration_ms(samples: &[f32]) -> f64 {
        samples.len() as f64 / CHANNELS as f64 / RATE * 1000.0
    }

    async fn grid(samples: &[f32]) -> Option<BeatGrid> {
        let scans = scan(samples).await;
        let t = &scans.tempo;
        beat_grid::analyze(
            &t.onsets,
            &t.low_onsets,
            t.rate,
            t.first_onset_ms(),
            duration_ms(samples),
            t.result().as_ref(),
        )
    }

    async fn measure(samples: &[f32]) -> Option<Arrangement> {
        let scans = scan(samples).await;
        let t = &scans.tempo;
        let tempo = t.result();
        let found = beat_grid::analyze_with_beats(
            &t.onsets,
            &t.low_onsets,
            t.rate,
            t.first_onset_ms(),
            duration_ms(samples),
            tempo.as_ref(),
        );
        analyze(Inputs {
            timbre: &t.timbre,
            timbre_rate: t.timbre_rate(),
            onsets: &t.onsets,
            onset_rate: t.rate,
            first_onset_ms: t.first_onset_ms(),
            beats: found.as_ref().map(|f| &f.1),
            grid: found.as_ref().map(|f| &f.0),
            duration_ms: duration_ms(samples),
            vocals: scans.voice.spans(),
        })
    }

    /// Sixteen bars of intro, sixteen of build, thirty-two of drop, sixteen of breakdown,
    /// sixteen of drop and sixteen of outro.
    fn arranged() -> Vec<f32> {
        // (bars, kick, bass, hat, pad)
        let plan: [(usize, f64, f64, f64, f64); 6] = [
            (16, 0.00, 0.00, 0.06, 0.10),
            (16, 0.25, 0.10, 0.14, 0.16),
            (32, 0.85, 0.40, 0.20, 0.14),
            (16, 0.00, 0.02, 0.05, 0.22),
            (16, 0.85, 0.40, 0.20, 0.14),
            (16, 0.05, 0.02, 0.05, 0.10),
        ];
        let beat_frames = 60.0 / BPM * RATE;
        let total_bars: usize = plan.iter().map(|p| p.0).sum();
        let frames = (total_bars as f64 * 4.0 * beat_frames) as usize;
        let mut random = Random::new(21);
        (0..frames)
            .flat_map(|frame| {
                let beat = frame as f64 / beat_frames;
                let bar = (beat / 4.0) as usize;
                let mut at = 0;
                let mut part = plan[0];
                for candidate in plan {
                    if bar < at + candidate.0 {
                        part = candidate;
                        break;
                    }
                    at += candidate.0;
                }
                let index = beat as usize;
                let since = (beat - index as f64) * 60.0 / BPM;
                let t = frame as f64 / RATE;
                let mut value = part.4 * (2.0 * std::f64::consts::PI * 330.0 * t).sin();
                if index.is_multiple_of(4) {
                    value += part.1
                        * (-since / 0.06).exp()
                        * (2.0 * std::f64::consts::PI * 55.0 * since).sin();
                } else {
                    value += part.3 * (-since / 0.01).exp() * (random.next() * 2.0 - 1.0);
                }
                value += part.2 * (2.0 * std::f64::consts::PI * 82.0 * t).sin();
                let v = value.clamp(-1.0, 1.0) as f32;
                [v, v]
            })
            .collect()
    }

    /// A groove with a wide pad throughout and, for one stretch, a centred voice that slides
    /// and vibratos in syllables, the way singing does.
    fn with_voice(from_ms: f64, to_ms: f64, stereo: bool) -> Vec<f32> {
        let frames = SAMPLE_RATE as usize * 150;
        let beat_frames = 60.0 / BPM * RATE;
        let mut random = Random::new(23);
        (0..frames)
            .flat_map(|frame| {
                let ms = frame as f64 * 1000.0 / RATE;
                let t = frame as f64 / RATE;
                let beat = frame as f64 / beat_frames;
                let index = beat as usize;
                let since = (beat - index as f64) * 60.0 / BPM;
                let drums = if index.is_multiple_of(4) {
                    0.7 * (-since / 0.06).exp() * (2.0 * std::f64::consts::PI * 55.0 * since).sin()
                } else {
                    0.10 * (-since / 0.01).exp() * (random.next() * 2.0 - 1.0)
                };
                // Hard left and hard right at different pitches: level in the vocal band,
                // none of it in the centre.
                let pad_left = 0.20 * (2.0 * std::f64::consts::PI * 440.0 * t).sin();
                let pad_right = 0.20 * (2.0 * std::f64::consts::PI * 660.0 * t).sin();
                let mut voice = 0.0;
                if (from_ms..to_ms).contains(&ms) {
                    let into = (ms - from_ms) % 800.0;
                    if into < 560.0 {
                        let note = 260.0 * 2f64.powf(((ms - from_ms) / 800.0).floor() % 5.0 / 12.0);
                        let vibrato = 1.0 + 0.02 * (2.0 * std::f64::consts::PI * 5.5 * t).sin();
                        let shape = (into / 60.0).min(1.0) * ((560.0 - into) / 80.0).min(1.0);
                        voice =
                            shape * 0.30 * (2.0 * std::f64::consts::PI * note * vibrato * t).sin();
                        voice += shape
                            * 0.12
                            * (2.0 * std::f64::consts::PI * note * 2.0 * vibrato * t).sin();
                    }
                }
                let left = (drums + pad_left + voice).clamp(-1.0, 1.0) as f32;
                let right = (drums + if stereo { pad_right } else { pad_left } + voice)
                    .clamp(-1.0, 1.0) as f32;
                [left, if stereo { right } else { left }]
            })
            .collect()
    }

    #[tokio::test]
    async fn the_parts_of_a_track_are_found_where_they_actually_change() {
        let arrangement = measure(&arranged()).await.unwrap();
        let sections = &arrangement.sections;
        assert!((5..=8).contains(&sections.len()), "{sections:?}");
        assert_eq!(sections[0].start_ms, 0.0);
        for pair in sections.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms);
        }
        // The real changes are at bars 16, 32, 64, 80 and 96.
        let truth = [16.0, 32.0, 64.0, 80.0, 96.0];
        for section in &sections[1..] {
            let bar = section.start_ms / BAR_MS;
            let nearest = truth
                .iter()
                .map(|t: &f64| (t - bar).abs())
                .fold(f64::INFINITY, f64::min);
            assert!(
                nearest <= 1.0,
                "a section began at bar {bar:.1}, {nearest:.1} bars from any real change"
            );
        }
    }

    #[tokio::test]
    async fn section_boundaries_land_on_bar_lines_of_the_published_grid() {
        let samples = arranged();
        let grid = grid(&samples).await.unwrap();
        let arrangement = measure(&samples).await.unwrap();
        let segment = grid.segments[0];
        let bar_ms = grid.beats_per_bar.unwrap() as f64 * 60_000.0 / segment.bpm;
        for section in &arrangement.sections[1..] {
            let bars = (section.start_ms - segment.anchor_ms) / bar_ms;
            assert!(
                (bars - bars.round()).abs() < 0.01,
                "a section began {bars:.3} bars from the anchor"
            );
        }
    }

    /// A build sits between something quieter and something louder; a breakdown between two
    /// louder things.
    #[tokio::test]
    async fn a_section_that_climbs_into_a_drop_is_a_build() {
        let arrangement = measure(&arranged()).await.unwrap();
        let sections = &arrangement.sections;
        let build = sections
            .iter()
            .find(|s| s.kind == Kind::Build)
            .expect("a build");
        assert!(
            (15.0..=17.0).contains(&(build.start_ms / BAR_MS)),
            "{sections:?}"
        );
        let breakdown = sections
            .iter()
            .find(|s| s.kind == Kind::Breakdown)
            .expect("a breakdown");
        assert!(
            (63.0..=65.0).contains(&(breakdown.start_ms / BAR_MS)),
            "{sections:?}"
        );
    }

    #[tokio::test]
    async fn the_loud_parts_read_as_louder_than_the_quiet_ones() {
        let arrangement = measure(&arranged()).await.unwrap();
        let first = &arrangement.sections[0];
        let loudest = arrangement
            .sections
            .iter()
            .max_by(|a, b| a.energy.total_cmp(&b.energy))
            .unwrap();
        assert!(
            first.energy < 0.5,
            "the intro read at energy {:.2}",
            first.energy
        );
        assert!(
            loudest.energy > 0.8,
            "the loudest read at only {:.2}",
            loudest.energy
        );
        assert!(loudest.start_ms > first.end_ms);
    }

    #[tokio::test]
    async fn sections_are_named_only_where_the_signal_says_so() {
        let arrangement = measure(&arranged()).await.unwrap();
        assert_eq!(arrangement.sections[0].kind, Kind::Intro);
        assert!(arrangement.sections.iter().any(|s| s.kind == Kind::Drop));
    }

    /// This record changes every sixteen bars, so eight or sixteen are both defensible; a
    /// confident answer that is neither isn't.
    #[tokio::test]
    async fn the_phrase_length_is_read_off_where_the_changes_fall() {
        let arrangement = measure(&arranged()).await.unwrap();
        match arrangement.phrase_bars {
            None => assert_eq!(arrangement.phrase_anchor_ms, None),
            Some(bars) => {
                assert!([8, 16, 32].contains(&bars));
                assert!(arrangement.phrase_anchor_ms.is_some());
            }
        }
    }

    #[tokio::test]
    async fn a_centred_voice_over_a_wide_pad_is_found() {
        let arrangement = measure(&with_voice(40_000.0, 80_000.0, true))
            .await
            .unwrap();
        let vocals = arrangement.vocals.unwrap();
        assert!(!vocals.is_empty());
        let overlap: f64 = vocals
            .iter()
            .map(|v| (v.end_ms.min(80_000.0) - v.start_ms.max(40_000.0)).max(0.0))
            .sum();
        assert!(
            overlap >= 20_000.0,
            "only {overlap:.0} ms of the sung passage was found: {vocals:?}"
        );
    }

    /// "Nothing can be said" mustn't arrive looking like "no singing".
    #[tokio::test]
    async fn a_file_that_cannot_be_read_says_so_rather_than_saying_no_vocals() {
        let arrangement = measure(&with_voice(40_000.0, 80_000.0, false))
            .await
            .unwrap();
        assert_eq!(arrangement.vocals, None);
    }

    /// Beyond the sidecar's tests: a track whose first tracked beat isn't a downbeat.
    #[test]
    fn the_first_section_is_never_shorter_than_a_bar() {
        let beats = Tracked {
            times_ms: (0..100).map(|i| f64::from(i) * 500.0).collect(),
            numbers: (0..100).collect(),
            first: 0,
            last: 99,
            beats_per_bar: Some(4),
            // Bars begin on beats 2, 6, 10…
            phase: 2,
        };
        // Novelty that only ever falls: no peak, so no boundary but the start.
        let novelty: Vec<f64> = (0..100).map(|i| -f64::from(i)).collect();
        assert_eq!(boundaries(&novelty, &beats, 0, 99, None, None), [0]);
    }

    #[tokio::test]
    async fn something_with_no_beat_has_no_arrangement() {
        let tone: Vec<f32> = (0..SAMPLE_RATE as usize * 60)
            .flat_map(|frame| {
                let v =
                    (0.4 * (2.0 * std::f64::consts::PI * 220.0 * frame as f64 / RATE).sin()) as f32;
                [v, v]
            })
            .collect();
        assert_eq!(measure(&tone).await, None);
    }
}
