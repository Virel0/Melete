//! Planning a mix: the bend that meets the other tempo, the entry points tried, each scored and
//! fitted longest length first, and the best of them.

use super::*;

/// The rate the incoming deck runs at to sit on the outgoing tempo, folded into one octave,
/// or None when that's further than a record may be bent.
pub fn bendable_rate(bpm: f64, target: f64) -> Option<f64> {
    if bpm <= 0.0 || target <= 0.0 || !bpm.is_finite() || !target.is_finite() {
        return None;
    }
    let mut ratio = target / bpm;
    while ratio > 1.5 {
        ratio /= 2.0;
    }
    while ratio < 1.0 / 1.5 {
        ratio *= 2.0;
    }
    ((ratio - 1.0).abs() <= MAXIMUM_STRETCH + 1e-9).then_some(ratio)
}

/// An entry on the nearest phrase line when one's close, else on the bar line.
pub(super) fn entry_point(ms: f64, grid: &Grid, arrangement: &Arrangement, bar_ms: f64) -> f64 {
    arrangement
        .phrase_line_near(ms, bar_ms)
        .or_else(|| grid.downbeat_at_or_before(ms))
        .unwrap_or(ms)
}

/// A mix from `outgoing` into `incoming`, or None: crossfade. `not_before_ms` is the earliest
/// the mix may start in the outgoing record.
pub fn plan(
    outgoing: MixRecord<'_>,
    incoming: MixRecord<'_>,
    not_before_ms: f64,
) -> Option<Transition> {
    // Both read: matching tempo is no substitute for knowing the arrangement.
    outgoing.arrangement?;
    let in_arrangement = incoming.arrangement?;
    let out_exit = outgoing.grid.mix_out_ms?;
    let in_entry = incoming.grid.mix_in_ms?;
    let out_segment = *outgoing.grid.segment(out_exit)?;
    outgoing.grid.beats_per_bar?;
    incoming.grid.beats_per_bar?;
    let out_bar = outgoing.grid.bar_ms(out_exit)?;
    let in_bar = incoming.grid.bar_ms(in_entry)?;
    let rate = bendable_rate(
        incoming.grid.segment(in_entry).map_or(0.0, |s| s.bpm),
        out_segment.bpm,
    )?;
    // Its mix-in point always, and the start of every build, intro and breakdown.
    let mut entries = vec![entry_point(in_entry, incoming.grid, in_arrangement, in_bar)];
    for section in &in_arrangement.sections {
        if matches!(section.kind.as_str(), "build" | "intro" | "breakdown") {
            entries.push(entry_point(
                section.start_ms,
                incoming.grid,
                in_arrangement,
                in_bar,
            ));
        }
    }
    let mut distinct: Vec<f64> = Vec::new();
    for entry in entries {
        if !distinct.contains(&entry) {
            distinct.push(entry);
        }
    }
    distinct.sort_by(f64::total_cmp);
    // Best score wins; on a tie, the earlier, thinner entry.
    let mut best: Option<Transition> = None;
    for entry in distinct {
        let candidate = plan_entry(
            entry,
            rate,
            outgoing,
            incoming,
            out_exit,
            out_bar,
            out_segment.bpm,
            not_before_ms,
        );
        if let Some(candidate) = candidate
            && best
                .as_ref()
                .is_none_or(|b| candidate.score.total() > b.score.total())
        {
            best = Some(candidate);
        }
    }
    best
}

