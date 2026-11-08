//! Switching a series to another order. Episodes are TMDB's own whatever the order, so a switch
//! keeps what's attached to each: files are named as the order they follow and stay as they are
//! (watch progress and watched state are kept per file), and what's kept by season and episode
//! number (monitoring, downloads, searches and history) is renumbered through each episode's
//! TMDB id, in one transaction with the switch. What names an episode the new order leaves out
//! loses its number, which would be another episode's, and keeps the episode's TMDB id instead
//! (`left_out_tmdb_id`), so an order that has it again numbers it again. A switch waits while a
//! download is on its way for such an episode, or for a season the new order makes up of other
//! episodes: it would be imported as something it isn't. A switch holds [`SWITCHING`] for
//! writing throughout, so no grab is between checking its result and recording its download.
//!
//! TMDB editing the group a series follows changes its order too: that goes the same way
//! ([`regroup`]).

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
    let _switching = SWITCHING.write().await;
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
    let wanted = match renumber_all(&mut tx, tmdb_id, &before).await? {
        Ok(wanted) => wanted,
        // Dropping the transaction undoes the switch.
        Err(key) => return Ok(Chosen::Busy(key)),
    };
    if chosen.as_ref().is_none_or(|was| was.as_deref() != group) {
        for (id, _) in &wanted {
            record(&mut tx, *id, tmdb_id, group, by, now).await?;
        }
    }
    tx.commit().await?;
    Ok(Chosen::Done)
}

/// Puts in place the episodes TMDB now lists for the group the series follows, and renumbers
/// for them as a switch does. None when it's done (or the series doesn't follow that group);
/// the download it waits for when one is on its way that it would leave without its episode,
/// with the group's episodes as they were.
pub async fn regroup(
    pool: &SqlitePool,
    tmdb_id: i64,
    group: &crate::tmdb::EpisodeGroup,
    now: i64,
) -> anyhow::Result<Option<Key>> {
    let _switching = SWITCHING.write().await;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let follows: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM series_orders WHERE series_tmdb_id = ? AND group_id = ?")
            .bind(tmdb_id)
            .bind(&group.id)
            .fetch_optional(&mut *tx)
            .await?;
    if follows.is_none() {
        return Ok(None);
    }
    let before = order_of(&mut tx, tmdb_id).await?;
    super::fetch::put_episodes(&mut tx, group).await?;
    let wanted = match renumber_all(&mut tx, tmdb_id, &before).await? {
        Ok(wanted) => wanted,
        Err(key) => return Ok(Some(key)),
    };
    // Results searched for before were judged in the old order.
    sqlx::query("UPDATE series_orders SET chosen_at = ? WHERE series_tmdb_id = ?")
        .bind(now)
        .bind(tmdb_id)
        .execute(&mut *tx)
        .await?;
    let said = format!(
        "TMDB changed its \u{201c}{}\u{201d} episode group, which the episodes follow: they're \
         numbered as it has them now",
        group.name.trim()
    );
    for (id, _) in &wanted {
        say(&mut tx, *id, &said, now).await?;
    }
    tx.commit().await?;
    Ok(None)
}

/// After the order changed in `tx`: renumbers what each wanted series keeps, and counts files
/// numbered from the first episode again. The wanted series and their history's keys; the
/// download that has to come in first when one is on its way that the change would leave
/// without its episode (and nothing is renumbered).
async fn renumber_all(
    tx: &mut sqlx::SqliteConnection,
    tmdb_id: i64,
    before: &HashMap<i64, Key>,
) -> anyhow::Result<Result<Vec<(i64, String)>, Key>> {
    let after = order_of(&mut *tx, tmdb_id).await?;
    let wanted = wanted_series(&mut *tx, tmdb_id).await?;
    for (id, _) in &wanted {
        if let Some(key) = on_the_way(&mut *tx, *id, before, &after).await? {
            return Ok(Err(key));
        }
    }
    for (id, key) in &wanted {
        super::renumber::renumber(&mut *tx, (*id, key), before, &after).await?;
    }
    // Files numbered from the first episode, counted through the new order with the switch.
    crate::library::absolute::number_in(tx).await?;
    Ok(Ok(wanted))
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
    say(
        conn,
        wanted,
        &format!("episodes follow {order} now: {why}"),
        now,
    )
    .await
}

/// Writes `message` in the wanted series' history as an episode order change.
async fn say(
    conn: &mut sqlx::SqliteConnection,
    wanted: i64,
    message: &str,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO title_events (at, kind, service, key, title, message)
         SELECT ?, 'reordered', t.service, t.key, t.title, ?
         FROM title_event_titles t WHERE t.kind = 'series' AND t.wanted_id = ?",
    )
    .bind(now)
    .bind(message)
    .bind(wanted)
    .execute(conn)
    .await?;
    Ok(())
}
