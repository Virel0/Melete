//! The DJ planner: a mix between two records, beat-matched, bar-aligned and
//! handed over on a downbeat, or a refusal, which means crossfade. The sidecar's port of the
//! clients' `DJPlanner`, `MixScore`, `CamelotKey` and grid arithmetic, held to the clients'
//! forty-one-pair parity table: the ranking's opinion of a pair and the client's plan for it
//! have to be the same number, or a set books a mix the ranking didn't choose it for.

use serde::Deserialize;

mod camelot;
mod planning;
mod reading;
mod scoring;

pub use camelot::*;
pub use planning::*;
pub use scoring::*;

/// The shortest a mix is; anything less is a crossfade.
pub const MINIMUM_BARS: u32 = 8;
/// How far a record may be bent to meet another.
pub const MAXIMUM_STRETCH: f64 = 0.06;
/// How long the incoming record takes to find its own tempo again.
pub const RESTORE_SECONDS: f64 = 8.0;
/// The lengths tried, longest first.
const LENGTHS: [u32; 3] = [32, 16, 8];
/// How far a moment may move to sit on a phrase line, in bars: further, and the line isn't
/// where the music changes.
const PHRASE_REACH_BARS: f64 = 2.0;

/// One stretch of a grid, as stored and served.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// Where the stretch starts, in milliseconds.
    pub start_ms: f64,
    /// Where it ends, in milliseconds.
    pub end_ms: f64,
    /// Beat zero, on a downbeat when the meter is known: `beat(n) = anchor_ms + n · 60000 / bpm`.
    pub anchor_ms: f64,
    /// Its tempo, in beats a minute.
    pub bpm: f64,
    /// How far the track's own beats sit from the fit (RMS, in milliseconds); a blend locks only to
    /// a grid the track sits close to.
    pub residual_ms: f64,
}

/// A record's beat grid, as stored and served.
#[derive(Debug, Clone)]
pub struct Grid {
    /// One fit, or several where the track changes tempo; together they cover it.
    pub segments: Vec<Segment>,
    /// Beats in a bar; none when the meter wasn't clear, and then nothing is beat-matched.
    pub beats_per_bar: Option<i64>,
    /// The first downbeat of the first bar really playing; none refuses a mix into the record.
    pub mix_in_ms: Option<f64>,
    /// The downbeat of the last bar still in the groove; none refuses a mix out of the record.
    pub mix_out_ms: Option<f64>,
}

/// One section of a record's arrangement.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    /// Where it starts, in milliseconds.
    pub start_ms: f64,
    /// Where it ends, in milliseconds.
    pub end_ms: f64,
    /// What it is: `intro`, `build`, `drop`, `breakdown`, `outro` or `unknown`.
    pub kind: String,
    /// How loud it is within the record, 0 to 1.
    pub energy: f64,
}

/// A stretch where someone sings, in milliseconds.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VocalSpan {
    /// Where the singing starts.
    pub start_ms: f64,
    /// Where it ends.
    pub end_ms: f64,
}

/// A record's structure: sections, phrases and singing.
#[derive(Debug, Clone)]
pub struct Arrangement {
    /// Contiguous, covering the whole record.
    pub sections: Vec<Section>,
    /// How many bars a phrase runs (8, 16 or 32); none where the structure wouldn't commit.
    pub phrase_bars: Option<i64>,
    /// A downbeat that begins a phrase, in milliseconds; none with `phrase_bars`.
    pub phrase_anchor_ms: Option<f64>,
    /// None when it couldn't be told, which counts as singing.
    pub vocals: Option<Vec<VocalSpan>>,
}

/// A record as the planner sees it.
#[derive(Clone, Copy)]
pub struct MixRecord<'a> {
    /// Its beat grid.
    pub grid: &'a Grid,
    /// Its Camelot key, as `8A`; none when unknown.
    pub key: Option<&'a str>,
    /// Its arrangement; none when it hasn't been measured.
    pub arrangement: Option<&'a Arrangement>,
}

/// How a deck plays a mix without parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// The standard blend, for pairs that agree.
    Blend,
    /// The filter fade, for pairs that clash in key or where one is singing.
    FilterFade,
}

/// A mix: where each record starts, for how long, at what rates, and how it sounds.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    /// Where the mix starts in the outgoing record, on a downbeat, in milliseconds.
    pub outgoing_start_ms: f64,
    /// Where the incoming record starts, in milliseconds of it.
    pub incoming_start_ms: f64,
    /// How many bars the mix runs: 32, 16 or 8.
    pub bars: u32,
    /// How long the mix runs, in seconds.
    pub seconds: f64,
    /// The tempo both play at during the mix: the outgoing record's, in beats a minute.
    pub target_bpm: f64,
    /// The outgoing record's rate: always 1.0, as the planner only bends the incoming one.
    pub outgoing_rate: f64,
    /// The rate the incoming record is bent to (pitch held), at most [`MAXIMUM_STRETCH`] from 1.0.
    pub incoming_rate: f64,
    /// How long the incoming record takes afterwards to ease back to its own tempo, in seconds.
    pub restore_seconds: f64,
    /// Whether the two keys sit together on the Camelot wheel.
    pub key_compatible: bool,
    /// How well the pair suits each other, factor by factor.
    pub score: MixScore,
    /// How it sounds when played without parts.
    pub style: Style,
}

#[cfg(test)]
mod tests;
