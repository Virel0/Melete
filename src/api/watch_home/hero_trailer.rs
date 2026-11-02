//! The trailer each of the hero's picks has, so an app can play it behind the pick without
//! asking again: one in the title's folder first, when it's sharp enough to fill a screen; else,
//! only while trailers online are on, the one TMDB lists, in YouTube's privacy-enhanced player.
//! Null when there's none, and an app plays a clip of the title itself instead.

use super::*;
use crate::metadata::{Trailer, chosen};

/// The height, in lines, a trailer in the folder needs for the hero.
const SHARP: i64 = 1080;

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
    // A trailer shorter than this many lines looks soft behind a pick: 1080, or the title's own
    // height when that's lower (an old show's trailer is as sharp as the show).
    let own: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT max(json_extract(p.info, '$.video[0].height')) FROM media_files f
         JOIN media_probes p ON p.file_id = f.id WHERE f.{column} = ?"
    )))
    .bind(title.id)
    .fetch_one(pool)
    .await?;
    let sharp_enough = own.map_or(SHARP, |h| h.min(SHARP));
    // One not probed yet is taken: it's known soft only once it's measured.
    let folder: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM media_extras
         WHERE {column} = ? AND kind = 'trailer' AND missing_since IS NULL
           AND coalesce(json_extract(probe_info, '$.video[0].height') >= ?, 1)
         ORDER BY title COLLATE NOCASE, id LIMIT 1"
    )))
    .bind(title.id)
    .bind(sharp_enough)
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
