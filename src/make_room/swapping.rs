//! Swapping a title for a smaller release, started from the Make room page: a movie, or each
//! episode file of a series on its own, searched for a release at most 1080p and clearly smaller
//! than the copy, judged by the title's profile without comparing it with the file (it's meant
//! to be smaller, not better). The best is grabbed, and the usual check and replace puts the old
//! file in the trash once the new one plays as long. The title stops being monitored when the
//! swap starts, so nothing upgrades it back.
//!
//! Slowly: one search at a time, the next no sooner than [`PACE`] after the last, an indexer that
//! says it's at its limit left out for [`LIMIT_REST`]; only while downloads are on.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::downloads::Pipeline;
use crate::search::{self, Refused, Shrink, Target};
use crate::{tmdb, torznab};

/// The least time between two searches.
pub const PACE: Duration = Duration::from_secs(15);
/// How long an indexer at its limit is left out.
const LIMIT_REST: Duration = Duration::from_secs(3600);
/// The most lines a smaller copy may be.
const MAX_HEIGHT: i64 = 1080;
/// The most a smaller copy may take, as a share of the copy it replaces.
const AT_MOST_OF_COPY: f64 = 0.75;
/// The most a smaller copy may take an hour, in bytes: a generous 1080p HEVC release.
const AT_MOST_PER_HOUR: f64 = 4.5e9;
/// A grab whose download fails is tried again this many times.
const TRIES: i64 = 3;

/// Why a swap wasn't started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotStarted {
    NotFound,
    /// Not on the wanted list, so there's no profile to search with.
    NotWanted,
    /// Nothing to swap: no files, or every one is being swapped already.
    NothingToSwap,
}

/// Starts swapping a movie (or with `series`, each of a series' episode files) for a smaller
/// release, and stops the title being monitored. Returns how many files are to be swapped.
pub async fn start(
    pool: &SqlitePool,
    series: bool,
    id: i64,
) -> anyhow::Result<Result<usize, NotStarted>> {
    let (table, wanted, column) = if series {
        ("series", "wanted_series", "series_id")
    } else {
        ("movies", "wanted_movies", "movie_id")
    };
    let present: Option<Option<i64>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT (SELECT w.id FROM {wanted} w WHERE w.{column} = x.id) FROM {table} x
         WHERE x.id = ? AND x.missing_since IS NULL"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(present) = present else {
        return Ok(Err(NotStarted::NotFound));
    };
    let Some(wanted_id) = present else {
        return Ok(Err(NotStarted::NotWanted));
    };
    // A movie's files; a series' episode files, specials and unnumbered ones aside.
    let files: Vec<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.id, f.season, f.episode FROM media_files f
         WHERE f.{column} = ? AND f.missing_since IS NULL
           AND (f.season IS NULL OR (f.season > 0 AND f.episode IS NOT NULL))
           AND NOT EXISTS (SELECT 1 FROM swaps s WHERE s.file_id = f.id
               AND s.state IN ('waiting', 'grabbed'))
         ORDER BY f.season, f.episode, f.id"
    )))
    .bind(id)
    .fetch_all(pool)
    .await?;
    // A movie with several files isn't one a release can replace exactly.
    let files: Vec<_> = if series {
        files
    } else {
        files.into_iter().take(1).collect()
    };
    if files.is_empty() {
        return Ok(Err(NotStarted::NothingToSwap));
    }
    let now = unix_now();
    let mut tx = pool.begin().await?;
    for (file, season, episode) in &files {
        let (movie, show) = if series {
            (None, Some(wanted_id))
        } else {
            (Some(wanted_id), None)
        };
        sqlx::query(
            "INSERT INTO swaps (wanted_movie_id, wanted_series_id, season, episode, file_id, state,
                 started_at)
             VALUES (?, ?, ?, ?, ?, 'waiting', ?)
             ON CONFLICT (file_id) DO UPDATE SET state = 'waiting', reason = NULL,
                 download_id = NULL, attempts = 0, started_at = excluded.started_at,
                 tried_at = NULL",
        )
        .bind(movie)
        .bind(show)
        .bind(season.filter(|_| series))
        .bind(episode.filter(|_| series))
        .bind(file)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {wanted} SET monitored = 0 WHERE id = ?"
    )))
    .bind(wanted_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(files.len()))
}

/// Works through swaps for as long as the server runs, while downloads are on.
pub async fn run(pool: SqlitePool, tmdb: Option<Arc<tmdb::Client>>, pipeline: Arc<Pipeline>) {
    let mut limited: HashMap<i64, Instant> = HashMap::new();
    loop {
        limited.retain(|_, since| since.elapsed() < LIMIT_REST);
        let skip: Vec<i64> = limited.keys().copied().collect();
        match step(&pool, tmdb.as_deref(), &pipeline, &skip).await {
            Ok(found) => {
                for indexer in found {
                    limited.insert(indexer, Instant::now());
                }
            }
            Err(err) => tracing::warn!(error = format!("{err:#}"), "swapping stopped for now"),
        }
        tokio::time::sleep(PACE).await;
    }
}

