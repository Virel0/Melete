//! Writing a measurement, or why it failed, into every table it feeds.

use super::*;

/// Writes the server's measurement, or its failure, into every table.
pub(super) async fn store(
    pool: &SqlitePool,
    track_id: i64,
    modified_at: i64,
    result: &Result<Measurement, MeasureError>,
) -> sqlx::Result<()> {
    let (measured, error) = match result {
        Ok(measurement) => (measurement, None),
        Err(err) => (&Measurement::default(), Some(err.to_string())),
    };
    let now = unix_ms();
    let mut tx = pool.begin().await?;
    store_bounds(&mut tx, track_id, modified_at, &error, measured, now).await?;
    store_analysis(&mut tx, track_id, modified_at, &error, measured, now).await?;
    store_beat_grid(&mut tx, track_id, modified_at, &error, measured, now).await?;
    store_arrangement(&mut tx, track_id, modified_at, &error, measured, now).await?;
    tx.commit().await
}

/// Writes where the sound starts and ends.
async fn store_bounds(
    tx: &mut sqlx::SqliteConnection,
    track_id: i64,
    modified_at: i64,
    error: &Option<String>,
    measured: &Measurement,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO music_sound_bounds (track_id, modified_at, analyzer, source, error,
             sound_start_ms, sound_end_ms, measured_at)
         VALUES (?, ?, ?, 'server', ?, ?, ?, ?)
         ON CONFLICT (track_id) DO UPDATE SET modified_at = excluded.modified_at,
             analyzer = excluded.analyzer, source = excluded.source, error = excluded.error,
             sound_start_ms = excluded.sound_start_ms, sound_end_ms = excluded.sound_end_ms,
             measured_at = excluded.measured_at",
    )
    .bind(track_id)
    .bind(modified_at)
    .bind(ANALYZER)
    .bind(error)
    .bind(measured.bounds.map(|b| b.start_ms))
    .bind(measured.bounds.map(|b| b.end_ms))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Writes the loudness, true peak and tempo.
async fn store_analysis(
    tx: &mut sqlx::SqliteConnection,
    track_id: i64,
    modified_at: i64,
    error: &Option<String>,
    measured: &Measurement,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO music_audio_analysis (track_id, modified_at, analyzer, source, error,
             loudness_lufs, true_peak_dbfs, bpm, bpm_confidence, bpm_stability, measured_at)
         VALUES (?, ?, ?, 'server', ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (track_id) DO UPDATE SET modified_at = excluded.modified_at,
             analyzer = excluded.analyzer, source = excluded.source, error = excluded.error,
             loudness_lufs = excluded.loudness_lufs, true_peak_dbfs = excluded.true_peak_dbfs,
             bpm = excluded.bpm, bpm_confidence = excluded.bpm_confidence,
             bpm_stability = excluded.bpm_stability, measured_at = excluded.measured_at",
    )
    .bind(track_id)
    .bind(modified_at)
    .bind(ANALYZER)
    .bind(error)
    .bind(measured.loudness.map(|l| l.lufs))
    .bind(measured.loudness.map(|l| l.true_peak_dbfs))
    // Unsure tempi are kept: the threshold is applied when serving.
    .bind(measured.tempo.map(|t| t.bpm))
    .bind(measured.tempo.map(|t| t.confidence))
    .bind(measured.tempo.and_then(|t| t.stability))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Writes the beat grid, mix points and key.
async fn store_beat_grid(
    tx: &mut sqlx::SqliteConnection,
    track_id: i64,
    modified_at: i64,
    error: &Option<String>,
    measured: &Measurement,
    now: i64,
) -> sqlx::Result<()> {
    let grid = measured.grid.as_ref();
    let segments = grid.map(|g| {
        serde_json::Value::from(
            g.segments
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "startMs": s.start_ms, "endMs": s.end_ms, "anchorMs": s.anchor_ms,
                        "bpm": s.bpm, "residualMs": s.residual_ms, "beats": s.beats,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .to_string()
    });
    sqlx::query(
        "INSERT INTO music_beat_grids (track_id, modified_at, analyzer, error, segments,
             beats_per_bar, downbeat_index, mix_in_ms, mix_out_ms, key_camelot, key_confidence,
             measured_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (track_id) DO UPDATE SET modified_at = excluded.modified_at,
             analyzer = excluded.analyzer, error = excluded.error, segments = excluded.segments,
             beats_per_bar = excluded.beats_per_bar, downbeat_index = excluded.downbeat_index,
             mix_in_ms = excluded.mix_in_ms, mix_out_ms = excluded.mix_out_ms,
             key_camelot = excluded.key_camelot, key_confidence = excluded.key_confidence,
             measured_at = excluded.measured_at",
    )
    .bind(track_id)
    .bind(modified_at)
    .bind(ANALYZER)
    .bind(error)
    .bind(segments)
    .bind(grid.and_then(|g| g.beats_per_bar))
    .bind(grid.and_then(|g| g.downbeat_index))
    .bind(grid.and_then(|g| g.mix_in_ms))
    .bind(grid.and_then(|g| g.mix_out_ms))
    .bind(measured.key.as_ref().map(|k| &k.camelot))
    .bind(measured.key.as_ref().map(|k| k.confidence))
    .bind(now)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Writes the sections, phrasing and vocal spans.
async fn store_arrangement(
    tx: &mut sqlx::SqliteConnection,
    track_id: i64,
    modified_at: i64,
    error: &Option<String>,
    measured: &Measurement,
    now: i64,
) -> sqlx::Result<()> {
    let arrangement = measured.arrangement.as_ref();
    let sections = arrangement.map(|a| {
        serde_json::Value::from(
            a.sections
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "startMs": s.start_ms, "endMs": s.end_ms,
                        "kind": s.kind.as_str(), "energy": s.energy,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .to_string()
    });
    // '[]' and NULL say different things: nobody singing, and couldn't tell.
    let vocals = arrangement.and_then(|a| a.vocals.as_ref()).map(|spans| {
        serde_json::Value::from(
            spans
                .iter()
                .map(|v| serde_json::json!({ "startMs": v.start_ms, "endMs": v.end_ms }))
                .collect::<Vec<_>>(),
        )
        .to_string()
    });
    sqlx::query(
        "INSERT INTO music_arrangements (track_id, modified_at, analyzer, error, sections,
             phrase_bars, phrase_anchor_ms, vocals, measured_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (track_id) DO UPDATE SET modified_at = excluded.modified_at,
             analyzer = excluded.analyzer, error = excluded.error, sections = excluded.sections,
             phrase_bars = excluded.phrase_bars, phrase_anchor_ms = excluded.phrase_anchor_ms,
             vocals = excluded.vocals, measured_at = excluded.measured_at",
    )
    .bind(track_id)
    .bind(modified_at)
    .bind(ANALYZER)
    .bind(error)
    .bind(sections)
    .bind(arrangement.and_then(|a| a.phrase_bars))
    .bind(arrangement.and_then(|a| a.phrase_anchor_ms))
    .bind(vocals)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

pub(super) fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}
