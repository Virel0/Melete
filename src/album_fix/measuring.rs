//! Measuring each track's loudness and the album's, and the ReplayGain gain and peak they give.

use super::*;

/// Each track's loudness, and the album's when every track long enough to measure was
/// measured.
pub(super) async fn measure(
    tools: &sound::Tools,
    tracks: &[PathBuf],
    notes: &mut Vec<String>,
) -> (Vec<Option<Loudness>>, Option<Loudness>) {
    let mut measured: Vec<Option<Loudness>> = Vec::new();
    let mut weighted: Vec<(Loudness, f64)> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for track in tracks {
        let path = track.clone();
        let seconds = tokio::task::spawn_blocking(move || duration(&path))
            .await
            .ok()
            .flatten();
        let seconds = match seconds {
            Some(seconds) if seconds >= loudness::MINIMUM_SECONDS as f64 => seconds,
            // Too short to measure: it neither gets a gain nor counts toward the album's.
            Some(_) => {
                measured.push(None);
                continue;
            }
            None => {
                failed.push(file_name(track));
                measured.push(None);
                continue;
            }
        };
        match sound::loudness_of(tools, track).await {
            Ok(Some(loudness)) => {
                measured.push(Some(loudness));
                weighted.push((loudness, seconds));
            }
            // Too quiet to measure: the same.
            Ok(None) => measured.push(None),
            Err(MeasureError::Unavailable(err)) => {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "no ReplayGain: ffmpeg can't run"
                );
                notes.push("no ReplayGain: ffmpeg can't run".into());
                return (vec![None; tracks.len()], None);
            }
            Err(err) => {
                tracing::warn!(file = %track.display(), error = %err, "couldn't measure a track");
                failed.push(file_name(track));
                measured.push(None);
            }
        }
    }
    if !failed.is_empty() {
        notes.push(format!("couldn't measure {}", failed.join(", ")));
        // An album gain from some of its tracks would be wrong for all of them.
        return (measured, None);
    }
    (measured, album_of(&weighted))
}

/// The album's loudness from its tracks' and their lengths: their energy averaged over the
/// album's length, which is what measuring the tracks back to back gives, but for gating at
/// the joins. Its peak is the loudest track's.
pub(super) fn album_of(tracks: &[(Loudness, f64)]) -> Option<Loudness> {
    let seconds: f64 = tracks.iter().map(|(_, s)| s).sum();
    if seconds <= 0.0 {
        return None;
    }
    let energy: f64 = tracks
        .iter()
        .map(|(l, s)| s * 10f64.powf(l.lufs / 10.0))
        .sum();
    Some(Loudness {
        lufs: 10.0 * (energy / seconds).log10(),
        true_peak_dbfs: tracks
            .iter()
            .map(|(l, _)| l.true_peak_dbfs)
            .fold(f64::NEG_INFINITY, f64::max),
    })
}

/// "-6.32 dB": the gain that brings it to the reference.
pub(super) fn gain(loudness: Loudness) -> String {
    format!("{:.2} dB", REFERENCE_LUFS - loudness.lufs)
}

/// The peak as a fraction of full scale. The true peak, which is never below the sample peak,
/// so a player limiting by it never clips.
pub(super) fn peak(loudness: Loudness) -> String {
    format!("{:.6}", 10f64.powf(loudness.true_peak_dbfs / 20.0))
}
