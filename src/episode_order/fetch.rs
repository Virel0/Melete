//! Fetching a series' episode groups from TMDB, only for series that need them, and again
//! whenever the series' own metadata is fetched again.

use super::*;
use crate::tmdb::{self, RateLimited, Unauthorized};

/// Groups asked for at most, per series.
const MAX_GROUPS: usize = 20;

/// Fetches the episode groups of each matched series that needs them (its files use seasons
/// TMDB's own order doesn't have, or it follows a group) and hasn't had them since its metadata
/// was last fetched. Stops when TMDB refuses the token or asks for a long pause; another failure
/// skips the series until the next run. Returns how many series' groups it fetched.
pub async fn refresh(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<usize> {
    let due: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_series t JOIN series s ON s.match_tmdb_id = t.tmdb_id
         WHERE s.missing_since IS NULL
           AND (t.episode_groups_fetched_at IS NULL OR t.episode_groups_fetched_at < t.fetched_at)",
    )
    .fetch_all(pool)
    .await?;
    let mut fetched = 0;
    for tmdb_id in due {
        if !needed(pool, tmdb_id).await? {
            continue;
        }
        match fetch(pool, tmdb, tmdb_id).await {
            Ok(_) => fetched += 1,
            Err(err) if err.is::<Unauthorized>() || err.is::<RateLimited>() => return Err(err),
            Err(err) => tracing::warn!(
                tmdb_id,
                error = format!("{err:#}"),
                "couldn't fetch a series' episode groups; will retry next run"
            ),
        }
    }
    Ok(fetched)
}

/// Whether a series' groups are worth asking for: it follows one, or its files use seasons TMDB's
/// own order doesn't have.
async fn needed(pool: &SqlitePool, tmdb_id: i64) -> sqlx::Result<bool> {
    let follows: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM series_orders WHERE series_tmdb_id = ? AND group_id IS NOT NULL",
    )
    .bind(tmdb_id)
    .fetch_optional(pool)
    .await?;
    if follows.is_some() {
        return Ok(true);
    }
    let mut conn = pool.acquire().await?;
    let files = file_episodes(&mut conn, tmdb_id).await?;
    let own = tmdb_seasons(&mut conn, tmdb_id).await?;
    Ok(needs_another(&files, &own))
}

/// Fetches a series' episode groups and each one's episodes, and keeps them, replacing what was
/// kept. A group TMDB no longer lists is forgotten unless the series follows it, which keeps its
/// last episodes. Only the series' TMDB id and the groups' ids leave the server. False when TMDB
/// has no such series.
pub async fn fetch(pool: &SqlitePool, tmdb: &tmdb::Client, tmdb_id: i64) -> anyhow::Result<bool> {
    let Some(mut listed) = tmdb.episode_groups(tmdb_id).await? else {
        mark_fetched(pool, tmdb_id).await?;
        return Ok(false);
    };
    listed.truncate(MAX_GROUPS);
    let mut groups = Vec::new();
    for group in &listed {
        if let Some(details) = tmdb.episode_group(&group.id).await? {
            groups.push(details);
        }
    }
    let mut tx = pool.begin().await?;
    store(&mut tx, tmdb_id, &groups).await?;
    tx.commit().await?;
    Ok(true)
}

/// Keeps a series' groups as fetched, in place of those kept before.
async fn store(
    tx: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
    groups: &[tmdb::EpisodeGroup],
) -> anyhow::Result<()> {
    let now = crate::db::unix_now();
    let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
    sqlx::query(
        "DELETE FROM tmdb_episode_groups
         WHERE series_tmdb_id = ?1 AND id NOT IN (SELECT value FROM json_each(?2))
           AND id NOT IN (SELECT group_id FROM series_orders WHERE group_id IS NOT NULL)",
    )
    .bind(tmdb_id)
    .bind(serde_json::to_string(&ids)?)
    .execute(&mut *tx)
    .await?;
    for group in groups {
        let mine: Option<i64> = sqlx::query_scalar(
            "INSERT INTO tmdb_episode_groups (id, series_tmdb_id, name, kind, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, kind = excluded.kind,
                 fetched_at = excluded.fetched_at
             WHERE tmdb_episode_groups.series_tmdb_id = excluded.series_tmdb_id
             RETURNING 1",
        )
        .bind(&group.id)
        .bind(tmdb_id)
        .bind(group.name.trim())
        .bind(group.kind)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        // Another series' group by that id: TMDB's answer is wrong, and the group is left out.
        if mine.is_none() {
            continue;
        }
        sqlx::query("DELETE FROM tmdb_episode_group_episodes WHERE group_id = ?")
            .bind(&group.id)
            .execute(&mut *tx)
            .await?;
        for (season, episodes) in &group.seasons {
            for (i, episode_tmdb_id) in episodes.iter().enumerate() {
                sqlx::query(
                    "INSERT INTO tmdb_episode_group_episodes
                         (group_id, season, episode, episode_tmdb_id)
                     VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
                )
                .bind(&group.id)
                .bind(season)
                .bind(i as i64 + 1)
                .bind(episode_tmdb_id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    mark_fetched_in(tx, tmdb_id, now).await?;
    Ok(())
}

async fn mark_fetched(pool: &SqlitePool, tmdb_id: i64) -> sqlx::Result<()> {
    let mut conn = pool.acquire().await?;
    mark_fetched_in(&mut conn, tmdb_id, crate::db::unix_now()).await
}

async fn mark_fetched_in(
    conn: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE tmdb_series SET episode_groups_fetched_at = ? WHERE tmdb_id = ?")
        .bind(now)
        .bind(tmdb_id)
        .execute(conn)
        .await?;
    Ok(())
}
