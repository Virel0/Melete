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
use crate::search::Target;
use crate::seedbox::{Connection, Seedbox};

mod again;
mod cleanup;
mod copy;
mod destination;
mod picking;
mod place;
mod verify;

pub use again::*;
use cleanup::*;
pub use copy::*;
pub(crate) use destination::*;
pub(crate) use picking::*;
pub(crate) use place::*;

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
    /// It doesn't run as long as the title it's for, or how long it runs can't be read: it's
    /// held back (nothing placed, its part files gone), marked failed and blocklisted.
    NotTheTitle(String),
    /// Not the title, but an earlier try already put some of its files in the library: nothing
    /// more is placed, those stay, and it's marked failed and blocklisted for an admin to check,
    /// without searching again.
    Held(String),
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

/// [`work_with`] without TMDB.
#[cfg(test)]
pub async fn work(pool: SqlitePool, pipeline: Arc<Pipeline>, fixes: Arc<Fixes>) {
    work_with(pool, pipeline, fixes, None).await;
}

/// Copies finished downloads home for as long as the server runs, searching again (with TMDB
/// when it's set up) for a title whose download proved not to be it.
pub async fn work_with(
    pool: SqlitePool,
    pipeline: Arc<Pipeline>,
    fixes: Arc<Fixes>,
    tmdb: Option<Arc<crate::tmdb::Client>>,
) {
    let Some(seedbox) = pipeline.seedbox.as_ref() else {
        return;
    };
    let sources = crate::fulfil::Sources {
        tmdb,
        slskd: None,
        pipeline: Some(pipeline.clone()),
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
                if let Err(err) = bring_home(&pool, seedbox, Some(&fixes), &sources, id).await {
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
    episode: Option<i64>,
    wanted_album_id: Option<i64>,
    save_path: Option<String>,
    files: Option<String>,
    attempts: i64,
}

impl Row {
    /// The search the download was for: its movie, or its season or episode.
    fn target(&self) -> Option<Target> {
        match (self.wanted_movie_id, self.wanted_series_id, self.season) {
            (Some(id), _, _) => Some(Target::Movie(id)),
            (None, Some(id), Some(season)) => Some(Target::Series {
                id,
                season,
                episode: self.episode,
            }),
            _ => None,
        }
    }
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

/// Copies one finished download home, checks a movie or episode runs as long as the title
/// (with `fixes`' ffprobe) before it's put in place, fixes an album's tracks when `fixes` is
/// given, and records how it went. Returns the title to search for again when the download
/// proved not to be it.
pub async fn transfer(
    pool: &SqlitePool,
    seedbox: &Seedbox,
    fixes: Option<&Fixes>,
    id: i64,
) -> anyhow::Result<Option<Target>> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT title, wanted_movie_id, wanted_series_id, season, episode, wanted_album_id,
                save_path, files, attempts
         FROM downloads WHERE id = ? AND state = 'completed'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let plan = match plan(pool, seedbox, &row).await? {
        Ok(plan) => plan,
        Err(stop) => {
            finish(pool, id, row.attempts, Err(stop)).await?;
            return Ok(None);
        }
    };
    let check = verify::expected(pool, &row)
        .await?
        .map(|expected| verify::Check {
            expected,
            ffprobe: fixes.map(|f| f.tools.ffprobe.clone()),
        });
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
        return Ok(None);
    }
    let outcome = match copy_all(pool, seedbox, id, &plan, check.as_ref()).await {
        Ok(()) => match (row.wanted_album_id, fixes) {
            (Some(album), Some(fixes)) => fix(pool, fixes, id, album, &plan).await,
            _ => Ok("in the library".to_owned()),
        },
        Err(stop) => Err(stop),
    };
    let not_the_title = matches!(outcome, Err(Stop::NotTheTitle(_)));
    // What won't be retried leaves no part files, and no empty folders it made.
    if matches!(
        outcome,
        Err(Stop::Fail(_) | Stop::NotTheTitle(_) | Stop::Held(_))
    ) {
        clean_up(pool, id).await;
    }
    finish(
        pool,
        id,
        row.attempts,
        outcome.map(|note| (plan.dir.clone(), note)),
    )
    .await?;
    Ok(not_the_title.then(|| row.target()).flatten())
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
        Err(stop @ (Stop::NotTheTitle(_) | Stop::Held(_))) => {
            crate::blocklist::add(pool, crate::blocklist::From::Download(id)).await?;
            let message = match stop {
                Stop::Held(why) => format!(
                    "nothing more put in the library, as it may be another title: {why}; the release is blocklisted"
                ),
                Stop::NotTheTitle(why) | Stop::Retry(why) | Stop::Fail(why) => format!(
                    "not put in the library, as it may be another title: {why}; the release is blocklisted"
                ),
            };
            sqlx::query(
                "UPDATE downloads SET state = 'failed', message = ?, updated_at = ? WHERE id = ?",
            )
            .bind(message)
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
