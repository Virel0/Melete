//! Followed artists and how looking up their new albums last went, and one wanted artist's albums.

use super::*;

/// A followed artist and how looking up their new albums last went.
#[derive(Serialize, sqlx::FromRow)]
pub(super) struct Follow {
    pub(super) wanted_artist_id: i64,
    pub(super) name: String,
    /// `all` or `new`, as `monitor_new` says.
    pub(super) monitor_new: String,
    /// The day of the first look (YYYY-MM-DD); null until one is made.
    pub(super) since: Option<String>,
    /// When the last look was made, unix seconds; null until one is.
    pub(super) checked_at: Option<i64>,
    /// Why the last look failed, when it did.
    pub(super) error: Option<String>,
}

/// Every followed artist, with when their new albums were last looked for.
pub(super) async fn follows(
    State(state): State<ImportState>,
) -> Result<Json<Vec<Follow>>, ApiError> {
    let rows = sqlx::query_as(
        "SELECT a.id AS wanted_artist_id, a.name, a.monitor_new, c.since, c.checked_at, c.error
         FROM wanted_artists a
         LEFT JOIN followed_artist_checks c ON c.wanted_artist_id = a.id
         WHERE a.monitored = 1 AND a.monitor_new IN ('all', 'new')
         ORDER BY c.error IS NULL, a.name COLLATE NOCASE, a.id",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(sqlx::FromRow)]
pub(super) struct WantedAlbumRow {
    pub(super) id: i64,
    pub(super) mb_release_group_id: String,
    pub(super) title: String,
    pub(super) album_type: Option<String>,
    pub(super) secondary_types: String,
    pub(super) release_date: Option<String>,
    pub(super) monitored: bool,
    pub(super) source: String,
    pub(super) album_id: Option<i64>,
}

/// One wanted artist's albums, newest first, each with the library album it already is.
pub(super) async fn wanted_albums(
    State(state): State<ImportState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wanted_artists WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such wanted artist",
        ));
    }
    let rows: Vec<WantedAlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT w.id, w.mb_release_group_id, w.title, w.album_type, w.secondary_types,
             w.release_date, w.monitored, w.source, ({ON_DISK}) AS album_id
         FROM wanted_albums w WHERE w.wanted_artist_id = ?
         ORDER BY w.release_date IS NULL, w.release_date DESC, w.title COLLATE NOCASE, w.id"
    )))
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                json!({
                    "id": r.id, "mb_release_group_id": r.mb_release_group_id, "title": r.title,
                    "album_type": r.album_type,
                    "secondary_types": serde_json::from_str::<Value>(&r.secondary_types).unwrap_or_else(|_| json!([])),
                    "release_date": r.release_date, "monitored": r.monitored, "source": r.source,
                    "album_id": r.album_id,
                })
            })
            .collect(),
    ))
}
