//! Bringing finished downloads home: over SFTP from the seedbox straight into the library
//! folder they belong in, each file under a hidden temporary name (which
//! scans skip) and then put in place. A transfer cut off resumes where it stopped.
//!
//! Nothing in the library is overwritten, moved or deleted: a file of the same
//! name already there stops the import, and new files are put in place with a hard link, which
//! fails rather than replace one. Only regular files inside Deluge's folder for finished
//! torrents are fetched; links on the seedbox are refused.
//!
//! An album goes in "Album (Year)" in its artist's folder in the music library:
//! its tracks, keeping their disc folders, and its cover image. Its tracks are then fixed
//! ([`album_fix`]) before the download counts as imported.

use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::album_fix::{self, Fixes};
use crate::db::unix_now;
use crate::deluge::TorrentFile;
use crate::downloads::Pipeline;
use crate::library::{AUDIO_EXTENSIONS, COVER_NAMES, IMAGE_EXTENSIONS};
use crate::seedbox::{Connection, Seedbox};

mod copy;
mod destination;
mod picking;
mod place;
mod tracking;

pub use copy::*;
pub(crate) use destination::*;
pub(crate) use picking::*;
pub(crate) use place::*;
use tracking::*;

/// How often to look for finished downloads when there's nothing to do.
const IDLE: Duration = Duration::from_secs(60);
/// The longest wait between attempts.
const MAX_BACKOFF: i64 = 6 * 3600;
/// Tries before a download that keeps failing is given up on (about two and a half days).
const MAX_ATTEMPTS: i64 = 12;

/// What's fetched: videos and subtitles. Samples, screenshots and info files stay behind, as
/// a cron-based sync script would leave them.
const VIDEO: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "ts", "m2ts", "mov", "wmv", "webm", "mpg", "mpeg",
];
const SUBTITLES: &[&str] = &["srt", "ass", "ssa", "sub", "idx", "vtt", "sup"];

/// Why a transfer stopped.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Worth trying again later (the network, the seedbox, a read-only library).
    Retry(String),
    /// Won't fix itself; the download is marked failed.
    Fail(String),
}

impl From<std::io::Error> for Stop {
    fn from(err: std::io::Error) -> Self {
        match err.kind() {
            ErrorKind::ReadOnlyFilesystem | ErrorKind::PermissionDenied => Stop::Retry(format!(
                "the library folder isn't writable ({err}); the libraries are mounted read-only until automation is turned on (drop \":ro\" from their mounts in compose.yaml)"
            )),
            _ => Stop::Retry(format!("writing to the library: {err}")),
        }
    }
}

/// Copies finished downloads home for as long as the server runs.
pub async fn work(pool: SqlitePool, pipeline: Arc<Pipeline>, fixes: Arc<Fixes>) {
    let Some(seedbox) = pipeline.seedbox.as_ref() else {
        return;
    };
    // A transfer cut off by a restart starts again, resuming its files.
    if let Err(err) =
        sqlx::query(
            "UPDATE downloads SET state = 'completed' WHERE state = 'transferring' AND source = 'torrent'",
        )
            .execute(&pool)
            .await
    {
        tracing::warn!(error = %err, "couldn't reset interrupted transfers");
    }
    loop {
        match next_due(&pool).await {
            Ok(Some(id)) => {
                if let Err(err) = transfer(&pool, seedbox, Some(&fixes), id).await {
                    // Don't leave it "transferring" until the next restart.
                    let _ = sqlx::query(
                        "UPDATE downloads SET state = 'completed', next_attempt_at = ? WHERE id = ? AND state = 'transferring'",
                    )
                    .bind(unix_now() + IDLE.as_secs() as i64)
                    .bind(id)
                    .execute(&pool)
                    .await;
                    tracing::warn!(
                        download = id,
                        error = format!("{err:#}"),
                        "transfer bookkeeping failed"
                    );
                    tokio::time::sleep(IDLE).await;
                }
            }
            Ok(None) => tokio::time::sleep(IDLE).await,
            Err(err) => {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "couldn't look for finished downloads"
                );
                tokio::time::sleep(IDLE).await;
            }
        }
    }
}

async fn next_due(pool: &SqlitePool) -> anyhow::Result<Option<i64>> {
    Ok(sqlx::query_scalar(
        // A Soulseek download is brought home from slskd's folder, not the seedbox.
        "SELECT id FROM downloads
         WHERE state = 'completed' AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
           AND source = 'torrent'
         ORDER BY completed_at, id LIMIT 1",
    )
    .bind(unix_now())
    .fetch_optional(pool)
    .await?)
}

