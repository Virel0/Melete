//! What the server measured, served by id: sound bounds, loudness and tempo, beat grids with their
//! keys, and arrangements.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct BoundsRow {
    pub(super) id: i64,
    pub(super) current: bool,
    pub(super) error: Option<String>,
    pub(super) sound_start_ms: Option<i64>,
    pub(super) sound_end_ms: Option<i64>,
}

pub(super) async fn get_bounds(
    State(pool): State<SqlitePool>,
    Query(query): Query<Ids>,
) -> Result<Json<Value>, ApiError> {
    let ids = requested(query.ids.as_deref())?;
    let rows: Vec<BoundsRow> = sqlx::query_as(
        "SELECT t.id, coalesce(b.modified_at = t.modified_at AND b.analyzer >= ?2, 0) AS current,
             b.error, b.sound_start_ms, b.sound_end_ms
         FROM music_tracks t LEFT JOIN music_sound_bounds b ON b.track_id = t.id
         WHERE t.missing_since IS NULL AND t.id IN (SELECT value FROM json_each(?1))
         ORDER BY t.id",
    )
    .bind(json_ids(&ids))
    .bind(ANALYZER)
    .fetch_all(&pool)
    .await?;
    let rows = rows
        .into_iter()
        .map(|row| {
            (
                row.id,
                row.current,
                row.error,
                (row.sound_start_ms, row.sound_end_ms),
            )
        })
        .collect();
    Ok(Json(answer(
        rows,
        |bounds| match bounds {
            (Some(start), Some(end)) => json!({ "soundStartMs": start, "soundEndMs": end }),
            // Measured, and nothing worth trimming.
            _ => Value::Null,
        },
        "bounds",
    )))
}

#[derive(sqlx::FromRow)]
pub(super) struct AnalysisRow {
    pub(super) id: i64,
    pub(super) current: bool,
    pub(super) error: Option<String>,
    pub(super) loudness_lufs: Option<f64>,
    pub(super) true_peak_dbfs: Option<f64>,
    pub(super) bpm: Option<f64>,
    pub(super) bpm_confidence: Option<f64>,
    pub(super) bpm_stability: Option<f64>,
}

/// A row as sent: an unsure tempo is withheld with its stability (a made-up number would be
/// ordered against as if it were true), and a row with nothing worth reporting is null.
pub(super) fn project(row: &AnalysisRow) -> Value {
    let tempo = row
        .bpm
        .zip(row.bpm_confidence)
        .filter(|&(_, confidence)| confidence >= tempo::MINIMUM_CONFIDENCE);
    if row.loudness_lufs.is_none() && tempo.is_none() {
        return Value::Null;
    }
    json!({
        "loudnessLufs": row.loudness_lufs,
        "truePeakDbfs": row.true_peak_dbfs,
        "bpm": tempo.map(|t| t.0),
        "bpmConfidence": tempo.map(|t| t.1),
        "bpmStability": tempo.and(row.bpm_stability),
    })
}

pub(super) async fn get_analysis(
    State(pool): State<SqlitePool>,
    Query(query): Query<Ids>,
) -> Result<Json<Value>, ApiError> {
    let ids = requested(query.ids.as_deref())?;
    let rows: Vec<AnalysisRow> = sqlx::query_as(
        "SELECT t.id, coalesce(a.modified_at = t.modified_at AND a.analyzer >= ?2, 0) AS current,
             a.error, a.loudness_lufs, a.true_peak_dbfs, a.bpm, a.bpm_confidence, a.bpm_stability
         FROM music_tracks t LEFT JOIN music_audio_analysis a ON a.track_id = t.id
         WHERE t.missing_since IS NULL AND t.id IN (SELECT value FROM json_each(?1))
         ORDER BY t.id",
    )
    .bind(json_ids(&ids))
    .bind(ANALYZER)
    .fetch_all(&pool)
    .await?;
    let rows = rows
        .into_iter()
        .map(|row| (row.id, row.current, row.error.clone(), row))
        .collect();
    Ok(Json(answer(rows, |row| project(&row), "analysis")))
}

