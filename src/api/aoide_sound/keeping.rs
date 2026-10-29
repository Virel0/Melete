//! Bounds, loudness and tempo a client measured itself: all checked before any is kept.

use super::*;

/// `{ <key>: { id: entry | null } }`, at most [`MAX_IDS`] of them; anything else is a 400
/// that shows `shape`.
pub(super) fn body<T: DeserializeOwned>(
    bytes: &[u8],
    key: &str,
    shape: &str,
) -> Result<BTreeMap<String, Option<T>>, ApiError> {
    let malformed = || bad(format!("send {shape} with at most {MAX_IDS} ids"));
    let mut parsed: Value = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    let entries: BTreeMap<String, Option<T>> = parsed
        .get_mut(key)
        .map(Value::take)
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(malformed)?;
    if entries.len() > MAX_IDS {
        return Err(malformed());
    }
    Ok(entries)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BoundsEntry {
    pub(super) sound_start_ms: i64,
    pub(super) sound_end_ms: i64,
}

/// Bounds a client measured itself, served without decoding until the server measures the
/// file (the same algorithm over the same file). All are checked before any is kept.
pub(super) async fn post_bounds(
    State(pool): State<SqlitePool>,
    bytes: Bytes,
) -> Result<StatusCode, ApiError> {
    let entries: BTreeMap<String, Option<BoundsEntry>> = body(
        &bytes,
        "bounds",
        "{ bounds: { id: { soundStartMs, soundEndMs } | null } }",
    )?;
    for (id, entry) in &entries {
        if let Some(b) = entry
            && (b.sound_start_ms < 0 || b.sound_end_ms <= b.sound_start_ms)
        {
            return Err(bad(format!(
                "{id}: soundStartMs must be >= 0 and soundEndMs > soundStartMs."
            )));
        }
    }
    let mut tx = pool.begin().await?;
    for (id, entry) in &entries {
        let Ok(id) = id.parse::<i64>() else {
            continue;
        };
        sqlx::query(
            "INSERT INTO music_sound_bounds (track_id, modified_at, analyzer, source, error,
                 sound_start_ms, sound_end_ms, measured_at)
             SELECT id, modified_at, ?2, 'client', NULL, ?3, ?4, ?5 FROM music_tracks
             WHERE id = ?1 AND missing_since IS NULL
             ON CONFLICT (track_id) DO UPDATE SET modified_at = excluded.modified_at,
                 analyzer = excluded.analyzer, source = 'client', error = NULL,
                 sound_start_ms = excluded.sound_start_ms, sound_end_ms = excluded.sound_end_ms,
                 measured_at = excluded.measured_at",
        )
        .bind(id)
        .bind(ANALYZER)
        .bind(entry.as_ref().map(|b| b.sound_start_ms))
        .bind(entry.as_ref().map(|b| b.sound_end_ms))
        .bind(unix_ms())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AnalysisEntry {
    pub(super) loudness_lufs: Option<f64>,
    pub(super) true_peak_dbfs: Option<f64>,
    pub(super) bpm: Option<f64>,
    pub(super) bpm_confidence: Option<f64>,
    pub(super) bpm_stability: Option<f64>,
}

pub(super) fn invalid(entry: &AnalysisEntry) -> Option<String> {
    let unit = |v: Option<f64>| v.is_some_and(|v| !(0.0..=1.0).contains(&v));
    if entry
        .loudness_lufs
        .is_some_and(|l| !(MINIMUM_LOUDNESS_LUFS..=0.0).contains(&l))
    {
        return Some(format!(
            "loudnessLufs must be between {MINIMUM_LOUDNESS_LUFS} and 0."
        ));
    }
    if entry.loudness_lufs.is_none() != entry.true_peak_dbfs.is_none() {
        return Some("loudnessLufs and truePeakDbfs go together; send both or neither.".into());
    }
    if entry
        .bpm
        .is_some_and(|b| !(MINIMUM_BPM..=MAXIMUM_BPM).contains(&b))
    {
        return Some(format!(
            "bpm must be between {MINIMUM_BPM} and {MAXIMUM_BPM}."
        ));
    }
    if unit(entry.bpm_confidence) {
        return Some("bpmConfidence must be between 0 and 1.".into());
    }
    if unit(entry.bpm_stability) {
        return Some("bpmStability must be between 0 and 1.".into());
    }
    None
}

/// Loudness and tempo a client measured itself. A tempo sent without its confidence is
/// taken at its word; stability means nothing without a tempo.
pub(super) async fn post_analysis(
    State(pool): State<SqlitePool>,
    bytes: Bytes,
) -> Result<StatusCode, ApiError> {
    let entries: BTreeMap<String, Option<AnalysisEntry>> = body(
        &bytes,
        "analysis",
        "{ analysis: { id: { loudnessLufs, truePeakDbfs, bpm, bpmConfidence, bpmStability } | null } }",
    )?;
    for (id, entry) in &entries {
        if let Some(reason) = entry.as_ref().and_then(invalid) {
            return Err(bad(format!("{id}: {reason}")));
        }
    }
    let mut tx = pool.begin().await?;
    for (id, entry) in &entries {
        let Ok(id) = id.parse::<i64>() else {
            continue;
        };
        let entry = entry.as_ref();
        let loudness = entry.and_then(|e| e.loudness_lufs.zip(e.true_peak_dbfs));
        let bpm = entry.and_then(|e| e.bpm);
        sqlx::query(
            "INSERT INTO music_audio_analysis (track_id, modified_at, analyzer, source, error,
                 loudness_lufs, true_peak_dbfs, bpm, bpm_confidence, bpm_stability, measured_at)
             SELECT id, modified_at, ?2, 'client', NULL, ?3, ?4, ?5, ?6, ?7, ?8 FROM music_tracks
             WHERE id = ?1 AND missing_since IS NULL
             ON CONFLICT (track_id) DO UPDATE SET modified_at = excluded.modified_at,
                 analyzer = excluded.analyzer, source = 'client', error = NULL,
                 loudness_lufs = excluded.loudness_lufs, true_peak_dbfs = excluded.true_peak_dbfs,
                 bpm = excluded.bpm, bpm_confidence = excluded.bpm_confidence,
                 bpm_stability = excluded.bpm_stability, measured_at = excluded.measured_at",
        )
        .bind(id)
        .bind(ANALYZER)
        .bind(loudness.map(|l| l.0))
        .bind(loudness.map(|l| l.1))
        .bind(bpm)
        .bind(bpm.map(|_| entry.and_then(|e| e.bpm_confidence).unwrap_or(1.0)))
        .bind(bpm.and(entry.and_then(|e| e.bpm_stability)))
        .bind(unix_ms())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
