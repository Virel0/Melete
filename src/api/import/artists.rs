//! Every wanted artist, with how many of their albums are wanted, monitored and on disk.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct WantedArtistRow {
    pub(super) id: i64,
    pub(super) mb_artist_id: String,
    pub(super) name: String,
    pub(super) monitored: bool,
    pub(super) monitor_new: String,
    pub(super) quality_profile: Option<String>,
    pub(super) profile_id: Option<i64>,
    pub(super) library: Option<String>,
    pub(super) source: String,
    pub(super) albums: i64,
    pub(super) monitored_albums: i64,
    pub(super) on_disk: i64,
}

/// A wanted album is on disk when a present album in a music library is its release group,
/// by its tags or by matching.
pub(super) const ON_DISK: &str = "SELECT m.id FROM music_albums m WHERE m.missing_since IS NULL
     AND w.mb_release_group_id IN (m.mb_release_group_id, m.match_release_group_id)
     ORDER BY m.id LIMIT 1";

/// Every wanted artist, with how many of their albums are wanted, monitored and on disk.
pub(super) async fn wanted_artists(
    State(state): State<ImportState>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<WantedArtistRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT a.id, a.mb_artist_id, a.name, a.monitored, a.monitor_new, a.quality_profile,
             a.profile_id, l.name AS library, a.source,
             count(w.id) AS albums, coalesce(sum(w.monitored), 0) AS monitored_albums,
             coalesce(sum(({ON_DISK}) IS NOT NULL), 0) AS on_disk
         FROM wanted_artists a
         LEFT JOIN libraries l ON l.id = a.library_id
         LEFT JOIN wanted_albums w ON w.wanted_artist_id = a.id
         GROUP BY a.id ORDER BY a.name COLLATE NOCASE, a.id"
    )))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                json!({
                    "id": r.id, "mb_artist_id": r.mb_artist_id, "name": r.name,
                    "monitored": r.monitored, "monitor_new": r.monitor_new,
                    "quality_profile": r.quality_profile, "profile_id": r.profile_id,
                    "library": r.library, "source": r.source,
                    "albums": r.albums, "monitored_albums": r.monitored_albums, "on_disk": r.on_disk,
                })
            })
            .collect(),
    ))
}
