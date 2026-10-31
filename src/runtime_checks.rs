//! Downloads of a wanted title held back by the running-time check, counted per title. After
//! [`STOP_AFTER`], Melete stops searching for the title by itself (TMDB's runtime may be what's
//! wrong, and each search would fetch another release in full), and admin health says so. An
//! admin can say the runtime is right after all: the check then goes by how long the last
//! held-back film ran (or isn't made, for a series or a film whose running time couldn't be
//! read) while TMDB's runtime stays what it was, and searching resumes. The count starts again
//! on an import or that override.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::admin_health::{Finding, Level};
use crate::db::unix_now;
use crate::search::{Target, Title};

/// Held-back downloads after which Melete stops searching for a title by itself.
pub const STOP_AFTER: i64 = 3;
/// Failures kept with their reasons.
const KEPT: usize = 5;

/// One held-back download.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// When, in Unix seconds.
    pub at: i64,
    /// The download, while it's on the list.
    pub download_id: i64,
    /// The release.
    pub release: String,
    /// Why it was held back.
    pub reason: String,
}

/// Where a title stands with the running-time check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// Downloads held back since the last import or override.
    pub failures: i64,
    /// Whether Melete has stopped searching for it by itself.
    pub stopped: bool,
    /// The latest failures, newest last.
    pub reasons: Vec<Failure>,
    /// Whether an admin's "the runtime is right after all" holds now (TMDB's runtime is what it
    /// was then).
    pub overridden: bool,
    /// The running time checked against while it holds, in seconds; null when nothing is
    /// checked.
    pub override_seconds: Option<f64>,
}

/// The title a download's search was for.
pub(crate) fn title_of(target: Target) -> Title {
    match target {
        Target::Movie(id) => Title::Movie(id),
        Target::Series { id, .. } => Title::Series(id),
    }
}

fn key(title: Title) -> (&'static str, i64) {
    match title {
        Title::Movie(id) => ("movie", id),
        Title::Series(id) => ("series", id),
    }
}

