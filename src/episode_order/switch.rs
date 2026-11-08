//! Switching a series to another order. Episodes are TMDB's own whatever the order, so a switch
//! keeps what's attached to each: files are named as the order they follow and stay as they are
//! (watch progress and watched state are kept per file), and what's kept by season and episode
//! number (monitoring, downloads, searches and history) is renumbered through each episode's
//! TMDB id, in one transaction with the switch.

use std::collections::HashMap;

use super::*;

/// What came of choosing an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chosen {
    /// The series follows it now.
    Done,
    /// The series has no such group, or its episodes haven't been fetched.
    NoSuchGroup,
    /// Melete doesn't choose for a series whose order was chosen already.
    EarlierStands,
}

/// A numbered episode: (season, episode).
type Key = (i64, i64);

/// Makes the TMDB series follow `group` (None: TMDB's own order), as chosen by `by`, and
/// renumbers what's kept by episode number to match; then gives files numbered from the first
/// episode their seasons in the new order.
pub async fn choose(
    pool: &SqlitePool,
    tmdb_id: i64,
    group: Option<&str>,
    by: ChosenBy,
    now: i64,
) -> anyhow::Result<Chosen> {
    let mut tx = pool.begin().await?;
    if let Some(group) = group
        && !usable(&mut tx, tmdb_id, group).await?
    {
        return Ok(Chosen::NoSuchGroup);
    }
    let chosen: Option<Option<String>> =
        sqlx::query_scalar("SELECT group_id FROM series_orders WHERE series_tmdb_id = ?")
            .bind(tmdb_id)
            .fetch_optional(&mut *tx)
            .await?;
    if by == ChosenBy::Auto && chosen.is_some() {
        return Ok(Chosen::EarlierStands);
    }
    let before = order_of(&mut tx, tmdb_id).await?;
    sqlx::query(
        "INSERT INTO series_orders (series_tmdb_id, group_id, chosen_by, chosen_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (series_tmdb_id) DO UPDATE SET group_id = excluded.group_id,
             chosen_by = excluded.chosen_by, chosen_at = excluded.chosen_at
         -- Melete's own choice never replaces one made before.
         WHERE excluded.chosen_by = 'admin'",
    )
    .bind(tmdb_id)
    .bind(group)
    .bind(by.as_str())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let after = order_of(&mut tx, tmdb_id).await?;
    let moved = moves(&before, &after);
    let changed = chosen.as_ref().is_none_or(|was| was.as_deref() != group);
    for (wanted, key) in wanted_series(&mut tx, tmdb_id).await? {
        if !moved.is_empty() {
            renumber_monitoring(&mut tx, wanted, &before, &after).await?;
            renumber_kept(&mut tx, wanted, &key, &moved).await?;
        }
        if changed {
            record(&mut tx, wanted, tmdb_id, group, by, now).await?;
        }
    }
    tx.commit().await?;
    crate::library::absolute::number(pool).await?;
    Ok(Chosen::Done)
}

/// Whether the series has the group, with its episodes fetched.
async fn usable(
    conn: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
    group: &str,
) -> sqlx::Result<bool> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM tmdb_episode_groups g
         WHERE id = ? AND series_tmdb_id = ? AND fetched_at IS NOT NULL
           AND EXISTS (SELECT 1 FROM tmdb_episode_group_episodes e WHERE e.group_id = g.id)",
    )
    .bind(group)
    .bind(tmdb_id)
    .fetch_optional(conn)
    .await?;
    Ok(found.is_some())
}

/// The series' episodes in the order it follows, by TMDB id.
async fn order_of(
    conn: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
) -> sqlx::Result<HashMap<i64, Key>> {
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT tmdb_id, season, episode FROM episodes_in_order WHERE series_tmdb_id = ?",
    )
    .bind(tmdb_id)
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(id, s, e)| (id, (s, e))).collect())
}

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

/// The wanted series that are this TMDB series, with the key their history is kept under.
async fn wanted_series(
    conn: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
) -> sqlx::Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT w.id, CAST(w.tvdb_id AS TEXT) FROM wanted_series w
         WHERE w.tmdb_id = ?1
            OR (w.tmdb_id IS NULL AND w.tvdb_id = (SELECT tvdb_id FROM tmdb_series WHERE tmdb_id = ?1))",
    )
    .bind(tmdb_id)
    .fetch_all(conn)
    .await
}

/// Keeps each episode monitored or not as it was: a season of the new order is monitored when
/// any of its episodes was, and those of them that weren't are left out on their own. Numbers
/// the old order didn't have are left as they are.
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
    for (season, on) in &season_on {
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
    const MOVES: &str = "(SELECT value ->> 0 AS s, value ->> 1 AS e, value ->> 2 AS ns,
                                 value ->> 3 AS ne FROM json_each(?1)) m";
    for (table, whose) in [
        ("downloads", "t.wanted_series_id = ?2"),
        ("searches", "t.wanted_series_id = ?2"),
        (
            "title_events",
            "t.key = ?3 AND t.service IN ('series', 'anime')",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} AS t SET season = m.ns, episode = m.ne FROM {MOVES}
             WHERE {whose} AND t.season = m.s AND t.episode = m.e"
        )))
        .bind(&moves)
        .bind(wanted)
        .bind(key)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Says in the series' history which order it follows now, and who chose it.
async fn record(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    tmdb_id: i64,
    group: Option<&str>,
    by: ChosenBy,
    now: i64,
) -> sqlx::Result<()> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM tmdb_episode_groups WHERE id = ? AND series_tmdb_id = ?",
    )
    .bind(group)
    .bind(tmdb_id)
    .fetch_optional(&mut *conn)
    .await?;
    let order = match &name {
        Some(name) => format!("TMDB's \u{201c}{name}\u{201d} episode group"),
        None => "TMDB's own order".to_owned(),
    };
    let why = match by {
        ChosenBy::Auto => "the files' seasons fit it exactly",
    };
    sqlx::query(
        "INSERT INTO title_events (at, kind, service, key, title, message)
         SELECT ?, 'reordered', t.service, t.key, t.title, ?
         FROM title_event_titles t WHERE t.kind = 'series' AND t.wanted_id = ?",
    )
    .bind(now)
    .bind(format!("episodes follow {order} now: {why}"))
    .bind(wanted)
    .execute(conn)
    .await?;
    Ok(())
}
