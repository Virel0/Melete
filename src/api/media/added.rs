//! What a list item says for a "new" badge, beyond the title itself: when the title came in,
//! and for a series how many of its episodes the caller hasn't watched.

use std::collections::HashMap;

use super::*;

/// Fills `added_at` on each item (by its `id`) and `unwatched_episodes` (series, for `me`;
/// null for movies).
pub(in crate::api) async fn add_list_fields(
    pool: &SqlitePool,
    me: i64,
    series: bool,
    items: &mut [Value],
) -> sqlx::Result<()> {
    let ids: Vec<i64> = items.iter().filter_map(|i| i["id"].as_i64()).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let added = added_at(pool, series, &ids).await?;
    let unwatched = if series {
        unwatched_episodes(pool, me, &ids).await?
    } else {
        HashMap::new()
    };
    for item in items {
        let Some(id) = item["id"].as_i64() else {
            continue;
        };
        item["added_at"] = json!(added.get(&id));
        item["unwatched_episodes"] = if series {
            json!(unwatched.get(&id).copied().unwrap_or(0))
        } else {
            Value::Null
        };
    }
    Ok(())
}

/// When each movie (or series) among `ids` came in: the first of its files still here.
async fn added_at(pool: &SqlitePool, series: bool, ids: &[i64]) -> sqlx::Result<HashMap<i64, i64>> {
    let column = if series { "series_id" } else { "movie_id" };
    let rows: Vec<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {column}, min(coalesce(added_at, modified_at)) FROM media_files
         WHERE missing_since IS NULL AND {column} IN (SELECT value FROM json_each(?))
         GROUP BY {column}"
    )))
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// How many episodes of each series among `ids` are on disk and not watched by `me`: each
/// episode once however many files have it, an episode watched in any of them counting as
/// watched, specials aside.
pub(in crate::api) async fn unwatched_episodes(
    pool: &SqlitePool,
    me: i64,
    ids: &[i64],
) -> sqlx::Result<HashMap<i64, i64>> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT series_id, sum(last - first + 1) FROM (
             SELECT f.series_id, f.season, f.episode AS first,
                 min(coalesce(f.episode_end, f.episode), f.episode + 99) AS last,
                 max(coalesce(p.watched, 0)) AS watched
             FROM media_files f
             LEFT JOIN playback_progress p ON p.file_id = f.id AND p.user_id = ?1
             WHERE f.missing_since IS NULL AND f.season > 0 AND f.episode IS NOT NULL
               AND f.series_id IN (SELECT value FROM json_each(?2))
             GROUP BY f.series_id, f.season, f.episode, last
         ) WHERE watched = 0 GROUP BY series_id",
    )
    .bind(me)
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}
