//! TMDB entries for the wanted list's calendar, including wanted titles not on disk yet.

use super::*;

/// TMDB entries for the wanted list's calendar. Monitored wanted titles are fetched
/// even when they aren't on disk (a series known only by its TVDB id is found through TMDB's
/// find), wanted series still airing are refreshed daily, and release dates are asked for
/// movies out within the last year, coming, or undated. Counts what it fetched as refreshed.
pub async fn refresh_wanted(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
) -> anyhow::Result<RefreshReport> {
    let now = unix_now();
    let mut report = RefreshReport::default();

    fetch_wanted_movies(pool, tmdb, now, &mut report).await?;
    refresh_release_dates(pool, tmdb, now, &mut report).await?;
    fetch_wanted_series(pool, tmdb, now, &mut report).await?;
    refresh_airing_series(pool, tmdb, now, &mut report).await?;

    Ok(report)
}

/// Fetches the monitored wanted movies TMDB entries aren't cached for yet.
async fn fetch_wanted_movies(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let movies: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT w.tmdb_id FROM wanted_movies w
         WHERE w.monitored = 1
           AND NOT EXISTS (SELECT 1 FROM tmdb_movies t WHERE t.tmdb_id = w.tmdb_id)",
    )
    .fetch_all(pool)
    .await?;
    for id in movies {
        match tmdb.movie(id).await {
            Ok(Some(details)) => {
                store_movie(pool, &details, now).await?;
                report.refreshed += 1;
            }
            Ok(None) => tracing::warn!(tmdb_id = id, "TMDB has no such wanted movie"),
            Err(err) => skip(err, "wanted movie", &id.to_string(), report)?,
        }
    }
    Ok(())
}

/// Asks for the digital, physical and TV release dates of monitored wanted movies out within the
/// last year, coming or undated, unless they were asked for within the last week.
async fn refresh_release_dates(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let dated: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_movies t JOIN wanted_movies w ON w.tmdb_id = t.tmdb_id
         WHERE w.monitored = 1
           AND (t.release_date IS NULL OR t.release_date >= date(?, 'unixepoch', '-365 days'))
           AND (t.release_dates_at IS NULL OR t.release_dates_at < ?)",
    )
    .bind(now)
    .bind(now - REFRESH_RELEASE_DATES)
    .fetch_all(pool)
    .await?;
    for id in dated {
        match tmdb.release_dates(id).await {
            Ok(dates) => {
                let dates = dates.unwrap_or_default();
                sqlx::query(
                    "UPDATE tmdb_movies SET digital_release = ?, physical_release = ?,
                         tv_release = ?, release_dates_at = ?
                     WHERE tmdb_id = ?",
                )
                .bind(dates.digital)
                .bind(dates.physical)
                .bind(dates.tv)
                .bind(now)
                .bind(id)
                .execute(pool)
                .await?;
                report.refreshed += 1;
            }
            Err(err) => skip(err, "release dates", &id.to_string(), report)?,
        }
    }
    Ok(())
}

/// Fetches the monitored wanted series TMDB entries aren't cached for yet.
async fn fetch_wanted_series(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    // Series by TMDB id, else by TVDB id (Sonarr doesn't always know the TMDB one).
    let unknown: Vec<(Option<i64>, i64)> = sqlx::query_as(
        "SELECT w.tmdb_id, w.tvdb_id FROM wanted_series w
         WHERE w.monitored = 1
           AND NOT EXISTS (SELECT 1 FROM tmdb_series t
                           WHERE t.tmdb_id = w.tmdb_id OR (w.tmdb_id IS NULL AND t.tvdb_id = w.tvdb_id))",
    )
    .fetch_all(pool)
    .await?;
    for (tmdb_id, tvdb_id) in unknown {
        let id = match tmdb_id {
            Some(id) => id,
            None => match tmdb.find(ExternalId::Tvdb(tvdb_id)).await {
                Ok(found) => match found.tv_results.first() {
                    Some(series) => series.id,
                    None => {
                        tracing::warn!(tvdb_id, "TMDB knows no series by this TVDB id");
                        continue;
                    }
                },
                Err(err) => {
                    skip(err, "wanted series", &tvdb_id.to_string(), report)?;
                    continue;
                }
            },
        };
        match fetch_series(tmdb, id).await {
            Ok(Some((details, seasons))) => {
                store_series(pool, &details, &seasons, now).await?;
                report.refreshed += 1;
            }
            Ok(None) => tracing::warn!(tmdb_id = id, "TMDB has no such wanted series"),
            Err(err) => skip(err, "wanted series", &id.to_string(), report)?,
        }
    }
    Ok(())
}

/// Fetches again the monitored wanted series still airing whose cached entry is over a day old.
async fn refresh_airing_series(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    now: i64,
    report: &mut RefreshReport,
) -> anyhow::Result<()> {
    let airing: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_series t
         JOIN wanted_series w ON t.tmdb_id = w.tmdb_id OR (w.tmdb_id IS NULL AND t.tvdb_id = w.tvdb_id)
         WHERE w.monitored = 1 AND coalesce(t.status, '') NOT IN ('Ended', 'Canceled')
           AND t.fetched_at < ?",
    )
    .bind(now - REFRESH_AIRING)
    .fetch_all(pool)
    .await?;
    for id in airing {
        match fetch_series(tmdb, id).await {
            Ok(Some((details, seasons))) => {
                store_series(pool, &details, &seasons, now).await?;
                report.refreshed += 1;
            }
            Ok(None) => tracing::warn!(
                tmdb_id = id,
                "TMDB no longer has this series; keeping the cached copy"
            ),
            Err(err) => skip(err, "series refresh", &id.to_string(), report)?,
        }
    }
    Ok(())
}
