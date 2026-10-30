//! How TMDB matching has gone since this server started, for the Tasks page and the admin
//! health page: kept in memory, as the tasks are. What's said is in plain words chosen here,
//! never an error's own text, which can carry an address.

use std::sync::{LazyLock, Mutex};

use sqlx::SqlitePool;

use crate::admin_health::{Finding, Level};
use crate::metadata::RefreshReport;
use crate::tmdb::{RateLimited, Unauthorized};

/// What went wrong in a run of TMDB matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trouble {
    /// TMDB refused the token: nothing is matched or refreshed until it's changed.
    TokenRefused,
    /// TMDB asked for fewer requests than the run needed.
    RateLimited,
    /// The run stopped on something else; the server's log says what.
    Stopped,
    /// No title's request went through.
    NothingWentThrough,
    /// Some titles' requests failed; the rest went through.
    SomeFailed,
}

/// How TMDB matching has gone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The latest run's trouble, if it had any.
    pub trouble: Option<Trouble>,
    /// Runs in a row that got nothing from TMDB (stopped, or with no request through).
    pub failing_runs: u32,
}

impl Status {
    /// Notes how a run went.
    pub fn record(&mut self, run: &anyhow::Result<RefreshReport>) {
        self.trouble = trouble(run);
        let failed = matches!(
            self.trouble,
            Some(
                Trouble::TokenRefused
                    | Trouble::RateLimited
                    | Trouble::Stopped
                    | Trouble::NothingWentThrough
            )
        );
        self.failing_runs = if failed { self.failing_runs + 1 } else { 0 };
    }
}

/// What a run's outcome says went wrong, if anything.
fn trouble(run: &anyhow::Result<RefreshReport>) -> Option<Trouble> {
    match run {
        Err(err) if err.chain().any(|e| e.is::<Unauthorized>()) => Some(Trouble::TokenRefused),
        Err(err) if err.chain().any(|e| e.is::<RateLimited>()) => Some(Trouble::RateLimited),
        Err(_) => Some(Trouble::Stopped),
        Ok(report) if report.errors == 0 => None,
        Ok(report)
            if report.matched + report.unmatched + report.ambiguous + report.refreshed == 0 =>
        {
            Some(Trouble::NothingWentThrough)
        }
        Ok(_) => Some(Trouble::SomeFailed),
    }
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(Default::default);

fn status() -> std::sync::MutexGuard<'static, Status> {
    // A panic while held leaves nothing half-written worth refusing.
    STATUS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Notes how a run of TMDB matching went.
pub fn record(run: &anyhow::Result<RefreshReport>) {
    status().record(run);
}

/// How TMDB matching has gone since this server started.
pub fn current() -> Status {
    status().clone()
}

/// What admin health says of TMDB matching as it's gone: a problem when TMDB refused the token,
/// or when two runs in a row got nothing from it.
pub fn finding(status: &Status) -> Option<Finding> {
    let message = match (status.trouble, status.failing_runs) {
        (Some(Trouble::TokenRefused), _) => "TMDB refused its token at the last matching run, so nothing is matched or refreshed. Check the token under Outside services, then run TMDB matching from Tasks.".to_owned(),
        (_, runs) if runs >= 2 => format!(
            "TMDB matching got nothing from TMDB in its last {runs} runs: TMDB couldn't be reached, or answered with errors. The server's log says which."
        ),
        _ => return None,
    };
    Some(Finding {
        level: Level::Problem,
        code: if status.trouble == Some(Trouble::TokenRefused) {
            "tmdb_refused"
        } else {
            "tmdb_failing"
        },
        message,
        count: i64::from(status.failing_runs),
        id: None,
    })
}

/// How a run went, as the Tasks page shows it: what the library's titles stand at, and what
/// went wrong, in words safe to show.
pub async fn summary(pool: &SqlitePool, run: &anyhow::Result<RefreshReport>) -> String {
    let report = match (trouble(run), run) {
        (Some(Trouble::TokenRefused), _) => return "failed: TMDB refused the token".into(),
        (Some(Trouble::RateLimited), _) => {
            return "failed: TMDB asked for fewer requests; the next run carries on".into();
        }
        (_, Err(_)) => return "failed; the server's log says why".into(),
        (_, Ok(report)) => report,
    };
    let said = match trouble(run) {
        Some(Trouble::NothingWentThrough) => format!(
            "failed: no request to TMDB went through, for {}; the server's log says why",
            titles(report.errors)
        ),
        Some(_) => format!(
            "this run matched {}; {} failed and are tried again next run",
            report.matched, report.errors
        ),
        None => format!("this run matched {}", report.matched),
    };
    match totals(pool).await {
        Ok((matched, open)) => format!("{said}. In all, {matched} matched, {open} need a match"),
        Err(err) => {
            tracing::warn!(error = %err, "couldn't count matched titles");
            said
        }
    }
}

fn titles(n: usize) -> String {
    format!("{n} {}", if n == 1 { "title" } else { "titles" })
}

/// The library's titles matched to TMDB, and those it couldn't settle that no one has decided
/// by hand (Needs a match).
async fn totals(pool: &SqlitePool) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT count(CASE WHEN match_status = 'matched' THEN 1 END),
                count(CASE WHEN match_status IN ('unmatched', 'ambiguous')
                    AND match_method IS NOT 'manual' THEN 1 END)
         FROM (SELECT match_status, match_method FROM movies WHERE missing_since IS NULL
               UNION ALL
               SELECT match_status, match_method FROM series WHERE missing_since IS NULL)",
    )
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests;
