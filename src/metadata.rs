//! Matching adopted movies and series to TMDB, and keeping their metadata fresh.
//!
//! Matching is deliberately strict: an ID tag in the folder name wins, otherwise exactly one
//! search result must fit both the title and the year. Anything less is recorded as
//! ambiguous (with the candidates) or unmatched, never guessed. A request that fails leaves
//! the title untouched so the next run tries again.

use anyhow::bail;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::jobs::pace::Pace;
use crate::tmdb::{
    self, ExternalId, MovieDetails, RateLimited, Season, SeriesDetails, Unauthorized,
};

mod choose;
mod credits;
mod imported;
mod manual;
mod matching;
mod nfo;
mod store;
mod trailers;
mod wanted;

pub use choose::*;
pub use credits::refresh_credits;
use imported::*;
pub use manual::*;
use matching::*;
use nfo::*;
use store::*;
pub use trailers::{Trailer, chosen, refresh_trailers};
pub use wanted::*;

const DAY: i64 = 86_400;
/// Unmatched and ambiguous titles are retried after this long, since TMDB keeps growing.
const RETRY_UNMATCHED: i64 = 7 * DAY;
const REFRESH_MOVIES: i64 = 30 * DAY;
/// Series get new episodes, so their cache goes stale sooner.
const REFRESH_SERIES: i64 = 7 * DAY;
/// A wanted series still airing is refreshed daily, for the calendar.
const REFRESH_AIRING: i64 = DAY;
/// Release dates are asked for again after this long.
const REFRESH_RELEASE_DATES: i64 = 7 * DAY;
const MAX_CANDIDATES: usize = 5;

/// What a metadata refresh did, by title.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct RefreshReport {
    /// Titles matched to TMDB this run.
    pub matched: usize,
    /// Titles TMDB has nothing for, left for a retry in a week.
    pub unmatched: usize,
    /// Titles more than one result fits, recorded with the candidates and left for someone to
    /// choose (or a retry in a week).
    pub ambiguous: usize,
    /// Already-matched titles whose cached metadata was fetched again.
    pub refreshed: usize,
    /// Titles skipped because a request failed; they are retried next run.
    pub errors: usize,
}

