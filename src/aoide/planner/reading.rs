//! Reading a grid and an arrangement: segments, bars, downbeats and whether a blend may lock to
//! the grid; sections, singing and phrase lines.

use super::*;

impl Grid {
    /// The segment holding a moment, or the nearest.
    pub(crate) fn segment(&self, ms: f64) -> Option<&Segment> {
        self.segments
            .iter()
            .find(|s| ms >= s.start_ms && ms < s.end_ms)
            .or_else(|| self.segments.iter().rev().find(|s| s.start_ms <= ms))
            .or_else(|| self.segments.first())
    }

    pub(crate) fn bar_ms(&self, ms: f64) -> Option<f64> {
        Some(60_000.0 / self.segment(ms)?.bpm * self.beats_per_bar? as f64)
    }

    /// The downbeat at or before a moment; every segment's beat zero is one.
    pub(crate) fn downbeat_at_or_before(&self, ms: f64) -> Option<f64> {
        let beats_per_bar = self.beats_per_bar?;
        let segment = self.segment(ms)?;
        let bar_ms = 60_000.0 / segment.bpm * beats_per_bar as f64;
        let bars = ((ms - segment.anchor_ms) / bar_ms).floor();
        Some(segment.anchor_ms + bars * bar_ms)
    }

    /// Whether a blend this long may lock to the grid here: on a grid the track sits on,
    /// finishing inside the segment it started in.
    pub(crate) fn can_lock(&self, ms: f64, blend_seconds: f64) -> bool {
        if self.beats_per_bar.is_none() {
            return false;
        }
        let Some(segment) = self.segment(ms) else {
            return false;
        };
        segment.residual_ms <= maximum_residual_ms(blend_seconds)
            && ms + blend_seconds * 1000.0 <= segment.end_ms
    }
}

/// How far off its own grid a track may sit and still be locked to, for a blend this long.
fn maximum_residual_ms(blend_seconds: f64) -> f64 {
    if blend_seconds <= 0.0 {
        return 0.0;
    }
    (20.0 * (30.0 / blend_seconds)).min(40.0)
}

impl Arrangement {
    pub(crate) fn section_at(&self, ms: f64) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| ms >= s.start_ms && ms < s.end_ms)
            .or_else(|| self.sections.iter().rev().find(|s| s.start_ms <= ms))
    }

    /// Whether anyone might be singing in this stretch. Unknown is yes.
    pub(crate) fn may_sing(&self, from: f64, to: f64) -> bool {
        self.vocals
            .as_ref()
            .is_none_or(|spans| spans.iter().any(|v| v.start_ms < to && v.end_ms > from))
    }

    pub(super) fn phrase_start_at_or_before(&self, ms: f64, bar_ms: f64) -> Option<f64> {
        let (bars, anchor) = (self.phrase_bars?, self.phrase_anchor_ms?);
        if bar_ms <= 0.0 {
            return None;
        }
        let phrase_ms = bar_ms * bars as f64;
        Some(anchor + ((ms - anchor) / phrase_ms).floor() * phrase_ms)
    }

    /// The phrase line at or before a moment, if close: for moments that mustn't move later.
    pub(super) fn phrase_line_at_or_before(&self, ms: f64, bar_ms: f64) -> Option<f64> {
        let line = self.phrase_start_at_or_before(ms, bar_ms)?;
        (ms - line <= PHRASE_REACH_BARS * bar_ms + 0.5).then_some(line)
    }

    /// The nearest phrase line either side, if close: for an entry.
    pub(crate) fn phrase_line_near(&self, ms: f64, bar_ms: f64) -> Option<f64> {
        let bars = self.phrase_bars?;
        if bar_ms <= 0.0 {
            return None;
        }
        let before = self.phrase_start_at_or_before(ms, bar_ms)?;
        let after = before + bar_ms * bars as f64;
        let nearest = if ms - before <= after - ms {
            before
        } else {
            after
        };
        ((ms - nearest).abs() <= PHRASE_REACH_BARS * bar_ms + 0.5).then_some(nearest)
    }
}