/// One entry point, scored and fitted, longest length first.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_entry(
    entry: f64,
    rate: f64,
    outgoing: MixRecord<'_>,
    incoming: MixRecord<'_>,
    out_mix_out: f64,
    out_bar: f64,
    out_segment_bpm: f64,
    not_before_ms: f64,
) -> Option<Transition> {
    let in_segment = *incoming.grid.segment(entry)?;
    let (exit_kind, entry_kind, capped) = sections_and_cap(outgoing, incoming, out_mix_out, entry);
    for bars in LENGTHS {
        let seconds = out_bar * f64::from(bars) / 1000.0;
        let start_ms = mix_start(outgoing, out_mix_out, out_bar, bars);
        if start_ms < not_before_ms
            || !outgoing.grid.can_lock(start_ms, seconds)
            || !incoming.grid.can_lock(entry, seconds)
            // A bent record is eaten faster than the clock.
            || entry + seconds * 1000.0 * rate > in_segment.end_ms
        {
            continue;
        }
        let (outgoing_sings, incoming_sings) =
            singing(outgoing, incoming, start_ms, entry, seconds, rate);
        if outgoing_sings == Some(true) && incoming_sings == Some(true) {
            continue;
        }
        let score = MixScore {
            tempo: MixScore::tempo_factor(rate),
            key: MixScore::key_factor(outgoing.key, incoming.key),
            vocals: MixScore::vocals_factor(outgoing_sings, incoming_sings),
            energy: MixScore::energy_factor(
                outgoing
                    .arrangement
                    .and_then(|a| a.section_at(start_ms))
                    .map(|s| s.energy),
                incoming
                    .arrangement
                    .and_then(|a| a.section_at(entry))
                    .map(|s| s.energy),
            ),
            sections: MixScore::sections_factor(exit_kind, entry_kind),
        };
        // A length that scores nothing refuses the entry outright, as the clients do.
        let earned = score.bars()?;
        let allowed = if capped {
            earned.min(MINIMUM_BARS)
        } else {
            earned
        };
        if bars > allowed {
            continue;
        }
        let start = outgoing
            .grid
            .downbeat_at_or_before(start_ms)
            .unwrap_or(start_ms);
        let anyone_sings = outgoing_sings == Some(true) || incoming_sings == Some(true);
        let (style, key_compatible) = style_of(outgoing, incoming, anyone_sings);
        // A filter fade is capped too: thirty-two bars of a voice filtered under another
        // record is thirty-two bars of clash.
        if style != Style::Blend && bars > MINIMUM_BARS {
            continue;
        }
        return Some(Transition {
            outgoing_start_ms: start,
            incoming_start_ms: entry,
            bars,
            seconds,
            target_bpm: out_segment_bpm,
            outgoing_rate: 1.0,
            incoming_rate: rate,
            restore_seconds: RESTORE_SECONDS,
            key_compatible,
            score,
            style,
        });
    }
    None
}

/// The kinds of section the mix leaves and enters, and whether the pair keeps it short.
fn sections_and_cap<'a>(
    outgoing: MixRecord<'a>,
    incoming: MixRecord<'a>,
    out_mix_out: f64,
    entry: f64,
) -> (Option<&'a str>, Option<&'a str>, bool) {
    let exit_kind = outgoing
        .arrangement
        .and_then(|a| a.section_at(out_mix_out))
        .map(|s| s.kind.as_str());
    let entry_kind = incoming
        .arrangement
        .and_then(|a| a.section_at(entry))
        .map(|s| s.kind.as_str());
    // A drop onto a drop, or keys known to clash, doesn't get long together.
    let drop_on_drop = exit_kind == Some("drop") && entry_kind == Some("drop");
    let keys_clash =
        outgoing.key.is_some() && incoming.key.is_some() && !compatible(outgoing.key, incoming.key);
    let capped = drop_on_drop || keys_clash;
    (exit_kind, entry_kind, capped)
}

/// Where a mix of `bars` bars starts in the outgoing record.
fn mix_start(outgoing: MixRecord<'_>, out_mix_out: f64, out_bar: f64, bars: u32) -> f64 {
    // Back from the mix-out a whole number of bars, then onto a close phrase line, else
    // the bar line.
    let counted = out_mix_out - out_bar * f64::from(bars);
    outgoing
        .arrangement
        .and_then(|a| a.phrase_line_at_or_before(counted, out_bar))
        .or_else(|| outgoing.grid.downbeat_at_or_before(counted))
        .unwrap_or(counted)
}

/// Whether each record may be singing during the overlap.
fn singing(
    outgoing: MixRecord<'_>,
    incoming: MixRecord<'_>,
    start_ms: f64,
    entry: f64,
    seconds: f64,
    rate: f64,
) -> (Option<bool>, Option<bool>) {
    // Singing across exactly the overlap; unknown counts as singing.
    let outgoing_sings = outgoing
        .arrangement
        .map(|a| a.may_sing(start_ms, start_ms + seconds * 1000.0));
    let incoming_sings = incoming
        .arrangement
        .map(|a| a.may_sing(entry, entry + seconds * 1000.0 * rate));
    (outgoing_sings, incoming_sings)
}

/// The mix's style, a filter fade when the keys clash or anyone sings, and whether the keys
/// go together.
fn style_of(outgoing: MixRecord<'_>, incoming: MixRecord<'_>, anyone_sings: bool) -> (Style, bool) {
    let key_compatible = compatible(outgoing.key, incoming.key);
    let clashes = outgoing.key.is_some() && incoming.key.is_some() && !key_compatible;
    let style = if clashes || anyone_sings {
        Style::FilterFade
    } else {
        Style::Blend
    };
    (style, key_compatible)
}
