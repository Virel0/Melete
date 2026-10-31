//! The trailer each of the hero's picks has, so an app can play it behind the pick without
//! asking again: one in the title's folder first; else, only while trailers online are on, the
//! one TMDB lists, in YouTube's privacy-enhanced player. Null when there's none.

use super::*;
use crate::metadata::{Trailer, chosen};

/// A hero pick's trailer: `{source: "folder", file_id}` (an extra's id, played as a file is)
/// or `{source: "online", embed_url}`, the other one null; null when it has none. `online` is
/// whether trailers online are on.
pub(super) async fn hero_trailer(
    pool: &SqlitePool,
    title: TitleRef,
    online: bool,
) -> Result<Value, ApiError> {
    let (column, details, table) = if title.series {
        ("series_id", "tmdb_series", "series")
    } else {
        ("movie_id", "tmdb_movies", "movies")
    };
    let folder: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM media_extras
         WHERE {column} = ? AND kind = 'trailer' AND missing_since IS NULL
         ORDER BY title COLLATE NOCASE, id LIMIT 1"
    )))
    .bind(title.id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = folder {
        return Ok(json!({ "source": "folder", "file_id": id, "embed_url": null }));
    }
    if !online {
        return Ok(Value::Null);
    }
    let row: Option<(Option<String>, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT t.videos, coalesce(t.language, 'en-US') FROM {table} x
         JOIN {details} t ON t.tmdb_id = x.match_tmdb_id WHERE x.id = ?"
    )))
    .bind(title.id)
    .fetch_optional(pool)
    .await?;
    let Some((videos, language)) = row else {
        return Ok(Value::Null);
    };
    let trailers: Vec<Trailer> = videos
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    Ok(match chosen(&trailers, &language) {
        Some(trailer) => json!({
            "source": "online",
            "file_id": null,
            "embed_url": format!("{}/embed/{}", super::super::trailers::PLAYER, trailer.key),
        }),
        None => Value::Null,
    })
}