/// TMDB's runtime for the title as it is now: a movie's minutes, a series' episodes' minutes.
async fn signature(pool: &SqlitePool, title: Title) -> sqlx::Result<Option<String>> {
    match title {
        Title::Movie(_) => sqlx::query_scalar(
            "SELECT CAST(t.runtime_minutes AS TEXT) FROM wanted_movies w
             JOIN tmdb_movies t ON t.tmdb_id = w.tmdb_id WHERE w.id = ?",
        ),
        Title::Series(_) => sqlx::query_scalar(
            "SELECT group_concat(season || ':' || episode || ':' || coalesce(runtime_minutes, ''), ',')
             FROM (SELECT e.season, e.episode, e.runtime_minutes FROM wanted_series w
                   JOIN tmdb_episodes e ON e.series_tmdb_id = w.tmdb_id WHERE w.id = ?
                   ORDER BY e.season, e.episode)",
        ),
    }
    .bind(key(title).1)
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// Counts a download held back for the title, with why and how long its video ran.
pub(crate) async fn record_failure(
    pool: &SqlitePool,
    title: Title,
    failure: Failure,
    seconds: Option<f64>,
) -> anyhow::Result<()> {
    let (kind, id) = key(title);
    let kept: Option<String> =
        sqlx::query_scalar("SELECT reasons FROM runtime_checks WHERE kind = ? AND wanted_id = ?")
            .bind(kind)
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let mut reasons: Vec<Failure> = kept
        .map(|r| serde_json::from_str(&r).unwrap_or_default())
        .unwrap_or_default();
    reasons.push(failure);
    let drop = reasons.len().saturating_sub(KEPT);
    reasons.drain(..drop);
    sqlx::query(
        "INSERT INTO runtime_checks (kind, wanted_id, failures, reasons, last_seconds)
         VALUES (?1, ?2, 1, ?3, ?4)
         ON CONFLICT (kind, wanted_id) DO UPDATE SET failures = failures + 1,
             reasons = excluded.reasons, last_seconds = excluded.last_seconds",
    )
    .bind(kind)
    .bind(id)
    .bind(serde_json::to_string(&reasons)?)
    .bind(seconds)
    .execute(pool)
    .await?;
    Ok(())
}

/// Starts the count again: a download of the title was imported.
pub(crate) async fn reset(pool: &SqlitePool, title: Title) -> sqlx::Result<()> {
    let (kind, id) = key(title);
    sqlx::query(
        "UPDATE runtime_checks SET failures = 0, reasons = '[]', last_seconds = NULL
         WHERE kind = ? AND wanted_id = ?",
    )
    .bind(kind)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether Melete has stopped searching for the title by itself.
pub(crate) async fn stopped(pool: &SqlitePool, title: Title) -> sqlx::Result<bool> {
    let (kind, id) = key(title);
    let failures: Option<i64> =
        sqlx::query_scalar("SELECT failures FROM runtime_checks WHERE kind = ? AND wanted_id = ?")
            .bind(kind)
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(failures.unwrap_or(0) >= STOP_AFTER)
}

/// An admin's override that holds now: Some(the running time to check against), Some(None)
/// for no check; None when there's none or TMDB's runtime has changed since.
pub(crate) async fn override_for(
    pool: &SqlitePool,
    title: Title,
) -> sqlx::Result<Option<Option<f64>>> {
    let (kind, id) = key(title);
    let row: Option<(Option<f64>, Option<String>)> = sqlx::query_as(
        "SELECT override_seconds, override_tmdb FROM runtime_checks
         WHERE kind = ? AND wanted_id = ? AND override_at IS NOT NULL",
    )
    .bind(kind)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((seconds, then)) = row else {
        return Ok(None);
    };
    Ok((then == signature(pool, title).await?).then_some(seconds))
}

async fn exists(pool: &SqlitePool, title: Title) -> sqlx::Result<bool> {
    let query = match title {
        Title::Movie(_) => "SELECT EXISTS (SELECT 1 FROM wanted_movies WHERE id = ?)",
        Title::Series(_) => "SELECT EXISTS (SELECT 1 FROM wanted_series WHERE id = ?)",
    };
    sqlx::query_scalar(query)
        .bind(key(title).1)
        .fetch_one(pool)
        .await
}

/// Where the title stands; None when there's no such wanted title.
pub async fn status(pool: &SqlitePool, title: Title) -> anyhow::Result<Option<Status>> {
    if !exists(pool, title).await? {
        return Ok(None);
    }
    let (kind, id) = key(title);
    let row: Option<(i64, String, Option<f64>)> = sqlx::query_as(
        "SELECT failures, reasons, override_seconds FROM runtime_checks
         WHERE kind = ? AND wanted_id = ?",
    )
    .bind(kind)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let (failures, reasons, override_seconds) = row.unwrap_or((0, "[]".into(), None));
    let overridden = override_for(pool, title).await?.is_some();
    Ok(Some(Status {
        failures,
        stopped: failures >= STOP_AFTER,
        reasons: serde_json::from_str(&reasons)?,
        overridden,
        override_seconds: if overridden { override_seconds } else { None },
    }))
}

/// An admin says the title's runtime is right after all: from now the check goes by how long
/// the last held-back film ran (none for a series, or when that couldn't be read) while TMDB's
/// runtime stays as it is, the count starts again, and searching resumes. None when there's no
/// such wanted title.
pub async fn say_right(pool: &SqlitePool, title: Title) -> anyhow::Result<Option<Status>> {
    if !exists(pool, title).await? {
        return Ok(None);
    }
    let (kind, id) = key(title);
    let tmdb = signature(pool, title).await?;
    sqlx::query(
        "INSERT INTO runtime_checks (kind, wanted_id, override_at, override_tmdb)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (kind, wanted_id) DO UPDATE SET failures = 0, reasons = '[]',
             override_at = ?3, override_tmdb = ?4,
             override_seconds = CASE WHEN kind = 'movie' THEN last_seconds END,
             last_seconds = NULL",
    )
    .bind(kind)
    .bind(id)
    .bind(unix_now())
    .bind(tmdb)
    .execute(pool)
    .await?;
    status(pool, title).await
}

/// Admin health's findings: each title Melete stopped searching for, in its area.
pub async fn findings(pool: &SqlitePool) -> anyhow::Result<Vec<(&'static str, Finding)>> {
    let rows: Vec<(i64, String, i64, String, String)> = sqlx::query_as(
        "SELECT r.wanted_id, r.reasons, r.failures, t.service, t.title FROM runtime_checks r
         JOIN title_event_titles t ON t.kind = r.kind AND t.wanted_id = r.wanted_id
         WHERE r.failures >= ? ORDER BY r.kind, r.wanted_id",
    )
    .bind(STOP_AFTER)
    .fetch_all(pool)
    .await?;
    let mut found = Vec::new();
    for (id, reasons, failures, service, title) in rows {
        let area = match service.as_str() {
            "series" => "series",
            "anime" => "anime",
            _ => "movies",
        };
        let reasons: Vec<Failure> = serde_json::from_str(&reasons).unwrap_or_default();
        let said: Vec<String> = reasons
            .iter()
            .map(|f| format!("{}: {}", f.release, f.reason))
            .collect();
        found.push((
            area,
            Finding {
                level: Level::Problem,
                code: "runtime_checks_failed",
                message: format!(
                    "Melete stopped searching for {title} by itself after {failures} downloads didn't pass the running-time check ({}). If its runtime is right after all, say so on its page.",
                    said.join("; ")
                ),
                count: failures,
                id: Some(id),
            },
        ));
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
