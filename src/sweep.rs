//! A library's Scan and search: the library is scanned (and its new files matched and
//! probed, as any scan is), then, as asked, its missing titles are searched for, then its files
//! below their profile's cutoff, then the subtitles its films and episodes lack are fetched. Each
//! search step is a run of [`search_missing`] for that library's titles alone, with the same
//! pacing, limits and progress as Search all missing; one in the area already under way is let
//! finish first. Movie and series libraries only; one sweep at a time per library.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::library::{Library, LibraryKind};
use crate::search_missing::{self, Already, Area, Goal};

/// How often a step under way is looked at.
pub const POLL: Duration = Duration::from_secs(10);

/// What's done after the scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asked {
    pub missing: bool,
    pub upgrades: bool,
    pub subtitles: bool,
}

/// What a sweep needs from outside.
#[derive(Clone)]
pub struct Sources {
    /// What its searches need.
    pub search: search_missing::Sources,
    /// Where fetched subtitles are kept.
    pub subtitles: PathBuf,
    /// [`POLL`], but less in tests.
    pub poll: Duration,
    /// Scanned through the library task, so new files are matched and probed before anything
    /// is searched for; tests scan directly.
    pub through_jobs: bool,
}

/// A sweep, and how far it has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Sweep {
    pub id: i64,
    /// The library's name.
    pub library: String,
    /// "scanning", "missing", "upgrades", "subtitles" or "done": the step under way, or the
    /// last one reached.
    pub step: String,
    /// "running", "done", or "failed", with why in `message`.
    pub state: String,
    pub missing: bool,
    pub upgrades: bool,
    pub subtitles: bool,
    /// The runs it started (`GET /api/v1/missing-searches/{id}`).
    pub missing_search_id: Option<i64>,
    pub upgrade_search_id: Option<i64>,
    pub subtitles_fetched: i64,
    /// How it went, in plain words, once it has ended.
    pub message: Option<String>,
    /// The username of who started it.
    pub started_by: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// Why a sweep wasn't started.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    NoSuchLibrary,
    /// A music library: albums are searched for under Music.
    Music,
    /// Its library isn't responding, as said.
    NotResponding(String),
    /// One is under way for the library already.
    Already(i64),
}

const COLUMNS: &str = "SELECT w.id, l.name AS library, w.step, w.state, w.missing, w.upgrades,
        w.subtitles, w.missing_search_id, w.upgrade_search_id, w.subtitles_fetched, w.message,
        u.username AS started_by, w.started_at, w.finished_at
     FROM library_sweeps w JOIN libraries l ON l.id = w.library_id
     LEFT JOIN users u ON u.id = w.started_by";

/// A sweep by its id.
pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Sweep>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{COLUMNS} WHERE w.id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// The latest sweep of the library called `name`.
pub async fn latest(pool: &SqlitePool, name: &str) -> sqlx::Result<Option<Sweep>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COLUMNS} WHERE l.name = ? ORDER BY w.id DESC LIMIT 1"
    )))
    .bind(name)
    .fetch_optional(pool)
    .await
}

/// Marks sweeps a restart cut off; the runs they started are marked by their own.
pub async fn recover(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE library_sweeps SET state = 'failed', message = 'a restart cut it off',
             finished_at = ?
         WHERE state = 'running'",
    )
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Starts a sweep of the library called `name` in the background; returns it as it starts.
pub async fn start(
    pool: &SqlitePool,
    sources: Sources,
    name: &str,
    asked: Asked,
    started_by: i64,
) -> anyhow::Result<Result<Sweep, Refused>> {
    let found: Option<(i64, String, String, String)> =
        sqlx::query_as("SELECT id, name, kind, path FROM libraries WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    let Some((id, name, kind, path)) = found else {
        return Ok(Err(Refused::NoSuchLibrary));
    };
    let library = Library {
        id,
        name,
        kind: LibraryKind::parse(&kind).unwrap_or(LibraryKind::Music),
        path: path.into(),
    };
    if library.kind == LibraryKind::Music {
        return Ok(Err(Refused::Music));
    }
    if let Some(down) = crate::library::health::down_at(&library.path) {
        return Ok(Err(Refused::NotResponding(down.saying())));
    }
    let inserted: sqlx::Result<i64> = sqlx::query_scalar(
        "INSERT INTO library_sweeps (library_id, step, state, missing, upgrades, subtitles,
             started_by, started_at)
         VALUES (?, 'scanning', 'running', ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(library.id)
    .bind(asked.missing)
    .bind(asked.upgrades)
    .bind(asked.subtitles)
    .bind(started_by)
    .bind(unix_now())
    .fetch_one(pool)
    .await;
    let sweep = match inserted {
        Ok(sweep) => sweep,
        // The index allows one at a time.
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
            let running: i64 = sqlx::query_scalar(
                "SELECT id FROM library_sweeps WHERE library_id = ? AND state = 'running'",
            )
            .bind(library.id)
            .fetch_one(pool)
            .await?;
            return Ok(Err(Refused::Already(running)));
        }
        Err(err) => return Err(err.into()),
    };
    let started = get(pool, sweep)
        .await?
        .ok_or_else(|| anyhow::anyhow!("sweep {sweep} vanished"))?;
    let pool = pool.clone();
    tokio::spawn(async move {
        let ran = run(&pool, &sources, sweep, &library, asked, started_by).await;
        let (state, message) = match ran {
            Ok(said) => ("done", said),
            Err(err) => ("failed", format!("{err:#}")),
        };
        let ended = sqlx::query(
            "UPDATE library_sweeps SET state = ?, message = ?, finished_at = ?,
                 step = CASE WHEN ? = 'done' THEN 'done' ELSE step END
             WHERE id = ?",
        )
        .bind(state)
        .bind(&message)
        .bind(unix_now())
        .bind(state)
        .bind(sweep)
        .execute(&pool)
        .await;
        match ended {
            Ok(_) => tracing::info!(
                library = library.name,
                state,
                message,
                "scan and search ended"
            ),
            Err(err) => tracing::warn!(error = %err, "couldn't record how a scan and search ended"),
        }
    });
    Ok(Ok(started))
}