enum Outcome<D> {
    Matched { details: D, method: &'static str },
    Ambiguous(Vec<Candidate>),
    Unmatched,
}

#[derive(sqlx::FromRow)]
struct TitleRow {
    id: i64,
    parsed_title: String,
    parsed_year: Option<i64>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
    tvdb_id: Option<i64>,
    /// The *arr ids as the refresh read them, to detect an import changing them meanwhile.
    arr_tmdb_id: Option<i64>,
    arr_tvdb_id: Option<i64>,
    /// The ids the title folder's `.nfo` files give, as JSON.
    #[sqlx(default)]
    nfo_ids: Option<String>,
}

/// Whether a title is a movie or a series.
#[derive(Clone, Copy)]
pub enum Kind {
    Movie,
    Series,
}

/// Matches titles not yet matched, then refreshes stale cached metadata.
///
/// Stops at once if TMDB rejects the token; other failures skip one title and carry on.
pub async fn refresh(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<RefreshReport> {
    // One refresh at a time: one started by an import waits for the startup one, then picks up
    // the titles the import reset.
    static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one_at_a_time = RUNNING.lock().await;
    let now = unix_now();
    let mut report = RefreshReport::default();

    // What Melete imported for a wanted title is that title: matched before anything is
    // searched for.
    match_imported(pool, tmdb, now, &mut report).await?;
    match_unmatched_movies(pool, tmdb, now, &mut report).await?;
    match_unmatched_series(pool, tmdb, now, &mut report).await?;
    refresh_stale_movies(pool, tmdb, now, &mut report).await?;
    refresh_stale_series(pool, tmdb, now, &mut report).await?;

    Ok(report)
}

/// Matches the movies not matched yet, and the unmatched ones due for another try.
async fn match_unmatched_movies(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let movies: Vec<TitleRow> = sqlx::query_as(
        "SELECT id, parsed_title, parsed_year, COALESCE(arr_tmdb_id, tmdb_id) AS tmdb_id, imdb_id,
             NULL AS tvdb_id, arr_tmdb_id, NULL AS arr_tvdb_id, nfo_ids FROM movies
         WHERE missing_since IS NULL AND (match_status IS NULL
             OR (match_status <> 'matched' AND match_attempted_at < ?
                 AND match_method IS NOT 'manual'))",
    )
    .bind(now - RETRY_UNMATCHED)
    .fetch_all(pool)
    .await?;
    let pace = Pace::start("library", "matching movies on TMDB", movies.len());
    for row in &movies {
        let item = pace.item();
        match match_movie(tmdb, row).await {
            Ok(Outcome::Matched { details, method }) => {
                store_movie(pool, &details, now).await?;
                if set_auto_match(
                    pool,
                    Kind::Movie,
                    row,
                    Some((details.id, method)),
                    None,
                    now,
                )
                .await?
                {
                    report.matched += 1;
                }
            }
            Ok(outcome) => record_miss(pool, Kind::Movie, row, outcome, now, report).await?,
            Err(err) => {
                item.failed();
                skip(err, "movie", &row.parsed_title, report)?
            }
        }
    }
    pace.finish();
    Ok(())
}

/// Matches the series not matched yet, and the unmatched ones due for another try.
async fn match_unmatched_series(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let series: Vec<TitleRow> = sqlx::query_as(
        "SELECT id, parsed_title, parsed_year, COALESCE(arr_tmdb_id, tmdb_id) AS tmdb_id, imdb_id,
             COALESCE(arr_tvdb_id, tvdb_id) AS tvdb_id, arr_tmdb_id, arr_tvdb_id, nfo_ids
         FROM series
         WHERE missing_since IS NULL AND (match_status IS NULL
             OR (match_status <> 'matched' AND match_attempted_at < ?
                 AND match_method IS NOT 'manual'))",
    )
    .bind(now - RETRY_UNMATCHED)
    .fetch_all(pool)
    .await?;
    let pace = Pace::start("library", "matching series on TMDB", series.len());
    for row in &series {
        let item = pace.item();
        match match_series(tmdb, row).await {
            Ok(Outcome::Matched {
                details: (details, seasons),
                method,
            }) => {
                store_series(pool, &details, &seasons, now).await?;
                if set_auto_match(
                    pool,
                    Kind::Series,
                    row,
                    Some((details.id, method)),
                    None,
                    now,
                )
                .await?
                {
                    report.matched += 1;
                }
            }
            Ok(outcome) => record_miss(pool, Kind::Series, row, outcome, now, report).await?,
            Err(err) => {
                item.failed();
                skip(err, "series", &row.parsed_title, report)?
            }
        }
    }
    pace.finish();
    Ok(())
}

/// Fetches again the matched movies whose cached TMDB entry is stale.
async fn refresh_stale_movies(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let stale_movies: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_movies t JOIN movies m ON m.match_tmdb_id = t.tmdb_id
         WHERE m.missing_since IS NULL
             AND (t.fetched_at < ? OR coalesce(t.language, 'en-US') <> ?)",
    )
    .bind(now - REFRESH_MOVIES)
    // A changed metadata language fetches every title again.
    .bind(tmdb.language())
    .fetch_all(pool)
    .await?;
    let pace = Pace::start("library", "refreshing movies from TMDB", stale_movies.len());
    for id in stale_movies {
        let item = pace.item();
        match tmdb.movie(id).await {
            Ok(Some(details)) => {
                store_movie(pool, &details, now).await?;
                report.refreshed += 1;
            }
            Ok(None) => tracing::warn!(
                tmdb_id = id,
                "TMDB no longer has this movie; keeping the cached copy"
            ),
            Err(err) => {
                item.failed();
                skip(err, "movie refresh", &id.to_string(), report)?
            }
        }
    }
    pace.finish();
    Ok(())
}

/// Fetches again the matched series whose cached TMDB entry is stale.
async fn refresh_stale_series(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let stale_series: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_series t JOIN series s ON s.match_tmdb_id = t.tmdb_id
         WHERE s.missing_since IS NULL
             AND (t.fetched_at < ? OR coalesce(t.language, 'en-US') <> ?)",
    )
    .bind(now - REFRESH_SERIES)
    .bind(tmdb.language())
    .fetch_all(pool)
    .await?;
    let pace = Pace::start("library", "refreshing series from TMDB", stale_series.len());
    for id in stale_series {
        let item = pace.item();
        match fetch_series(tmdb, id).await {
            Ok(Some((details, seasons))) => {
                store_series(pool, &details, &seasons, now).await?;
                report.refreshed += 1;
            }
            Ok(None) => tracing::warn!(
                tmdb_id = id,
                "TMDB no longer has this series; keeping the cached copy"
            ),
            Err(err) => {
                item.failed();
                skip(err, "series refresh", &id.to_string(), report)?
            }
        }
    }
    pace.finish();
    Ok(())
}

#[cfg(test)]
mod tests;
