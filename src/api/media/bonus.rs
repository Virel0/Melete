//! A title's extras: trailers, featurettes and other bonus videos in its folder, each played
//! like a file by its id.

use super::*;

/// A present movie's extras.
pub(super) async fn movie_extras(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Value>>, ApiError> {
    extras_of(&state.pool, &current.access, &MOVIES, id).await
}

/// A present series' extras.
pub(super) async fn series_extras(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Value>>, ApiError> {
    extras_of(&state.pool, &current.access, &SERIES, id).await
}

/// A title's present extras, trailers first: each one's id, kind, title and length once
/// probed. 404 when there's no such present title, or the user may not see it.
async fn extras_of(
    pool: &SqlitePool,
    access: &crate::access::Access,
    kind: &Kind,
    id: i64,
) -> Result<Json<Vec<Value>>, ApiError> {
    let Kind {
        titles,
        file_column,
        ..
    } = kind;
    let present: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT x.id FROM {titles} x WHERE x.id = ? AND x.missing_since IS NULL AND {}",
        if *titles == "series" {
            access.series_sql("x")
        } else {
            access.movie_sql("x")
        }
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if present.is_none() {
        return Err(not_found(if *titles == "series" {
            "series"
        } else {
            "movie"
        }));
    }
    let rows: Vec<(i64, String, String, Option<f64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, kind, title,
                 CASE WHEN probed_size = size AND probed_modified_at = modified_at
                      THEN json_extract(probe_info, '$.duration_seconds') END
             FROM media_extras WHERE {file_column} = ? AND missing_since IS NULL
             ORDER BY CASE kind WHEN 'trailer' THEN 0 WHEN 'featurette' THEN 1
                 WHEN 'behind_the_scenes' THEN 2 WHEN 'deleted_scene' THEN 3
                 WHEN 'interview' THEN 4 ELSE 5 END,
                 title COLLATE NOCASE, id"
        )))
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, kind, title, duration)| {
                json!({ "id": id, "kind": kind, "title": title, "duration_seconds": duration })
            })
            .collect(),
    ))
}
