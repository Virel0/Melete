//! How far the scan running now has got, for the live event stream: a row in `scan_progress`
//! while a library (or one of its folders) is scanned, with the folders its walk has read,
//! written at most once a [`TICK`] while the walk runs on its blocking thread. Kept in the
//! database, so each server (and each test's database) has its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;

/// How often the folders read so far are written down.
pub const TICK: Duration = Duration::from_secs(1);

/// A scan under way: its row, and the count its walk raises.
pub(super) struct Progress {
    pool: SqlitePool,
    library_id: i64,
    folders: Arc<AtomicUsize>,
}

impl Progress {
    /// Notes a scan of `library_id` starting, whole or of one folder. Writing it down is only
    /// for the stream to show, so a database too busy to take it doesn't stop the scan.
    pub(super) async fn start(pool: &SqlitePool, library_id: i64, whole: bool) -> Self {
        let now = unix_now();
        let noted = sqlx::query(
            "INSERT INTO scan_progress (library_id, whole, folders_done, started_at, updated_at)
             VALUES (?, ?, 0, ?, ?)
             ON CONFLICT (library_id) DO UPDATE SET whole = excluded.whole, folders_done = 0,
                 started_at = excluded.started_at, updated_at = excluded.updated_at",
        )
        .bind(library_id)
        .bind(whole)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await;
        if let Err(err) = noted {
            tracing::debug!(error = %err, "couldn't note a scan starting");
        }
        Self {
            pool: pool.clone(),
            library_id,
            folders: Arc::default(),
        }
    }

    /// The count the walk raises for each folder it reads.
    pub(super) fn folders(&self) -> Arc<AtomicUsize> {
        self.folders.clone()
    }

    /// Runs `walk` (the blocking walk, raising [`Self::folders`]), writing the count down every
    /// `tick` while it runs, and once more when it's done.
    pub(super) async fn walking<T>(&self, tick: Duration, walk: impl Future<Output = T>) -> T {
        let walk = std::pin::pin!(walk);
        let mut walk = walk;
        let mut written = 0;
        let walked = loop {
            tokio::select! {
                walked = &mut walk => break walked,
                () = tokio::time::sleep(tick) => written = self.write(Some(written)).await,
            }
        };
        self.write(None).await;
        walked
    }

    /// Writes the count down when it's moved on from `written` (always, when that's None);
    /// answers what's written now.
    async fn write(&self, written: Option<usize>) -> usize {
        let now = self.folders.load(Ordering::Relaxed);
        let written = match written {
            Some(written) if written == now => return written,
            Some(written) => written,
            None => 0,
        };
        let wrote = sqlx::query(
            "UPDATE scan_progress SET folders_done = ?, updated_at = ? WHERE library_id = ?",
        )
        .bind(i64::try_from(now).unwrap_or(i64::MAX))
        .bind(unix_now())
        .bind(self.library_id)
        .execute(&self.pool)
        .await;
        match wrote {
            Ok(_) => now,
            Err(err) => {
                tracing::debug!(error = %err, "couldn't note a scan's progress");
                written
            }
        }
    }

    /// Notes the scan ended, however it went.
    pub(super) async fn finish(self) {
        let removed = sqlx::query("DELETE FROM scan_progress WHERE library_id = ?")
            .bind(self.library_id)
            .execute(&self.pool)
            .await;
        if let Err(err) = removed {
            tracing::warn!(error = %err, "couldn't note a scan ending");
        }
    }
}

/// Forgets scans a stopped server left noted as running.
pub async fn recover(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM scan_progress")
        .execute(pool)
        .await?;
    Ok(())
}

/// A scan running now, as the event stream sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Running {
    /// The library's name.
    pub library: String,
    /// Its kind: movies, shows or music.
    pub kind: String,
    /// The whole library, or one title folder in it.
    pub whole: bool,
    /// Folders read so far.
    pub folders_done: i64,
    /// When it started, Unix seconds.
    pub started_at: i64,
}

/// The scans running now in the libraries `library_sql` (SQL over `p.library_id`) lets through,
/// of video (movies and shows) and music as asked.
pub async fn running(
    pool: &SqlitePool,
    library_sql: &str,
    video: bool,
    music: bool,
) -> sqlx::Result<Vec<Running>> {
    let rows: Vec<(String, String, bool, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT l.name, l.kind, p.whole, p.folders_done, p.started_at
             FROM scan_progress p JOIN libraries l ON l.id = p.library_id
             WHERE {library_sql}
               AND ((?1 AND l.kind IN ('movies', 'shows')) OR (?2 AND l.kind = 'music'))
             ORDER BY l.name"
    )))
    .bind(video)
    .bind(music)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(library, kind, whole, folders_done, started_at)| Running {
            library,
            kind,
            whole,
            folders_done,
            started_at,
        })
        .collect())
}

#[cfg(test)]
mod tests;
