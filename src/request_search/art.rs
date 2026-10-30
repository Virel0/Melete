//! A search result's picture, only from what's here: a movie's or series' TMDB poster Melete
//! keeps for a title it knows, or an album's cover in the library. Nothing is fetched for a
//! result that isn't here, so a search sends out only its words.

use super::*;

/// The address of its picture on this server, when there's one here.
pub(super) async fn here(pool: &SqlitePool, c: &Candidate) -> sqlx::Result<Option<String>> {
    Ok(match c.kind.as_str() {
        "movie" | "series" => {
            let sql = if c.kind == "movie" {
                "SELECT poster_path FROM tmdb_movies WHERE tmdb_id = ?"
            } else {
                "SELECT poster_path FROM tmdb_series WHERE tmdb_id = ?"
            };
            let path: Option<Option<String>> = sqlx::query_scalar(sql)
                .bind(c.tmdb_id)
                .fetch_optional(pool)
                .await?;
            crate::manage::poster(path.flatten())
        }
        "album" if c.in_library => {
            let id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM music_albums WHERE missing_since IS NULL
                   AND coalesce(match_release_group_id, mb_release_group_id) = ?
                 ORDER BY id LIMIT 1",
            )
            .bind(&c.mb_release_group_id)
            .fetch_optional(pool)
            .await?;
            id.map(|id| format!("/api/v1/music/albums/{id}/cover"))
        }
        _ => None,
    })
}