#[derive(sqlx::FromRow)]
struct Row {
    title: String,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    season: Option<i64>,
    wanted_album_id: Option<i64>,
    save_path: Option<String>,
    files: Option<String>,
    attempts: i64,
}

/// Where a download goes and what's copied.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The library the folder is in.
    pub(crate) root: PathBuf,
    /// The folder the files go in.
    pub(crate) dir: PathBuf,
    pub(crate) files: Vec<Planned>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Planned {
    pub(crate) remote: String,
    pub(crate) size: u64,
    /// The folder below the plan's it goes in (an album's disc folder), or "".
    pub(crate) folder: String,
    pub(crate) name: String,
}

/// Copies one finished download home, fixes an album's tracks when `fixes` is given, and
/// records how it went.
pub async fn transfer(
    pool: &SqlitePool,
    seedbox: &Seedbox,
    fixes: Option<&Fixes>,
    id: i64,
) -> anyhow::Result<()> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT title, wanted_movie_id, wanted_series_id, season, wanted_album_id, save_path, files,
                attempts
         FROM downloads WHERE id = ? AND state = 'completed'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(()) };
    let plan = match plan(pool, seedbox, &row).await? {
        Ok(plan) => plan,
        Err(stop) => return finish(pool, id, row.attempts, Err(stop)).await,
    };
    let claimed = sqlx::query(
        "UPDATE downloads SET state = 'transferring', message = 'connecting to the seedbox', updated_at = ?
         WHERE id = ? AND state = 'completed'",
    )
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(());
    }
    let outcome = match copy_all(pool, seedbox, id, &plan).await {
        Ok(()) => match (row.wanted_album_id, fixes) {
            (Some(album), Some(fixes)) => fix(pool, fixes, id, album, &plan).await,
            _ => Ok("in the library".to_owned()),
        },
        Err(stop) => Err(stop),
    };
    finish(
        pool,
        id,
        row.attempts,
        outcome.map(|note| (plan.dir.clone(), note)),
    )
    .await
}

/// Fixes the album's tracks just placed ([`album_fix`]), and only those, before the download
/// counts as imported.
pub(crate) async fn fix(
    pool: &SqlitePool,
    fixes: &Fixes,
    id: i64,
    album: i64,
    plan: &Plan,
) -> Result<String, Stop> {
    sqlx::query(
        "UPDATE downloads SET message = 'measuring and tagging', updated_at = ? WHERE id = ?",
    )
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await
    .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
    let tracks: Vec<PathBuf> = plan
        .files
        .iter()
        .filter(|f| has_extension(&f.name, AUDIO_EXTENSIONS))
        .map(|f| plan.dir.join(&f.folder).join(&f.name))
        .collect();
    let has_cover = plan.files.iter().any(|f| is_cover(&f.name));
    album_fix::fix(pool, fixes, id, album, &plan.dir, &tracks, has_cover)
        .await
        .map_err(Stop::Retry)
}

pub(crate) async fn finish(
    pool: &SqlitePool,
    id: i64,
    attempts: i64,
    outcome: Result<(PathBuf, String), Stop>,
) -> anyhow::Result<()> {
    let now = unix_now();
    match outcome {
        Ok((dir, message)) => {
            // So the new files show up now rather than at the next scheduled scan: only the
            // folder they went into is scanned, which wakes no other disk.
            crate::library::ask_scan_of(pool, &dir).await?;
            sqlx::query(
                "UPDATE downloads SET state = 'imported', destination = ?, attempts = 0, next_attempt_at = NULL,
                     message = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(dir.to_string_lossy().into_owned())
            .bind(message)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
        Err(Stop::Retry(why)) if attempts + 1 >= MAX_ATTEMPTS => {
            sqlx::query(
                "UPDATE downloads SET state = 'failed', message = ?, updated_at = ? WHERE id = ?",
            )
            .bind(format!("gave up after {MAX_ATTEMPTS} tries: {why}"))
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
        Err(Stop::Retry(why)) => {
            let wait = (60i64 << attempts.clamp(0, 20)).min(MAX_BACKOFF);
            sqlx::query(
                "UPDATE downloads SET state = 'completed', attempts = attempts + 1, next_attempt_at = ?,
                     message = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(now + wait)
            .bind(format!("{why}; trying again in {} minutes", wait / 60))
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
        Err(Stop::Fail(why)) => {
            sqlx::query(
                "UPDATE downloads SET state = 'failed', message = ?, updated_at = ? WHERE id = ?",
            )
            .bind(why)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