#[derive(sqlx::FromRow)]
pub(super) struct GridRow {
    pub(super) id: i64,
    pub(super) current: bool,
    pub(super) error: Option<String>,
    pub(super) segments: Option<String>,
    pub(super) beats_per_bar: Option<i64>,
    pub(super) downbeat_index: Option<i64>,
    pub(super) mix_in_ms: Option<f64>,
    pub(super) mix_out_ms: Option<f64>,
    pub(super) key_camelot: Option<String>,
    pub(super) key_confidence: Option<f64>,
}

/// A grid as sent, or null when there's none; the key rides with the grid, as in the sidecar.
pub(super) fn project_grid(row: &GridRow) -> Value {
    let segments = row
        .segments
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|s| s.as_array().is_some_and(|a| !a.is_empty()));
    let Some(segments) = segments else {
        return Value::Null;
    };
    json!({
        "segments": segments,
        "beatsPerBar": row.beats_per_bar,
        "downbeatIndex": row.downbeat_index,
        "mixInMs": row.mix_in_ms,
        "mixOutMs": row.mix_out_ms,
        "key": row.key_camelot,
        "keyConfidence": row.key_confidence,
    })
}

pub(super) async fn get_grids(
    State(pool): State<SqlitePool>,
    Query(query): Query<Ids>,
) -> Result<Json<Value>, ApiError> {
    let ids = requested(query.ids.as_deref())?;
    let rows: Vec<GridRow> = sqlx::query_as(
        "SELECT t.id, coalesce(g.modified_at = t.modified_at AND g.analyzer >= ?2, 0) AS current,
             g.error, g.segments, g.beats_per_bar, g.downbeat_index, g.mix_in_ms, g.mix_out_ms,
             g.key_camelot, g.key_confidence
         FROM music_tracks t LEFT JOIN music_beat_grids g ON g.track_id = t.id
         WHERE t.missing_since IS NULL AND t.id IN (SELECT value FROM json_each(?1))
         ORDER BY t.id",
    )
    .bind(json_ids(&ids))
    .bind(ANALYZER)
    .fetch_all(&pool)
    .await?;
    let rows = rows
        .into_iter()
        .map(|row| (row.id, row.current, row.error.clone(), row))
        .collect();
    Ok(Json(answer(rows, |row| project_grid(&row), "grids")))
}

#[derive(sqlx::FromRow)]
pub(super) struct ArrangementRow {
    pub(super) id: i64,
    pub(super) current: bool,
    pub(super) error: Option<String>,
    pub(super) sections: Option<String>,
    pub(super) phrase_bars: Option<i64>,
    pub(super) phrase_anchor_ms: Option<f64>,
    pub(super) vocals: Option<String>,
}

/// An arrangement as sent, or null when there's no structure worth describing. `vocals` is
/// `[]` when nobody was heard singing and null when it couldn't be told.
pub(super) fn project_arrangement(row: &ArrangementRow) -> Value {
    let parse = |text: &Option<String>| {
        text.as_deref()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
    };
    let Some(sections) =
        parse(&row.sections).filter(|s| s.as_array().is_some_and(|a| !a.is_empty()))
    else {
        return Value::Null;
    };
    json!({
        "sections": sections,
        "phraseBars": row.phrase_bars,
        "phraseAnchorMs": row.phrase_anchor_ms,
        "vocals": parse(&row.vocals),
    })
}

pub(super) async fn get_arrangements(
    State(pool): State<SqlitePool>,
    Query(query): Query<Ids>,
) -> Result<Json<Value>, ApiError> {
    let ids = requested(query.ids.as_deref())?;
    let rows: Vec<ArrangementRow> = sqlx::query_as(
        "SELECT t.id, coalesce(r.modified_at = t.modified_at AND r.analyzer >= ?2, 0) AS current,
             r.error, r.sections, r.phrase_bars, r.phrase_anchor_ms, r.vocals
         FROM music_tracks t LEFT JOIN music_arrangements r ON r.track_id = t.id
         WHERE t.missing_since IS NULL AND t.id IN (SELECT value FROM json_each(?1))
         ORDER BY t.id",
    )
    .bind(json_ids(&ids))
    .bind(ANALYZER)
    .fetch_all(&pool)
    .await?;
    let rows = rows
        .into_iter()
        .map(|row| (row.id, row.current, row.error.clone(), row))
        .collect();
    Ok(Json(answer(
        rows,
        |row| project_arrangement(&row),
        "arrangements",
    )))
}
