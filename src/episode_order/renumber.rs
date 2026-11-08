//! Renumbering what a series keeps by season and episode number when the order it follows
//! changes, through each episode's TMDB id: monitoring, downloads, searches and history; what
//! names an episode the new order leaves out loses its number and keeps the episode's TMDB id.

use std::collections::HashMap;

use super::switch::Key;
use super::*;

/// Each episode in both orders that's numbered differently: where it was, where it is.
fn moves(before: &HashMap<i64, Key>, after: &HashMap<i64, Key>) -> Vec<(Key, Key)> {
    let mut moved: Vec<(Key, Key)> = before
        .iter()
        .filter_map(|(id, was)| {
            after
                .get(id)
                .filter(|now| *now != was)
                .map(|now| (*was, *now))
        })
        .collect();
    moved.sort();
    moved
}

/// Each episode the old order has and the new one leaves out: where it was, and its TMDB id.
fn left_out(before: &HashMap<i64, Key>, after: &HashMap<i64, Key>) -> Vec<(Key, i64)> {
    let mut gone: Vec<(Key, i64)> = before
        .iter()
        .filter(|(id, _)| !after.contains_key(id))
        .map(|(id, key)| (*key, *id))
        .collect();
    gone.sort();
    gone
}

/// Renumbers one wanted series' monitoring, downloads, searches and history from the old order
/// to the new.
pub(super) async fn renumber(
    conn: &mut sqlx::SqliteConnection,
    (wanted, key): (i64, &str),
    before: &HashMap<i64, Key>,
    after: &HashMap<i64, Key>,
) -> sqlx::Result<()> {
    let moved = moves(before, after);
    let gone = left_out(before, after);
    if !moved.is_empty() || !gone.is_empty() {
        renumber_monitoring(conn, wanted, before, after).await?;
        // Left out first: renumbering may give their numbers to other episodes.
        detach_left_out(conn, wanted, key, &gone).await?;
        renumber_kept(conn, wanted, key, &moved).await?;
    }
    number_again(conn, wanted, key, after).await
}

