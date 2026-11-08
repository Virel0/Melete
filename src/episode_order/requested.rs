//! Seasons asked for by TMDB's own numbers (a request for one season, or a list's choice), put
//! in the order a series follows, so the episodes asked for are the ones monitored. TMDB's
//! season 2 may be part of a group's season 1, or its seasons 2 and 3.

use super::*;

/// Whether the series follows one of its groups rather than TMDB's own order.
async fn follows_group(conn: &mut sqlx::SqliteConnection, tmdb_id: i64) -> sqlx::Result<bool> {
    let follows: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM series_orders WHERE series_tmdb_id = ? AND group_id IS NOT NULL",
    )
    .bind(tmdb_id)
    .fetch_optional(conn)
    .await?;
    Ok(follows.is_some())
}

/// The wanted series' TMDB id: its own, else the one TMDB's cache has for its TVDB id.
pub async fn tmdb_id_of(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
) -> sqlx::Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT coalesce(w.tmdb_id, (SELECT t.tmdb_id FROM tmdb_series t WHERE t.tvdb_id = w.tvdb_id
                                     ORDER BY t.tmdb_id LIMIT 1))
         FROM wanted_series w WHERE w.id = ?",
    )
    .bind(wanted)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// The seasons of the order the series follows (specials as 0), when it follows a group; None
/// while it follows TMDB's own, whose seasons are TMDB's.
pub async fn followed_seasons(
    conn: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
) -> sqlx::Result<Option<Vec<i64>>> {
    if !follows_group(&mut *conn, tmdb_id).await? {
        return Ok(None);
    }
    let seasons: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT season FROM episodes_in_order WHERE series_tmdb_id = ? ORDER BY season",
    )
    .bind(tmdb_id)
    .fetch_all(conn)
    .await?;
    Ok(Some(seasons))
}

/// Where TMDB's own season `season` is in the order the series follows: each season holding any
/// of its episodes, with those episodes (empty when TMDB's list for it isn't here). None while
/// the series follows TMDB's own order, where it's that season.
pub async fn tmdb_season_in_order(
    conn: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
    season: i64,
) -> sqlx::Result<Option<Seasons>> {
    if !follows_group(&mut *conn, tmdb_id).await? {
        return Ok(None);
    }
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT season, episode FROM episodes_in_order
         WHERE series_tmdb_id = ? AND tmdb_season = ?",
    )
    .bind(tmdb_id)
    .bind(season)
    .fetch_all(conn)
    .await?;
    Ok(Some(collect(rows)))
}

/// Monitors, for a wanted series following a group, the episodes of TMDB's own season `season`,
/// as a choice made in Melete: each season of the order holding any of them is monitored, and
/// in a season that wasn't, the episodes that aren't among them are left out on their own.
/// False, with nothing done, while the series follows TMDB's own order.
pub async fn monitor_tmdb_season(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    tmdb_id: i64,
    season: i64,
) -> sqlx::Result<bool> {
    let Some(asked) = tmdb_season_in_order(&mut *conn, tmdb_id, season).await? else {
        return Ok(false);
    };
    for (season, episodes) in &asked {
        let on: Option<bool> = sqlx::query_scalar(
            "SELECT monitored = 1 FROM wanted_seasons WHERE wanted_series_id = ? AND season = ?",
        )
        .bind(wanted)
        .bind(season)
        .fetch_optional(&mut *conn)
        .await?;
        if on != Some(true) {
            let others: Vec<i64> = sqlx::query_scalar(
                "SELECT episode FROM episodes_in_order WHERE series_tmdb_id = ? AND season = ?",
            )
            .bind(tmdb_id)
            .bind(season)
            .fetch_all(&mut *conn)
            .await?;
            for episode in others.iter().filter(|e| !episodes.contains(e)) {
                sqlx::query(
                    "INSERT INTO wanted_episode_overrides
                         (wanted_series_id, season, episode, monitored, monitored_here)
                     VALUES (?, ?, ?, 0, 1)
                     ON CONFLICT (wanted_series_id, season, episode)
                         DO UPDATE SET monitored = 0, monitored_here = 1",
                )
                .bind(wanted)
                .bind(season)
                .bind(episode)
                .execute(&mut *conn)
                .await?;
            }
        }
        sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored, monitored_here)
             VALUES (?, ?, 1, 1)
             ON CONFLICT (wanted_series_id, season) DO UPDATE SET monitored = 1, monitored_here = 1",
        )
        .bind(wanted)
        .bind(season)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "DELETE FROM wanted_episode_overrides
             WHERE wanted_series_id = ? AND season = ? AND monitored = 0
               AND episode IN (SELECT value FROM json_each(?))",
        )
        .bind(wanted)
        .bind(season)
        .bind(serde_json::to_string(episodes).expect("numbers serialize"))
        .execute(&mut *conn)
        .await?;
    }
    Ok(true)
}
