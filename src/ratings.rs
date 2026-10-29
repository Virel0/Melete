//! Content ratings for the library's movies and series: each title's certifications by country
//! from TMDB, and the age they come to on one scale, for users with a highest rating they may
//! see. Fetched for matched titles in the library, once and then monthly. A title is judged by
//! one country's rating, chosen in Settings, else the United States'; with neither it's unrated.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::tmdb::{self, RateLimited, Unauthorized};

mod scale;

pub use scale::{COUNTRIES, age};

/// A title's ratings are fetched again after this long.
const REFRESH: i64 = 30 * 24 * 60 * 60;

/// The setting naming the country whose ratings count, by its ISO 3166 code.
pub const COUNTRY_SETTING: &str = "MELETE_RATINGS_COUNTRY";

/// The country whose rating counts when the chosen one has none, and the default.
const FALLBACK: &str = "US";

/// The country `value` names, when it's one whose ratings Melete reads.
pub fn known_country(value: &str) -> Option<&'static str> {
    COUNTRIES.iter().map(|c| c.value).find(|c| *c == value)
}

/// The country whose ratings count, as set now: the United States when it's unset, or (which
/// saving refuses) not one Melete knows.
pub async fn country(pool: &SqlitePool) -> sqlx::Result<&'static str> {
    let set = crate::settings::value(pool, COUNTRY_SETTING).await?;
    Ok(match set.as_deref().map(str::trim) {
        None | Some("") => FALLBACK,
        Some(value) => known_country(value).unwrap_or_else(|| {
            tracing::warn!(
                value,
                "unknown country for age ratings; using the United States'"
            );
            FALLBACK
        }),
    })
}

/// The age a title's certifications come to in `country`: its rating there, else the United
/// States' when that one has none Melete can read; None when neither can be read.
pub fn age_of(certifications: &BTreeMap<String, String>, country: &str) -> Option<u8> {
    [country, FALLBACK].into_iter().find_map(|country| {
        certifications
            .get(country)
            .and_then(|cert| age(country, cert))
    })
}

/// What a refresh did.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Titles whose ratings were fetched.
    pub fetched: usize,
    /// Titles whose request failed, to be tried next time.
    pub errors: usize,
}