async fn step(pool: &SqlitePool, sweep: i64, step: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE library_sweeps SET step = ? WHERE id = ?")
        .bind(step)
        .bind(sweep)
        .execute(pool)
        .await?;
    Ok(())
}

/// The sweep's steps; answers what each did, in a sentence.
async fn run(
    pool: &SqlitePool,
    sources: &Sources,
    sweep: i64,
    library: &Library,
    asked: Asked,
    started_by: i64,
) -> anyhow::Result<String> {
    if sources.through_jobs {
        let ticket = crate::jobs::request_folder_scan(library.id, None);
        crate::jobs::scans::finished_with(ticket).await;
    } else {
        crate::library::scan(pool, library).await?;
    }
    // A scan of a library that stopped answering finds nothing to go on.
    if let Some(down) = crate::library::health::down_at(&library.path) {
        anyhow::bail!("{}: {}", down.saying(), down.why);
    }
    let mut said = vec![format!("{} scanned", library.name)];
    let area = match library.kind {
        LibraryKind::Movies => Area::Movies,
        _ => Area::Series,
    };
    let searches = [
        (
            asked.missing,
            Goal::Missing,
            "missing",
            "missing_search_id",
            "missing",
        ),
        (
            asked.upgrades,
            Goal::CutoffUnmet,
            "upgrades",
            "upgrade_search_id",
            "upgrades",
        ),
    ];
    for (wanted, goal, name, column, words) in searches {
        if !wanted {
            continue;
        }
        step(pool, sweep, name).await?;
        let (run, message) = search(pool, sources, (area, library.id), goal, started_by, |run| {
            let pool = pool.clone();
            async move {
                sqlx::query(sqlx::AssertSqlSafe(format!(
                    "UPDATE library_sweeps SET {column} = ? WHERE id = ?"
                )))
                .bind(run)
                .bind(sweep)
                .execute(&pool)
                .await
                .map(|_| ())
            }
        })
        .await?;
        said.push(format!("{words} (search {run}): {message}"));
    }
    if asked.subtitles {
        step(pool, sweep, "subtitles").await?;
        let fetched =
            crate::opensubtitles::fetch_for_library(pool, &sources.subtitles, library.id).await;
        said.push(match fetched {
            Ok(Some(report)) => {
                sqlx::query("UPDATE library_sweeps SET subtitles_fetched = ? WHERE id = ?")
                    .bind(i64::try_from(report.fetched)?)
                    .bind(sweep)
                    .execute(pool)
                    .await?;
                format!(
                    "subtitles: {} fetched, {} with none made for the file",
                    report.fetched, report.none_made
                )
            }
            Ok(None) => "subtitles: none fetched, as OpenSubtitles has no languages to fetch for imports (Outside services)".into(),
            Err(err) => format!("subtitles: stopped: {err:#}"),
        });
    }
    Ok(said.join("; "))
}

/// Runs a search of the library's titles for `goal`, after any run already under way in its
/// area; `started` is told the run's id as it starts. Answers its id and how it ended.
async fn search<F, Fut>(
    pool: &SqlitePool,
    sources: &Sources,
    (area, library): (Area, i64),
    goal: Goal,
    started_by: i64,
    started: F,
) -> anyhow::Result<(i64, String)>
where
    F: Fn(i64) -> Fut,
    Fut: Future<Output = sqlx::Result<()>>,
{
    let run = loop {
        let search = sources.search.clone();
        match search_missing::start_in(pool, search, area, goal, Some(library), started_by).await? {
            Ok(run) => break run.id,
            Err(Already(_)) => tokio::time::sleep(sources.poll).await,
        }
    };
    started(run).await?;
    loop {
        let Some(now) = search_missing::get(pool, run).await? else {
            anyhow::bail!("search {run} vanished");
        };
        if !matches!(now.state.as_str(), "running" | "stopping") {
            return Ok((run, now.message.unwrap_or_else(|| now.state.clone())));
        }
        tokio::time::sleep(sources.poll).await;
    }
}

#[cfg(test)]
mod tests;
