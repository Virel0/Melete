//! The clients' forty-one-pair parity table, verbatim: starts and entries to the
//! nanosecond, totals to twelve decimals, styles included.

use super::*;

const BAR: f64 = 60_000.0 / 128.0 * 4.0;

#[derive(Clone)]
struct Gridded {
    grid: Grid,
    key: Option<&'static str>,
}

#[derive(Default)]
struct R {
    anchor_ms: f64,
    length_ms: Option<f64>,
    residual_ms: Option<f64>,
    key: Option<&'static str>,
    mix_in_ms: Option<f64>,
    mix_out_ms: Option<f64>,
    no_meter: bool,
}

/// A four-minute record at a tempo, gridded from an anchor, mixable from its first
/// downbeat to two bars before the end.
fn record(bpm: f64, r: R) -> Gridded {
    let length_ms = r.length_ms.unwrap_or(240_000.0);
    let bar_ms = 60_000.0 / bpm * 4.0;
    let meter = !r.no_meter;
    Gridded {
        grid: Grid {
            segments: vec![Segment {
                start_ms: 0.0,
                end_ms: length_ms,
                anchor_ms: r.anchor_ms,
                bpm,
                residual_ms: r.residual_ms.unwrap_or(5.0),
            }],
            beats_per_bar: meter.then_some(4),
            mix_in_ms: meter.then(|| r.mix_in_ms.unwrap_or(r.anchor_ms)),
            mix_out_ms: meter.then(|| {
                r.mix_out_ms
                    .unwrap_or(r.anchor_ms + (length_ms - r.anchor_ms - bar_ms * 2.0))
            }),
        },
        key: r.key,
    }
}

/// A four-minute record at 128, mixable from bar 8 to sixteen bars before the end.
fn grid(mix_in_ms: Option<f64>, mix_out_ms: Option<f64>, key: Option<&'static str>) -> Gridded {
    record(
        128.0,
        R {
            key,
            mix_in_ms: Some(mix_in_ms.unwrap_or(BAR * 8.0)),
            mix_out_ms: Some(mix_out_ms.unwrap_or(BAR * 112.0)),
            ..R::default()
        },
    )
}

fn g() -> Gridded {
    grid(None, None, None)
}

/// Sections in bars, laid end to end.
fn arranged(
    phrase_bars: Option<i64>,
    vocals: Option<Vec<VocalSpan>>,
    parts: &[(&str, u32, f64)],
) -> Arrangement {
    let mut at = 0.0;
    let sections = parts
        .iter()
        .map(|&(kind, bars, energy)| {
            let section = Section {
                start_ms: at,
                end_ms: at + BAR * f64::from(bars),
                kind: kind.to_owned(),
                energy,
            };
            at += BAR * f64::from(bars);
            section
        })
        .collect();
    Arrangement {
        sections,
        phrase_bars,
        phrase_anchor_ms: phrase_bars.map(|_| 0.0),
        vocals,
    }
}

fn a(parts: &[(&str, u32, f64)]) -> Arrangement {
    arranged(Some(16), Some(vec![]), parts)
}

fn sung(spans: &[(f64, f64)], parts: &[(&str, u32, f64)]) -> Arrangement {
    let spans = spans
        .iter()
        .map(|&(s, e)| VocalSpan {
            start_ms: BAR * s,
            end_ms: BAR * e,
        })
        .collect();
    arranged(Some(16), Some(spans), parts)
}

/// Read, and found unremarkable: one unnamed section, instrumental, no phrase grid.
fn read(energy: f64) -> Arrangement {
    Arrangement {
        sections: vec![Section {
            start_ms: 0.0,
            end_ms: 600_000.0,
            kind: "unknown".into(),
            energy,
        }],
        phrase_bars: None,
        phrase_anchor_ms: None,
        vocals: Some(vec![]),
    }
}

struct Case {
    why: &'static str,
    outgoing: Gridded,
    incoming: Gridded,
    out_arrangement: Option<Arrangement>,
    in_arrangement: Option<Arrangement>,
    not_before_ms: f64,
    expected: Option<(u32, f64, f64, f64, f64, Style)>,
}

