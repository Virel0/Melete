//! The libraries with what each holds, and the titles TMDB matching couldn't settle.

use super::*;

#[derive(Debug, PartialEq, Serialize, sqlx::FromRow)]
pub(super) struct LibrarySummary {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) kind: String,
    /// `anime`, `kids` or none.
    pub(super) category: Option<String>,
    pub(super) path: String,
    pub(super) last_scan_at: Option<i64>,
    /// Movies, series or albums currently present.
    pub(super) titles: i64,
    pub(super) files: i64,
    pub(super) missing_files: i64,
    /// Episode files whose name gave no season and episode; music tracks without readable tags.
    pub(super) unparsed_files: i64,
}

pub(super) async fn list_libraries(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<LibrarySummary>>, ApiError> {
    let libraries = sqlx::query_as(
        "SELECT l.id, l.name, l.kind, l.category, l.path, l.last_scan_at,
             (SELECT count(*) FROM movies m WHERE m.library_id = l.id AND m.missing_since IS NULL)
             + (SELECT count(*) FROM series s WHERE s.library_id = l.id AND s.missing_since IS NULL)
             + (SELECT count(*) FROM music_albums a
                 WHERE a.library_id = l.id AND a.missing_since IS NULL) AS titles,
             (SELECT count(*) FROM media_files f
                 WHERE f.library_id = l.id AND f.missing_since IS NULL)
             + (SELECT count(*) FROM music_tracks t
                 WHERE t.library_id = l.id AND t.missing_since IS NULL) AS files,
             (SELECT count(*) FROM media_files f
                 WHERE f.library_id = l.id AND f.missing_since IS NOT NULL)
             + (SELECT count(*) FROM music_tracks t
                 WHERE t.library_id = l.id AND t.missing_since IS NOT NULL) AS missing_files,
             (SELECT count(*) FROM media_files f
                 WHERE f.library_id = l.id AND f.missing_since IS NULL
                   AND f.series_id IS NOT NULL AND f.season IS NULL)
             + (SELECT count(*) FROM music_tracks t
                 WHERE t.library_id = l.id AND t.missing_since IS NULL AND t.tagged = 0)
                 AS unparsed_files
         FROM libraries l ORDER BY l.name",
    )
    .fetch_all(&pool)
    .await?;
    Ok(Json(libraries))
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct UnmatchedRow {
    pub(super) kind: String,
    pub(super) id: i64,
    pub(super) library: String,
    pub(super) rel_path: String,
    pub(super) parsed_title: String,
    pub(super) parsed_year: Option<i64>,
    pub(super) status: String,
    pub(super) candidates: Option<String>,
}

/// Titles TMDB matching could not settle, with the candidates for ambiguous ones. Titles the
/// admin already decided by hand are not listed.
pub(super) async fn list_unmatched(
    State(pool): State<SqlitePool>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<UnmatchedRow> = sqlx::query_as(
        "SELECT 'movie' AS kind, m.id, l.name AS library, m.rel_path, m.parsed_title,
             m.parsed_year, m.match_status AS status, m.match_candidates AS candidates
         FROM movies m JOIN libraries l ON l.id = m.library_id
         WHERE m.missing_since IS NULL AND m.match_status IN ('unmatched', 'ambiguous')
           AND m.match_method IS NOT 'manual'
         UNION ALL
         SELECT 'series', s.id, l.name, s.rel_path, s.parsed_title, s.parsed_year,
             s.match_status, s.match_candidates
         FROM series s JOIN libraries l ON l.id = s.library_id
         WHERE s.missing_since IS NULL AND s.match_status IN ('unmatched', 'ambiguous')
           AND s.match_method IS NOT 'manual'
         ORDER BY library, rel_path",
    )
    .fetch_all(&pool)
    .await?;
    let mut unmatched = Vec::with_capacity(rows.len());
    for row in rows {
        let candidates: Value = match &row.candidates {
            Some(json) => serde_json::from_str(json)?,
            None => json!([]),
        };
        unmatched.push(json!({
            "kind": row.kind,
            "id": row.id,
            "library": row.library,
            "rel_path": row.rel_path,
            "parsed_title": row.parsed_title,
            "parsed_year": row.parsed_year,
            "status": row.status,
            "candidates": candidates,
        }));
    }
    Ok(Json(unmatched))
}