/// A swap's row, as a step reads it.
#[derive(sqlx::FromRow)]
struct Waiting {
    id: i64,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    season: Option<i64>,
    episode: Option<i64>,
    size: i64,
    seconds: Option<f64>,
}

/// One round: grabbed swaps settled by how their downloads went, then the next waiting one
/// searched and its best release grabbed. Returns the indexers that said they're at their limit.
pub async fn step(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    pipeline: &Pipeline,
    skip: &[i64],
) -> anyhow::Result<Vec<i64>> {
    settle(pool).await?;
    let Some(next) = next_waiting(pool).await? else {
        return Ok(Vec::new());
    };
    let gone: bool = sqlx::query_scalar(
        "SELECT f.missing_since IS NOT NULL FROM swaps s JOIN media_files f ON f.id = s.file_id
         WHERE s.id = ?",
    )
    .bind(next.id)
    .fetch_one(pool)
    .await?;
    if gone {
        finish(pool, next.id, "failed", Some("its file left the library")).await?;
        return Ok(Vec::new());
    }
    if next.wanted_series_id.is_some() && next.season.is_none() {
        let why = "its file has no episode in the order the series follows now";
        finish(pool, next.id, "failed", Some(why)).await?;
        return Ok(Vec::new());
    }
    swap_one(pool, tmdb, pipeline, skip, &next).await
}