type Expected = Option<(u32, f64, f64, f64, f64, Style)>;

fn unremarkable(
    why: &'static str,
    outgoing: Gridded,
    incoming: Gridded,
    expected: Expected,
) -> Case {
    Case {
        why,
        outgoing,
        incoming,
        out_arrangement: Some(read(0.8)),
        in_arrangement: Some(read(0.2)),
        not_before_ms: 0.0,
        expected,
    }
}

fn case(
    why: &'static str,
    outgoing: Gridded,
    incoming: Gridded,
    out_arrangement: Option<Arrangement>,
    in_arrangement: Option<Arrangement>,
    expected: Expected,
) -> Case {
    Case {
        why,
        outgoing,
        incoming,
        out_arrangement,
        in_arrangement,
        not_before_ms: 0.0,
        expected,
    }
}

fn blend(bars: u32, start: f64, entry: f64, rate: f64, total: f64) -> Expected {
    Some((bars, start, entry, rate, total, Style::Blend))
}

fn fade(bars: u32, start: f64, entry: f64, rate: f64, total: f64) -> Expected {
    Some((bars, start, entry, rate, total, Style::FilterFade))
}

fn r128() -> Gridded {
    record(128.0, R::default())
}

fn table() -> Vec<Case> {
    let slow_bpm = 128.0 / 1.01;
    let slow_mix_in = 60_000.0 / slow_bpm * 4.0 * 50.0;
    let plain = a(&[("intro", 16, 0.2), ("drop", 96, 0.9), ("outro", 16, 0.2)]);
    let drop_out = a(&[("intro", 16, 0.2), ("drop", 112, 0.9)]);
    let drop_in = a(&[("drop", 128, 0.9)]);
    let build_in = a(&[("build", 32, 0.4), ("drop", 96, 0.9)]);
    let with_build = a(&[
        ("intro", 16, 0.2),
        ("build", 16, 0.5),
        ("drop", 80, 0.95),
        ("outro", 16, 0.2),
    ]);
    let mid_energy = a(&[("intro", 16, 0.2), ("drop", 96, 0.55), ("outro", 16, 0.2)]);
    let breakdown_out = a(&[
        ("intro", 16, 0.2),
        ("drop", 80, 0.9),
        ("breakdown", 16, 0.5),
        ("outro", 16, 0.2),
    ]);
    let singing_out = sung(&[(90.0, 128.0)], &[("drop", 128, 0.9)]);
    let singing_in = sung(&[(0.0, 40.0)], &[("drop", 128, 0.9)]);
    let sings_later = sung(&[(40.0, 80.0)], &[("drop", 128, 0.9)]);
    let cannot_tell = arranged(Some(16), None, &[("drop", 128, 0.9)]);
    let instrumental = a(&[("drop", 128, 0.9)]);
    let no_phrases = arranged(None, Some(vec![]), &[("unknown", 128, 0.5)]);
    let build_then_drop = a(&[("build", 32, 0.4), ("drop", 96, 0.9)]);
    let singing_throughout = sung(&[(0.0, 128.0)], &[("build", 32, 0.4), ("drop", 96, 0.9)]);
    let sings_at_eighty = sung(&[(80.0, 90.0)], &[("drop", 128, 1.0)]);
    let quiet_drop = a(&[("drop", 128, 0.0)]);
    let mut not_before_160 = unremarkable(
        "a mix can't start before the engine could have got ready",
        record(
            128.0,
            R {
                mix_out_ms: Some(180_000.0),
                ..R::default()
            },
        ),
        r128(),
        blend(8, 165_000.0, 0.0, 1.0, 0.68),
    );
    not_before_160.not_before_ms = 160_000.0;
    let mut not_before_179 = unremarkable(
        "and too late is a crossfade",
        record(
            128.0,
            R {
                mix_out_ms: Some(180_000.0),
                ..R::default()
            },
        ),
        r128(),
        None,
    );
    not_before_179.not_before_ms = 179_000.0;
    vec![
        unremarkable(
            "two records at one tempo get sixteen bars",
            r128(),
            r128(),
            blend(16, 206_250.0, 0.0, 1.0, 0.68),
        ),
        unremarkable(
            "the mix ends where the outgoing record stops being mixable",
            record(
                128.0,
                R {
                    mix_out_ms: Some(180_000.0),
                    ..R::default()
                },
            ),
            r128(),
            blend(16, 150_000.0, 0.0, 1.0, 0.68),
        ),
        unremarkable(
            "an anchor off zero keeps the start on its own downbeats",
            record(
                128.0,
                R {
                    anchor_ms: 431.2,
                    mix_out_ms: Some(431.2 + 1875.0 * 64.0),
                    ..R::default()
                },
            ),
            r128(),
            blend(16, 90_431.2, 0.0, 1.0, 0.68),
        ),
        unremarkable(
            "126 into 128 bends the incoming record up",
            r128(),
            record(126.0, R::default()),
            blend(16, 206_250.0, 0.0, 1.015_873_015_873_015_8, 0.68),
        ),
        unremarkable(
            "130 into 128 bends it down",
            r128(),
            record(130.0, R::default()),
            blend(16, 206_250.0, 0.0, 0.984_615_384_615_384_7, 0.68),
        ),
        unremarkable(
            "100 into 128 is a different record",
            r128(),
            record(100.0, R::default()),
            None,
        ),
        unremarkable(
            "double time is played at its own speed",
            record(85.0, R::default()),
            record(170.0, R::default()),
            blend(16, 189_176.470_588_235_3, 0.0, 1.0, 0.68),
        ),
        unremarkable(
            "and so is half time",
            record(170.0, R::default()),
            record(85.0, R::default()),
            blend(16, 214_588.235_294_117_65, 0.0, 1.0, 0.68),
        ),
        unremarkable(
            "eight bars when sixteen won't fit",
            record(
                128.0,
                R {
                    mix_out_ms: Some(20_000.0),
                    ..R::default()
                },
            ),
            r128(),
            blend(8, 3_750.0, 0.0, 1.0, 0.68),
        ),
        unremarkable(
            "and nothing when eight won't",
            record(
                128.0,
                R {
                    mix_out_ms: Some(10_000.0),
                    ..R::default()
                },
            ),
            r128(),
            None,
        ),
        not_before_160,
        not_before_179,
        unremarkable(
            "no meter behind, no mix",
            record(
                128.0,
                R {
                    no_meter: true,
                    ..R::default()
                },
            ),
            r128(),
            None,
        ),
        unremarkable(
            "no meter ahead, no mix",
            r128(),
            record(
                128.0,
                R {
                    no_meter: true,
                    ..R::default()
                },
            ),
            None,
        ),
        unremarkable(
            "a grid its own record doesn't sit on",
            r128(),
            record(
                128.0,
                R {
                    residual_ms: Some(60.0),
                    ..R::default()
                },
            ),
            None,
        ),
        unremarkable(
            "the incoming record must have the bars to give",
            r128(),
            record(
                128.0,
                R {
                    length_ms: Some(110_000.0),
                    mix_in_ms: Some(100_000.0),
                    mix_out_ms: Some(105_000.0),
                    ..R::default()
                },
            ),
            None,
        ),
        unremarkable(
            "a bent record is consumed faster than the clock: thirty seconds isn't enough",
            r128(),
            record(
                slow_bpm,
                R {
                    length_ms: Some(slow_mix_in + 30_100.0),
                    mix_in_ms: Some(slow_mix_in),
                    mix_out_ms: Some(120_000.0),
                    ..R::default()
                },
            ),
            blend(8, 221_250.0, 94_687.5, 1.01, 0.68),
        ),
        unremarkable(
            "half a second more record and it gets the sixteen",
            r128(),
            record(
                slow_bpm,
                R {
                    length_ms: Some(slow_mix_in + 30_600.0),
                    mix_in_ms: Some(slow_mix_in),
                    mix_out_ms: Some(120_000.0),
                    ..R::default()
                },
            ),
            blend(16, 206_250.0, 94_687.5, 1.01, 0.68),
        ),
        unremarkable(
            "records that clash are capped at eight and get the filter fade",
            record(
                128.0,
                R {
                    key: Some("3B"),
                    ..R::default()
                },
            ),
            record(
                128.0,
                R {
                    key: Some("10B"),
                    ..R::default()
                },
            ),
            fade(8, 221_250.0, 0.0, 1.0, 0.58),
        ),
        unremarkable(
            "neighbours aren't capped, and the score decides on longer",
            record(
                128.0,
                R {
                    key: Some("8A"),
                    ..R::default()
                },
            ),
            record(
                128.0,
                R {
                    key: Some("9A"),
                    ..R::default()
                },
            ),
            blend(32, 176_250.0, 0.0, 1.0, 0.78),
        ),
        unremarkable(
            "both ends are put on the grid, whatever the mix points say",
            record(
                128.0,
                R {
                    mix_out_ms: Some(200_234.0),
                    ..R::default()
                },
            ),
            record(
                128.0,
                R {
                    anchor_ms: 100.0,
                    mix_in_ms: Some(40_051.0),
                    ..R::default()
                },
            ),
            blend(16, 168_750.0, 39_475.0, 1.0, 0.68),
        ),
        case(
            "an outgoing record that hasn't been read isn't mixed",
            g(),
            g(),
            None,
            Some(plain.clone()),
            None,
        ),
        case(
            "nor is an incoming one",
            g(),
            g(),
            Some(plain.clone()),
            None,
            None,
        ),
        case(
            "matching tempo is no substitute for knowing the arrangement",
            g(),
            g(),
            None,
            None,
            None,
        ),
        case(
            "a mix starts on the bar when the phrase line is more than two bars back",
            grid(None, Some(BAR * 115.0), None),
            g(),
            Some(plain.clone()),
            Some(plain.clone()),
            blend(32, 155_625.0, 0.0, 1.0, 0.760_000_000_000_000_1),
        ),
        case(
            "and comes in on one",
            g(),
            grid(Some(BAR * 11.0), None, None),
            Some(plain.clone()),
            Some(plain.clone()),
            blend(32, 150_000.0, 0.0, 1.0, 0.760_000_000_000_000_1),
        ),
        case(
            "an entry a bar short of a phrase line moves forward onto it",
            g(),
            grid(Some(BAR * 15.0), None, None),
            Some(plain.clone()),
            Some(plain.clone()),
            blend(32, 150_000.0, 30_000.0, 1.0, 0.800_000_000_000_000_2),
        ),
        case(
            "without a phrase grid, a bar line is the best there is",
            grid(None, Some(BAR * 115.0), None),
            grid(Some(BAR * 11.0), None, None),
            Some(no_phrases.clone()),
            Some(no_phrases),
            blend(32, 155_625.0, 20_625.0, 1.0, 0.800_000_000_000_000_2),
        ),
        case(
            "two records singing at once isn't a mix",
            g(),
            g(),
            Some(singing_out.clone()),
            Some(singing_in.clone()),
            None,
        ),
        case(
            "one voice is fine, and gets the filter fade",
            g(),
            g(),
            Some(singing_out),
            Some(sings_later),
            fade(8, 195_000.0, 15_000.0, 1.0, 0.665),
        ),
        case(
            "not being able to tell counts as singing",
            g(),
            g(),
            Some(cannot_tell.clone()),
            Some(singing_in),
            None,
        ),
        case(
            "looked and found none isn't the same as couldn't tell",
            g(),
            g(),
            Some(cannot_tell),
            Some(instrumental),
            fade(8, 195_000.0, 15_000.0, 1.0, 0.665),
        ),
        case(
            "a drop onto a drop gets eight bars, not sixteen",
            g(),
            g(),
            Some(drop_out.clone()),
            Some(drop_in),
            blend(8, 195_000.0, 15_000.0, 1.0, 0.725_000_000_000_000_1),
        ),
        case(
            "coming in on a build keeps the sixteen",
            g(),
            g(),
            Some(drop_out),
            Some(build_in),
            blend(16, 180_000.0, 0.0, 1.0, 0.725_000_000_000_000_1),
        ),
        case(
            "a record comes in on its build when it has one",
            g(),
            g(),
            Some(mid_energy),
            Some(with_build.clone()),
            blend(32, 150_000.0, 30_000.0, 1.0, 0.890_000_000_000_000_1),
        ),
        case(
            "everything right earns thirty-two",
            grid(None, None, Some("9A")),
            grid(None, None, Some("8A")),
            Some(breakdown_out),
            Some(with_build),
            blend(32, 150_000.0, 30_000.0, 1.0, 0.92),
        ),
        case(
            "compatible keys and nobody singing is a blend",
            grid(None, None, Some("8A")),
            grid(None, None, Some("9A")),
            Some(build_then_drop.clone()),
            Some(build_then_drop.clone()),
            blend(32, 150_000.0, 0.0, 1.0, 0.825),
        ),
        case(
            "clashing keys are a capped filter fade",
            grid(None, None, Some("3B")),
            grid(None, None, Some("10B")),
            Some(build_then_drop.clone()),
            Some(build_then_drop.clone()),
            fade(8, 195_000.0, 0.0, 1.0, 0.625),
        ),
        case(
            "the outgoing record singing is a filter fade, capped at eight",
            grid(None, None, Some("8A")),
            grid(None, None, Some("9A")),
            Some(singing_throughout.clone()),
            Some(build_then_drop.clone()),
            fade(8, 195_000.0, 0.0, 1.0, 0.764_999_999_999_999_9),
        ),
        case(
            "and so is the incoming one",
            grid(None, None, Some("8A")),
            grid(None, None, Some("9A")),
            Some(build_then_drop),
            Some(singing_throughout),
            fade(8, 195_000.0, 0.0, 1.0, 0.764_999_999_999_999_9),
        ),
        case(
            "a score refused at thirty-two bars refuses the entry, though sixteen would have been clean",
            grid(None, None, Some("3B")),
            record(
                128.0 / 1.03,
                R {
                    key: Some("10B"),
                    mix_in_ms: Some(BAR * 8.0),
                    mix_out_ms: Some(BAR * 112.0),
                    ..R::default()
                },
            ),
            Some(sings_at_eighty),
            Some(quiet_drop),
            None,
        ),
    ]
}