/// Keeps a title's certifications and the age they come to in the chosen country.
pub async fn store(
    pool: &SqlitePool,
    tv: bool,
    tmdb_id: i64,
    certifications: &BTreeMap<String, String>,
) -> sqlx::Result<()> {
    let country = country(pool).await?;
    sqlx::query(
        "INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (tv, tmdb_id) DO UPDATE SET certifications = excluded.certifications,
             age = excluded.age, fetched_at = excluded.fetched_at",
    )
    .bind(tv)
    .bind(tmdb_id)
    .bind(serde_json::to_string(certifications).unwrap_or_else(|_| "{}".into()))
    .bind(age_of(certifications, country).map(i64::from))
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Works out every stored title's age again, in one go, so a change to the scale or to the
/// chosen country counts at once, with nothing fetched again.
pub async fn reread(pool: &SqlitePool) -> sqlx::Result<()> {
    let country = country(pool).await?;
    let mut tx = pool.begin().await?;
    let rows: Vec<(bool, i64, String, Option<i64>)> =
        sqlx::query_as("SELECT tv, tmdb_id, certifications, age FROM tmdb_ratings")
            .fetch_all(&mut *tx)
            .await?;
    for (tv, tmdb_id, certifications, stored) in rows {
        let certifications: BTreeMap<String, String> =
            serde_json::from_str(&certifications).unwrap_or_default();
        let age = age_of(&certifications, country).map(i64::from);
        if age != stored {
            sqlx::query("UPDATE tmdb_ratings SET age = ? WHERE tv = ? AND tmdb_id = ?")
                .bind(age)
                .bind(tv)
                .bind(tmdb_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await
}

/// Fetches the ratings of titles about to be shown to a user with a highest rating (TMDB's
/// search and discovery results, outside the library, say) that Melete has none of, or only a
/// month-old one, and keeps them, so a title is asked for once a month at most. Paced as every
/// TMDB request is; only each title's TMDB id leaves the server. A title whose request fails
/// stays unrated, and so hidden; TMDB rejecting the token or asking for a long pause leaves the
/// rest unasked. How many were fetched.
pub async fn fetch_unknown(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    titles: &[(bool, i64)],
) -> sqlx::Result<usize> {
    let mut fetched = 0;
    let mut asked = std::collections::HashSet::new();
    for &(tv, id) in titles {
        let known: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tmdb_ratings
                            WHERE tv = ? AND tmdb_id = ? AND fetched_at >= ?)",
        )
        .bind(tv)
        .bind(id)
        .bind(unix_now() - REFRESH)
        .fetch_one(pool)
        .await?;
        if known || !asked.insert((tv, id)) {
            continue;
        }
        match tmdb.certifications(tv, id).await {
            Ok(found) => {
                store(pool, tv, id, &found.unwrap_or_default()).await?;
                fetched += 1;
            }
            Err(err) => {
                tracing::warn!(
                    tmdb_id = id,
                    error = format!("{err:#}"),
                    "TMDB ratings request failed; the title stays hidden from rating-limited users"
                );
                if err.is::<Unauthorized>() || err.is::<RateLimited>() {
                    break;
                }
            }
        }
    }
    Ok(fetched)
}

/// Forgets the month-old ratings of titles the library doesn't have, fetched for search and
/// discovery results; they're fetched again if shown again.
async fn forget_outside(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM tmdb_ratings WHERE fetched_at < ?
             AND NOT (tv = 0 AND EXISTS (SELECT 1 FROM movies m
                 WHERE m.match_tmdb_id = tmdb_ratings.tmdb_id))
             AND NOT (tv = 1 AND EXISTS (SELECT 1 FROM series s
                 WHERE s.match_tmdb_id = tmdb_ratings.tmdb_id))",
    )
    .bind(unix_now() - REFRESH)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetches the ratings of the library's matched titles that have none yet or whose are a month
/// old. Stops at once if TMDB rejects the token or asks for a long pause; other failures skip
/// one title.
pub async fn refresh(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<Report> {
    reread(pool).await?;
    forget_outside(pool).await?;
    let due: Vec<(bool, i64)> = sqlx::query_as(
        "SELECT DISTINCT 0, m.match_tmdb_id FROM movies m
             LEFT JOIN tmdb_ratings r ON r.tv = 0 AND r.tmdb_id = m.match_tmdb_id
         WHERE m.missing_since IS NULL AND m.match_tmdb_id IS NOT NULL
             AND (r.fetched_at IS NULL OR r.fetched_at < ?1)
         UNION
         SELECT DISTINCT 1, s.match_tmdb_id FROM series s
             LEFT JOIN tmdb_ratings r ON r.tv = 1 AND r.tmdb_id = s.match_tmdb_id
         WHERE s.missing_since IS NULL AND s.match_tmdb_id IS NOT NULL
             AND (r.fetched_at IS NULL OR r.fetched_at < ?1)",
    )
    .bind(unix_now() - REFRESH)
    .fetch_all(pool)
    .await?;
    let mut report = Report::default();
    for (tv, id) in due {
        match tmdb.certifications(tv, id).await {
            Ok(Some(certifications)) => {
                store(pool, tv, id, &certifications).await?;
                report.fetched += 1;
            }
            // TMDB has no such title any more: nothing to read, so it stays unrated.
            Ok(None) => {
                store(pool, tv, id, &BTreeMap::new()).await?;
                report.fetched += 1;
            }
            Err(err) if err.is::<Unauthorized>() || err.is::<RateLimited>() => return Err(err),
            Err(err) => {
                tracing::warn!(
                    tmdb_id = id,
                    error = format!("{err:#}"),
                    "TMDB ratings request failed; will retry next run"
                );
                report.errors += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
