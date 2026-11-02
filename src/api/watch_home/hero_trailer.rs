//! The trailer each of the hero's picks has, so an app can play it behind the pick without
//! asking again: one in the title's folder, when it's sharp enough to fill a screen. Null when
//! there's none, and an app plays a clip of the title itself instead.

use super::*;

/// The height, in lines, a trailer in the folder needs for the hero.
const SHARP: i64 = 1080;

/// A hero pick's trailer: `{source: "folder", file_id}`, an extra's id, played as a file is;
/// null when it has none sharp enough.
pub(super) async fn hero_trailer(pool: &SqlitePool, title: TitleRef) -> Result<Value, ApiError> {
    let column = if title.series {
        "series_id"
    } else {
        "movie_id"
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
    Ok(match folder {
        Some(id) => json!({ "source": "folder", "file_id": id }),
        None => Value::Null,
    })
}