/// Keeps each episode monitored or not as it was: a season of the new order is monitored when
/// any of its episodes was, and those of them that weren't are left out on their own. Numbers
/// the old order didn't have are left as they are; seasons the new order doesn't have are no
/// longer monitored, as they hold no episode.
async fn renumber_monitoring(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    before: &HashMap<i64, Key>,
    after: &HashMap<i64, Key>,
) -> sqlx::Result<()> {
    let seasons: HashMap<i64, bool> = sqlx::query_as(
        "SELECT season, monitored = 1 FROM wanted_seasons WHERE wanted_series_id = ?",
    )
    .bind(wanted)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let overrides: HashMap<Key, bool> = sqlx::query_as(
        "SELECT season, episode, monitored = 1 FROM wanted_episode_overrides
         WHERE wanted_series_id = ?",
    )
    .bind(wanted)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(s, e, m)| ((s, e), m))
    .collect();
    let was_monitored = |key: &Key| {
        seasons.get(&key.0) == Some(&true) && overrides.get(key).copied().unwrap_or(true)
    };
    // Each episode in both orders: where it is now, and whether it was monitored.
    let episodes: Vec<(Key, Key, bool)> = before
        .iter()
        .filter_map(|(id, was)| after.get(id).map(|now| (*was, *now, was_monitored(was))))
        .collect();
    let mut season_on: BTreeMap<i64, bool> = BTreeMap::new();
    for (_, now, on) in &episodes {
        *season_on.entry(now.0).or_default() |= on;
    }
    set_seasons(conn, wanted, &season_on, after).await?;
    for (was, now, _) in &episodes {
        for key in [was, now] {
            sqlx::query(
                "DELETE FROM wanted_episode_overrides
                 WHERE wanted_series_id = ? AND season = ? AND episode = ?",
            )
            .bind(wanted)
            .bind(key.0)
            .bind(key.1)
            .execute(&mut *conn)
            .await?;
        }
    }
    for (_, now, on) in &episodes {
        if season_on[&now.0] && !on {
            sqlx::query(
                "INSERT INTO wanted_episode_overrides
                     (wanted_series_id, season, episode, monitored, monitored_here)
                 VALUES (?, ?, ?, 0, 1)",
            )
            .bind(wanted)
            .bind(now.0)
            .bind(now.1)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Monitors each season of the new order as worked out, and stops monitoring seasons it doesn't
/// have (a choice made here, as an import from Sonarr counts it).
async fn set_seasons(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    season_on: &BTreeMap<i64, bool>,
    after: &HashMap<i64, Key>,
) -> sqlx::Result<()> {
    for (season, on) in season_on {
        sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored, monitored_here)
             VALUES (?, ?, ?, 1)
             ON CONFLICT (wanted_series_id, season) DO UPDATE SET monitored = excluded.monitored,
                 monitored_here = CASE WHEN wanted_seasons.monitored = excluded.monitored
                                       THEN wanted_seasons.monitored_here ELSE 1 END",
        )
        .bind(wanted)
        .bind(season)
        .bind(on)
        .execute(&mut *conn)
        .await?;
    }
    if after.is_empty() {
        return Ok(());
    }
    let have: BTreeSet<i64> = after.values().map(|key| key.0).collect();
    sqlx::query(
        "UPDATE wanted_seasons SET monitored = 0, monitored_here = 1
         WHERE wanted_series_id = ? AND monitored = 1
           AND season NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(wanted)
    .bind(serde_json::to_string(&have).expect("numbers serialize"))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// What's kept by episode number for a wanted series (`?2`, its history's key `?3`).
const KEPT: [(&str, &str); 3] = [
    ("downloads", "t.wanted_series_id = ?2"),
    ("searches", "t.wanted_series_id = ?2"),
    (
        "title_events",
        "t.key = ?3 AND t.service IN ('series', 'anime')",
    ),
];

/// Runs `update` (with `FROM {pairs} m`, `{whose}` and the table as `t`) on each of [`KEPT`],
/// binding `pairs` as JSON, the wanted series and its history's key.
async fn each_kept(
    conn: &mut sqlx::SqliteConnection,
    update: &str,
    pairs: &str,
    (wanted, key): (i64, &str),
) -> sqlx::Result<()> {
    for (table, whose) in KEPT {
        let sql = update.replace("{table}", table).replace("{whose}", whose);
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(pairs)
            .bind(wanted)
            .bind(key)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Renumbers the series' downloads, searches and history that name an episode, each row once.
/// Those naming only a season (packs, season searches) are left as they are.
async fn renumber_kept(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    key: &str,
    moved: &[(Key, Key)],
) -> sqlx::Result<()> {
    let moves = serde_json::to_string(
        &moved
            .iter()
            .map(|(was, now)| [was.0, was.1, now.0, now.1])
            .collect::<Vec<_>>(),
    )
    .expect("numbers serialize");
    each_kept(
        conn,
        "UPDATE {table} AS t SET season = m.ns, episode = m.ne
         FROM (SELECT value ->> 0 AS s, value ->> 1 AS e, value ->> 2 AS ns, value ->> 3 AS ne
               FROM json_each(?1)) m
         WHERE {whose} AND t.season = m.s AND t.episode = m.e",
        &moves,
        (wanted, key),
    )
    .await
}

/// Downloads, searches and history naming an episode the new order leaves out: their number
/// would be another episode's, so it goes, and the episode's TMDB id stays. Monitoring it set
/// goes too.
async fn detach_left_out(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    key: &str,
    gone: &[(Key, i64)],
) -> sqlx::Result<()> {
    if gone.is_empty() {
        return Ok(());
    }
    let pairs = serde_json::to_string(
        &gone
            .iter()
            .map(|((s, e), id)| [*s, *e, *id])
            .collect::<Vec<_>>(),
    )
    .expect("numbers serialize");
    each_kept(
        conn,
        "UPDATE {table} AS t SET season = NULL, episode = NULL, left_out_tmdb_id = m.id
         FROM (SELECT value ->> 0 AS s, value ->> 1 AS e, value ->> 2 AS id
               FROM json_each(?1)) m
         WHERE {whose} AND t.season = m.s AND t.episode = m.e",
        &pairs,
        (wanted, key),
    )
    .await?;
    sqlx::query(
        "DELETE FROM wanted_episode_overrides
         WHERE wanted_series_id = ?1 AND EXISTS (
             SELECT 1 FROM json_each(?2) m
             WHERE m.value ->> 0 = season AND m.value ->> 1 = episode)",
    )
    .bind(wanted)
    .bind(&pairs)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Numbers again what named an episode an earlier order left out, when this one has it.
async fn number_again(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    key: &str,
    after: &HashMap<i64, Key>,
) -> sqlx::Result<()> {
    let back = serde_json::to_string(
        &after
            .iter()
            .map(|(id, (s, e))| [*id, *s, *e])
            .collect::<Vec<_>>(),
    )
    .expect("numbers serialize");
    each_kept(
        conn,
        "UPDATE {table} AS t SET season = m.s, episode = m.e, left_out_tmdb_id = NULL
         FROM (SELECT value ->> 0 AS id, value ->> 1 AS s, value ->> 2 AS e
               FROM json_each(?1)) m
         WHERE {whose} AND t.left_out_tmdb_id = m.id",
        &back,
        (wanted, key),
    )
    .await
}
