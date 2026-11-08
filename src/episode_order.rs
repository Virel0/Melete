//! Which order a series' episodes follow: TMDB's own, or one of its episode groups (the seasons
//! other sites use, say, when TMDB lists a show as one long season). The order is kept per TMDB
//! series (`series_orders`), and everything that numbers episodes asks the database's
//! `episodes_in_order`, so browsing, the apps, Missing and Monitored, searches, RSS sync,
//! absolute numbers, imports and history count the same way.
//!
//! A group is fetched only when it's needed: the files use seasons TMDB's own order doesn't
//! have, or the series follows a group. Melete switches a series to a group by itself only when
//! exactly one group has exactly the seasons and episodes its files have; otherwise the closest
//! is suggested. An admin's choice is never changed automatically.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use sqlx::SqlitePool;

mod fetch;
mod fit;
mod requested;
mod switch;

pub use fetch::refresh;
pub use fit::{decide, suggestion};
pub use requested::{followed_seasons, monitor_tmdb_season, tmdb_id_of, tmdb_season_in_order};
pub use switch::{Chosen, choose};

/// Who chose a series' order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChosenBy {
    /// Melete, because the files fit a group exactly.
    Auto,
}

impl ChosenBy {
    fn as_str(self) -> &'static str {
        match self {
            ChosenBy::Auto => "auto",
        }
    }
}

/// The order a series follows.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Order {
    /// The TMDB episode group it follows; null for TMDB's own order.
    pub group_id: Option<String>,
    /// The group's name, as TMDB has it.
    pub name: Option<String>,
    /// `auto` when Melete chose it, `admin` when an admin did; null when nobody chose.
    pub chosen_by: Option<String>,
    /// When, in Unix seconds.
    pub chosen_at: Option<i64>,
}

/// The order the TMDB series follows.
pub async fn current(pool: &SqlitePool, tmdb_id: i64) -> sqlx::Result<Order> {
    let chosen: Option<Order> = sqlx::query_as(
        "SELECT o.group_id, g.name, o.chosen_by, o.chosen_at FROM series_orders o
         LEFT JOIN tmdb_episode_groups g ON g.id = o.group_id
         WHERE o.series_tmdb_id = ?",
    )
    .bind(tmdb_id)
    .fetch_optional(pool)
    .await?;
    Ok(chosen.unwrap_or(Order {
        group_id: None,
        name: None,
        chosen_by: None,
        chosen_at: None,
    }))
}

/// Episodes by season, season 0 left out.
pub(crate) type Seasons = BTreeMap<i64, BTreeSet<i64>>;

/// The episodes the series' files are named as, by season: present files of the folders
/// matched to it, specials and numbers worked out from absolute ones left out.
async fn file_episodes(conn: &mut sqlx::SqliteConnection, tmdb_id: i64) -> sqlx::Result<Seasons> {
    let rows: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT f.season, f.episode, f.episode_end FROM media_files f
         JOIN series s ON s.id = f.series_id
         WHERE s.match_tmdb_id = ? AND s.missing_since IS NULL AND f.missing_since IS NULL
           AND f.season > 0 AND f.mapped = 0",
    )
    .bind(tmdb_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut seasons = Seasons::new();
    for (season, first, last) in rows {
        let last = last.unwrap_or(first).min(first + 99);
        seasons.entry(season).or_default().extend(first..=last);
    }
    Ok(seasons)
}

/// TMDB's own seasons and episodes for the series, specials left out.
async fn tmdb_seasons(conn: &mut sqlx::SqliteConnection, tmdb_id: i64) -> sqlx::Result<Seasons> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT season, episode FROM tmdb_episodes WHERE series_tmdb_id = ? AND season > 0",
    )
    .bind(tmdb_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(collect(rows))
}

fn collect(rows: impl IntoIterator<Item = (i64, i64)>) -> Seasons {
    let mut seasons = Seasons::new();
    for (season, episode) in rows {
        seasons.entry(season).or_default().insert(episode);
    }
    seasons
}

/// Whether the files use a season TMDB's own order doesn't have, which is when another order is
/// worth looking for.
fn needs_another(files: &Seasons, tmdb: &Seasons) -> bool {
    !tmdb.is_empty() && files.keys().any(|s| !tmdb.contains_key(s))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod testing {
    use sqlx::SqlitePool;

    /// "Some Anime" (TMDB 100, TVDB 200) as TMDB lists it, one season of 59 episodes (TMDB ids
    /// 1001 to 1059), following its "Seasons" group of 24, 23 and 12.
    pub(crate) async fn some_anime_in_seasons(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, fetched_at) VALUES (100, 200, 'Some Anime', 0);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id)
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 59)
                 SELECT 100, 1, i, 1000 + i FROM n;
             INSERT INTO tmdb_episode_groups (id, series_tmdb_id, name, kind, fetched_at)
                 VALUES ('aaaa', 100, 'Seasons', 6, 0);
             INSERT INTO tmdb_episode_group_episodes (group_id, season, episode, episode_tmdb_id)
                 SELECT 'aaaa', CASE WHEN episode <= 24 THEN 1 WHEN episode <= 47 THEN 2 ELSE 3 END,
                        CASE WHEN episode <= 24 THEN episode WHEN episode <= 47 THEN episode - 24
                             ELSE episode - 47 END, tmdb_id
                 FROM tmdb_episodes WHERE series_tmdb_id = 100;
             INSERT INTO series_orders (series_tmdb_id, group_id, chosen_by, chosen_at)
                 VALUES (100, 'aaaa', 'auto', 0);",
        )
        .execute(pool)
        .await
        .unwrap();
    }
}
