//! How loud a track is, by EBU R128, read from what ffmpeg's `loudnorm` filter prints: the
//! contract names that filter as the reference, so its numbers are used rather than a second
//! R128 implementation that would agree only approximately.

/// Quieter than this isn't a measurement: `loudnorm` reports −70 LUFS as its floor and `-inf`
/// for digital silence.
const SILENCE_FLOOR_LUFS: f64 = -70.0;
/// Shorter than this and `loudnorm`'s gating hasn't seen enough audio to mean anything.
pub const MINIMUM_SECONDS: i64 = 3;

/// A track's loudness, as `loudnorm` measured it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loudness {
    /// Integrated loudness over the whole track.
    pub lufs: f64,
    /// Above zero on a track clipped by its own master.
    pub true_peak_dbfs: f64,
}

/// The last `loudnorm` block in ffmpeg's log, or None when it printed nothing usable.
pub fn parse(log: &str) -> Option<Loudness> {
    let marker = log.rfind("\"input_i\"")?;
    let open = log[..marker].rfind('{')?;
    let close = marker + log[marker..].find('}')?;
    let block: serde_json::Value = serde_json::from_str(&log[open..=close]).ok()?;
    // Printed as quoted decimals.
    let number = |name: &str| {
        let value = &block[name];
        value
            .as_f64()
            .or_else(|| value.as_str()?.trim().parse().ok())
            .filter(|v: &f64| v.is_finite())
    };
    let lufs = number("input_i")?;
    let true_peak_dbfs = number("input_tp")?;
    // −inf, −70 and anything below: not enough signal, and a client must leave it alone.
    (lufs > SILENCE_FLOOR_LUFS).then_some(Loudness {
        lufs,
        true_peak_dbfs,
    })
}

#[cfg(test)]
mod tests {
    //! The sidecar's `LoudnessAnalyzerTests`.

    use super::*;

    /// A real run, verbatim: the block sits mid-log, with its numbers as quoted strings.
    const REAL_LOG: &str = "[aist#0:0/pcm_s16le @ 0xffff853a0300] Guessed Channel Layout: stereo
Input #0, wav, from '/tmp/tone.wav':
  Duration: 00:00:05.00, bitrate: 1411 kb/s
Stream mapping:
  Stream #0:0 (pcm_s16le) -> asplit:default
Output #0, f32le, to 'pipe:1':
[Parsed_loudnorm_1 @ 0xffff815b00c0]
{
\t\"input_i\" : \"-9.70\",
\t\"input_tp\" : \"-0.31\",
\t\"input_lra\" : \"6.40\",
\t\"input_thresh\" : \"-19.75\",
\t\"output_i\" : \"-24.03\",
\t\"output_tp\" : \"-23.97\",
\t\"output_lra\" : \"0.00\",
\t\"output_thresh\" : \"-34.03\",
\t\"normalization_type\" : \"dynamic\",
\t\"target_offset\" : \"0.03\"
}
[out#0/f32le @ 0xffff8538d280] video:0KiB audio:1723KiB subtitle:0KiB
size=    1723KiB time=00:00:05.00 bitrate=2822.4kbits/s speed=44.6x
";

    fn block(integrated: &str, true_peak: &str) -> String {
        format!(
            "[Parsed_loudnorm_1 @ 0x0]\n{{\n\t\"input_i\" : \"{integrated}\",\n\t\"input_tp\" : \"{true_peak}\",\n\t\"input_lra\" : \"0.00\",\n\t\"normalization_type\" : \"dynamic\"\n}}"
        )
    }

    #[test]
    fn the_measurement_is_read_out_of_the_log() {
        assert_eq!(
            parse(REAL_LOG),
            Some(Loudness {
                lufs: -9.70,
                true_peak_dbfs: -0.31
            })
        );
    }

    #[test]
    fn a_log_without_the_filter_yields_nothing() {
        assert_eq!(
            parse("Input #0, wav, from '/tmp/tone.wav':\nStream mapping:"),
            None
        );
    }

    #[test]
    fn something_too_quiet_to_measure_yields_nothing() {
        for reported in ["-70.00", "-120.00", "-inf"] {
            assert_eq!(parse(&block(reported, "-inf")), None, "{reported}");
        }
    }

    #[test]
    fn an_unparseable_number_yields_nothing() {
        assert_eq!(parse(&block("nan", "-1.0")), None);
    }

    #[test]
    fn a_true_peak_above_full_scale_is_kept() {
        assert_eq!(parse(&block("-6.20", "1.40")).unwrap().true_peak_dbfs, 1.40);
    }

    #[test]
    fn the_last_block_wins() {
        let log = format!("{}\n{}", block("-20.00", "-3.00"), block("-7.50", "-0.10"));
        assert_eq!(parse(&log).unwrap().lufs, -7.50);
    }
}
