//! How far a copy home has got: bytes copied out of the download's total, with the speed, written
//! to the download at most once a second (and at the start and end of each file), so the
//! downloads list and the live progress stream can show it.

use std::time::{Duration, Instant};

use super::*;

/// The least time between two writes of the count while a file is copied.
const EVERY: Duration = Duration::from_secs(1);

/// Follows one download's copy home.
pub(crate) struct Tracker<'a> {
    pool: &'a SqlitePool,
    id: i64,
    /// Every planned file's size, added up.
    total: u64,
    /// The bytes of the files before the one being copied.
    before: u64,
    /// When this run began, and how much was already there then (earlier runs' part files and
    /// placed files), so the speed counts only what this run copied.
    started: Instant,
    at_start: Option<u64>,
    /// When the count was last written.
    written: Option<Instant>,
}

impl<'a> Tracker<'a> {
    /// A tracker for download `id`, copying these files.
    pub(crate) fn new(pool: &'a SqlitePool, id: i64, files: &[Planned]) -> Self {
        Tracker {
            pool,
            id,
            total: files.iter().map(|f| f.size).sum(),
            before: 0,
            started: Instant::now(),
            at_start: None,
            written: None,
        }
    }

    /// A file of `size` bytes is done (copied, or placed by an earlier run): the next starts
    /// after it.
    pub(crate) async fn file_done(&mut self, size: u64) -> Result<(), Stop> {
        self.before += size;
        self.write(0).await
    }

    /// The file being copied has `done` bytes so far: written when it's the first word on it
    /// (`now`) or a second has passed since the last.
    pub(crate) async fn copying(&mut self, done: u64, now: bool) -> Result<(), Stop> {
        if !now && self.written.is_some_and(|at| at.elapsed() < EVERY) {
            return Ok(());
        }
        self.write(done).await
    }

    async fn write(&mut self, done: u64) -> Result<(), Stop> {
        let copied = self.before + done;
        let at_start = *self.at_start.get_or_insert(copied);
        let elapsed = self.started.elapsed().as_secs_f64();
        // Bytes a second this run, once it has run a second.
        let rate = (elapsed >= 1.0 && copied > at_start)
            .then(|| ((copied - at_start) as f64 / elapsed) as i64);
        sqlx::query(
            "UPDATE downloads SET copied = ?, copy_total = ?, copy_rate = ?
             WHERE id = ? AND state = 'transferring'",
        )
        .bind(copied as i64)
        .bind(self.total as i64)
        .bind(rate)
        .bind(self.id)
        .execute(self.pool)
        .await
        .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
        self.written = Some(Instant::now());
        Ok(())
    }
}
