//! Switching a series to another order. Episodes are TMDB's own whatever the order, so a switch
//! keeps what's attached to each: files are named as the order they follow and stay as they are
//! (watch progress and watched state are kept per file), and what's kept by season and episode
//! number (monitoring, downloads, searches and history) is renumbered through each episode's
//! TMDB id, in one transaction with the switch. What names an episode the new order leaves out
//! loses its number, which would be another episode's, and keeps the episode's TMDB id instead
//! (`left_out_tmdb_id`), so an order that has it again numbers it again. A switch waits while a
//! download is on its way for such an episode, or for a season the new order makes up of other
//! episodes: it would be imported as something it isn't.

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
    /// A download is on its way for this episode (or, with episode 0, this season) as the series
    /// is numbered now, which the new order leaves out or makes up differently: nothing changed.
    Busy(Key),
}

/// A numbered episode: (season, episode).
pub type Key = (i64, i64);

/// Makes the TMDB series follow `group` (None: TMDB's own order), as chosen by `by`, and
/// renumbers what's kept by episode number to match, files numbered from the first episode
/// included.
pub async fn choose(
    pool: &SqlitePool,
    tmdb_id: i64,
    group: Option<&str>,
    by: ChosenBy,
    now: i64,
) -> anyhow::Result<Chosen> {
    // Writing from the start, so a scan's writes meanwhile wait rather than fail it.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
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
    let wanted = wanted_series(&mut tx, tmdb_id).await?;
    for (id, _) in &wanted {
        if let Some(key) = on_the_way(&mut tx, *id, &before, &after).await? {
            // Dropping the transaction undoes the switch.
            return Ok(Chosen::Busy(key));
        }
    }
    let changed = chosen.as_ref().is_none_or(|was| was.as_deref() != group);
    for (id, key) in &wanted {
        renumber(&mut tx, (*id, key), &before, &after).await?;
        if changed {
            record(&mut tx, *id, tmdb_id, group, by, now).await?;
        }
    }
    // Files numbered from the first episode, counted through the new order with the switch.
    crate::library::absolute::number_in(&mut tx).await?;
    tx.commit().await?;
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

/// The first download on its way for the wanted series that the switch would leave without its
/// episode: one for an episode the new order leaves out, or a season pack for a season the new
/// order makes up of other episodes (episode 0).
async fn on_the_way(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    before: &HashMap<i64, Key>,
    after: &HashMap<i64, Key>,
) -> sqlx::Result<Option<Key>> {
    let coming: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT season, episode FROM downloads
         WHERE wanted_series_id = ? AND season IS NOT NULL
           AND state IN ('downloading', 'completed', 'transferring')
         ORDER BY season, episode",
    )
    .bind(wanted)
    .fetch_all(conn)
    .await?;
    let season_of = |order: &HashMap<i64, Key>, season: i64| -> BTreeSet<i64> {
        order
            .iter()
            .filter(|(_, key)| key.0 == season)
            .map(|(id, _)| *id)
            .collect()
    };
    for (season, episode) in coming {
        let busy = match episode {
            Some(episode) => before
                .iter()
                .any(|(id, key)| *key == (season, episode) && !after.contains_key(id)),
            None => season_of(before, season) != season_of(after, season),
        };
        if busy {
            return Ok(Some((season, episode.unwrap_or(0))));
        }
    }
    Ok(None)
}

/// Renumbers one wanted series' monitoring, downloads, searches and history from the old order
/// to the new.
async fn renumber(
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
        ChosenBy::Admin => "an admin chose it",
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