/// The next waiting swap. A series' episode is its file's as numbered now, not as when the swap
/// started: a change of the series' episode order may have numbered the file again.
async fn next_waiting(pool: &SqlitePool) -> sqlx::Result<Option<Waiting>> {
    sqlx::query_as(
        "SELECT s.id, s.wanted_movie_id, s.wanted_series_id,
             CASE WHEN s.wanted_series_id IS NOT NULL THEN f.season END AS season,
             CASE WHEN s.wanted_series_id IS NOT NULL THEN f.episode END AS episode, f.size,
             CAST(json_extract(p.info, '$.duration_seconds') AS REAL) AS seconds
         FROM swaps s JOIN media_files f ON f.id = s.file_id
         LEFT JOIN media_probes p ON p.file_id = f.id
         WHERE s.state = 'waiting' ORDER BY s.id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
}

/// Searches for a smaller release of one waiting swap and grabs the best. Returns the indexers
/// that said they're at their limit.
async fn swap_one(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    pipeline: &Pipeline,
    skip: &[i64],
    next: &Waiting,
) -> anyhow::Result<Vec<i64>> {
    let target = match (next.wanted_movie_id, next.wanted_series_id) {
        (Some(id), _) => Target::Movie(id),
        (None, Some(id)) => Target::Series {
            id,
            season: next.season.unwrap_or(0),
            episode: next.episode,
        },
        _ => return Ok(Vec::new()),
    };
    let shrink = Shrink {
        max_height: MAX_HEIGHT,
        max_bytes: limit(next.size, next.seconds),
    };
    let client = torznab::Client::new()?;
    let searched = search::search_shrinking(pool, &client, tmdb, target, skip, shrink).await?;
    sqlx::query("UPDATE swaps SET tried_at = ? WHERE id = ?")
        .bind(unix_now())
        .bind(next.id)
        .execute(pool)
        .await?;
    let search_id = match searched {
        Ok(id) => id,
        // Every indexer is resting: try again later, as it is.
        Err(Refused::NoIndexers) => return Ok(Vec::new()),
        Err(refused) => {
            finish(pool, next.id, "failed", Some(&refused.to_string())).await?;
            return Ok(Vec::new());
        }
    };
    let limited = limited_in(pool, search_id).await?;
    let mut grabbed = Vec::new();
    crate::rss::grab_for(pool, pipeline, target, search_id, &mut grabbed).await?;
    match grabbed.iter().find_map(|g| g.download_id) {
        Some(download) => {
            sqlx::query("UPDATE swaps SET state = 'grabbed', download_id = ? WHERE id = ?")
                .bind(download)
                .bind(next.id)
                .execute(pool)
                .await?;
        }
        None => {
            let why = grabbed
                .iter()
                .rev()
                .find_map(|g| g.refused.clone())
                .unwrap_or_else(|| "no release small enough was found".into());
            finish(pool, next.id, "none_found", Some(&why)).await?;
        }
    }
    Ok(limited)
}

/// The most a smaller copy of a file of `size` bytes and `seconds` long may take.
fn limit(size: i64, seconds: Option<f64>) -> i64 {
    let of_copy = (size as f64 * AT_MOST_OF_COPY) as i64;
    match seconds.filter(|s| *s > 60.0) {
        Some(s) => of_copy.min((s / 3600.0 * AT_MOST_PER_HOUR) as i64),
        None => of_copy,
    }
}

/// The indexers a search found at their limit.
async fn limited_in(pool: &SqlitePool, search_id: i64) -> anyhow::Result<Vec<i64>> {
    let indexers: String = sqlx::query_scalar("SELECT indexers FROM searches WHERE id = ?")
        .bind(search_id)
        .fetch_one(pool)
        .await?;
    let outcomes: Vec<search::IndexerOutcome> = serde_json::from_str(&indexers)?;
    Ok(outcomes
        .into_iter()
        .filter(|o| {
            o.error
                .as_deref()
                .is_some_and(crate::search_missing::says_limit)
        })
        .map(|o| o.indexer_id)
        .collect())
}

/// A grabbed swap and how its download stands.
#[derive(sqlx::FromRow)]
struct Grabbed {
    id: i64,
    /// The download's state.
    state: String,
    retired_at: Option<i64>,
    message: Option<String>,
    /// The swapped file has left the library.
    replaced: bool,
    attempts: i64,
}

/// Grabbed swaps whose downloads have settled: done once the old file left the library for the
/// new one, failed when the copy was kept (the new one played another length) or the download
/// failed too often; a failed download is otherwise searched for again.
async fn settle(pool: &SqlitePool) -> anyhow::Result<()> {
    let grabbed: Vec<Grabbed> = sqlx::query_as(
        "SELECT s.id, d.state, d.retired_at, d.message,
             EXISTS (SELECT 1 FROM media_files f WHERE f.id = s.file_id
                 AND f.missing_since IS NOT NULL) AS replaced,
             s.attempts
         FROM swaps s JOIN downloads d ON d.id = s.download_id WHERE s.state = 'grabbed'",
    )
    .fetch_all(pool)
    .await?;
    for Grabbed {
        id,
        state,
        retired_at: retired,
        message,
        replaced,
        attempts,
    } in grabbed
    {
        if replaced {
            finish(pool, id, "done", None).await?;
        } else if state == "failed" {
            if attempts + 1 >= TRIES {
                finish(pool, id, "failed", Some("its downloads kept failing")).await?;
            } else {
                sqlx::query(
                    "UPDATE swaps SET state = 'waiting', download_id = NULL,
                         attempts = attempts + 1 WHERE id = ?",
                )
                .bind(id)
                .execute(pool)
                .await?;
            }
        } else if retired.is_some() {
            let why = message.unwrap_or_else(|| "the copy was kept".into());
            finish(pool, id, "failed", Some(&why)).await?;
        }
    }
    Ok(())
}

async fn finish(pool: &SqlitePool, id: i64, state: &str, why: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("UPDATE swaps SET state = ?, download_id = NULL, reason = ? WHERE id = ?")
        .bind(state)
        .bind(why.map(|w| w.chars().take(300).collect::<String>()))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// How swapping one title is going, for the Make room page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, sqlx::FromRow)]
pub struct Progress {
    /// `movie` or `series`.
    pub kind: String,
    /// The library movie's or series' id.
    pub id: i64,
    pub title: String,
    /// In a library with the anime category, or wanted as anime.
    pub anime: bool,
    pub waiting: i64,
    pub grabbed: i64,
    pub done: i64,
    pub none_found: i64,
    pub failed: i64,
}

/// Every title with swaps, the most recently started first.
pub async fn progress(pool: &SqlitePool) -> sqlx::Result<Vec<Progress>> {
    sqlx::query_as(
        "SELECT CASE WHEN s.wanted_movie_id IS NULL THEN 'series' ELSE 'movie' END AS kind,
             coalesce(f.movie_id, f.series_id) AS id,
             coalesce(wm.title, ws.title) AS title,
             (l.category IS 'anime' OR ws.series_type IS 'anime') AS anime,
             sum(s.state = 'waiting') AS waiting, sum(s.state = 'grabbed') AS grabbed,
             sum(s.state = 'done') AS done, sum(s.state = 'none_found') AS none_found,
             sum(s.state = 'failed') AS failed
         FROM swaps s JOIN media_files f ON f.id = s.file_id
         JOIN libraries l ON l.id = f.library_id
         LEFT JOIN wanted_movies wm ON wm.id = s.wanted_movie_id
         LEFT JOIN wanted_series ws ON ws.id = s.wanted_series_id
         GROUP BY kind, coalesce(f.movie_id, f.series_id)
         ORDER BY max(s.started_at) DESC",
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests;
