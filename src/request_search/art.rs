//! A music result's picture: an album's cover in the library. Nothing is fetched for an album
//! that isn't here (there's no path to the Cover Art Archive for one), so a music search sends
//! out only its words. A movie's or series' poster is the API's to give, as discovery gives
//! posters (`tmdb_poster`).

use super::*;

/// The address of its cover on this server, for an album in the library.
pub(super) async fn here(pool: &SqlitePool, c: &Candidate) -> sqlx::Result<Option<String>> {
    if c.kind != "album" || !c.in_library {
        return Ok(None);
    }
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM music_albums WHERE missing_since IS NULL
           AND coalesce(match_release_group_id, mb_release_group_id) = ?
         ORDER BY id LIMIT 1",
    )
    .bind(&c.mb_release_group_id)
    .fetch_optional(pool)
    .await?;
    Ok(id.map(|id| format!("/api/v1/music/albums/{id}/cover")))
}
