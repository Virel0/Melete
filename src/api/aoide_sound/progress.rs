//! How much of the library has been measured: the coverage counts and their summary.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct Coverage {
    pub(super) tracks: i64,
    pub(super) bounds: i64,
    pub(super) analysis: i64,
    pub(super) grids: i64,
    pub(super) gridded: i64,
    pub(super) arrangements: i64,
    pub(super) described: i64,
    pub(super) outstanding: i64,
}

/// How much of the library has been measured. A track counts as measured, in the summary and
/// against `measuring`, only once all four of its measurements are current: right after a new
/// kind of measurement arrives, every track waits for it.
pub(super) async fn coverage(State(pool): State<SqlitePool>) -> Result<Json<Value>, ApiError> {
    let Coverage {
        tracks,
        bounds,
        analysis,
        grids,
        gridded,
        arrangements,
        described,
        outstanding,
    } = sqlx::query_as(
        "SELECT count(*) AS tracks, coalesce(sum(b), 0) AS bounds, coalesce(sum(a), 0) AS analysis,
             coalesce(sum(g), 0) AS grids, coalesce(sum(gridded), 0) AS gridded,
             coalesce(sum(r), 0) AS arrangements, coalesce(sum(described), 0) AS described,
             coalesce(sum(NOT (b AND a AND g AND r)), 0) AS outstanding
         FROM (SELECT
                 EXISTS (SELECT 1 FROM music_sound_bounds x WHERE x.track_id = t.id
                         AND x.modified_at = t.modified_at AND x.analyzer >= ?1) AS b,
                 EXISTS (SELECT 1 FROM music_audio_analysis x WHERE x.track_id = t.id
                         AND x.modified_at = t.modified_at AND x.analyzer >= ?1) AS a,
                 EXISTS (SELECT 1 FROM music_beat_grids x WHERE x.track_id = t.id
                         AND x.modified_at = t.modified_at AND x.analyzer >= ?1) AS g,
                 EXISTS (SELECT 1 FROM music_beat_grids x WHERE x.track_id = t.id
                         AND x.modified_at = t.modified_at AND x.analyzer >= ?1
                         AND x.segments IS NOT NULL) AS gridded,
                 EXISTS (SELECT 1 FROM music_arrangements x WHERE x.track_id = t.id
                         AND x.modified_at = t.modified_at AND x.analyzer >= ?1) AS r,
                 EXISTS (SELECT 1 FROM music_arrangements x WHERE x.track_id = t.id
                         AND x.modified_at = t.modified_at AND x.analyzer >= ?1
                         AND x.sections IS NOT NULL) AS described
               FROM music_tracks t WHERE t.missing_since IS NULL)",
    )
    .bind(ANALYZER)
    .fetch_one(&pool)
    .await?;
    let measured = tracks - outstanding;
    let summary = if tracks == 0 {
        "No audio in the library yet.".to_owned()
    } else {
        let progress = format!(
            "{measured} of {tracks} tracks measured ({:.0}%)",
            measured as f64 * 100.0 / tracks as f64
        );
        if outstanding > 0 {
            format!(
                "{progress}. {outstanding} waiting; tracks are measured in the background after each scan, one at a time."
            )
        } else {
            format!("{progress}. Nothing outstanding.")
        }
    };
    Ok(Json(json!({
        "tracks": tracks,
        "soundBounds": bounds,
        "audioAnalysis": analysis,
        "beatGrids": grids,
        "gridded": gridded,
        "arrangements": arrangements,
        "described": described,
        "measuring": outstanding,
        "sweepEnabled": true,
        "summary": summary,
    })))
}