#[test]
fn plans_the_clients_table_identically() {
    let table = table();
    assert_eq!(table.len(), 41);
    for row in &table {
        let outgoing = MixRecord {
            grid: &row.outgoing.grid,
            key: row.outgoing.key,
            arrangement: row.out_arrangement.as_ref(),
        };
        let incoming = MixRecord {
            grid: &row.incoming.grid,
            key: row.incoming.key,
            arrangement: row.in_arrangement.as_ref(),
        };
        let plan = plan(outgoing, incoming, row.not_before_ms);
        let why = row.why;
        let Some((bars, start, entry, rate, total, style)) = row.expected else {
            assert!(plan.is_none(), "{why}: expected a refusal, got {plan:?}");
            continue;
        };
        let plan = plan.unwrap_or_else(|| panic!("{why}: expected {bars} bars, got a refusal"));
        assert_eq!(plan.bars, bars, "{why}");
        assert!(
            (plan.outgoing_start_ms - start).abs() < 1e-9,
            "{why}: start {}",
            plan.outgoing_start_ms
        );
        assert!(
            (plan.incoming_start_ms - entry).abs() < 1e-9,
            "{why}: entry {}",
            plan.incoming_start_ms
        );
        assert!(
            (plan.incoming_rate - rate).abs() < 1e-12,
            "{why}: rate {}",
            plan.incoming_rate
        );
        assert!(
            (plan.score.total() - total).abs() < 1e-12,
            "{why}: total {}",
            plan.score.total()
        );
        assert_eq!(plan.style, style, "{why}");
        assert_eq!(
            (plan.outgoing_rate, plan.restore_seconds),
            (1.0, 8.0),
            "{why}"
        );
    }
}
