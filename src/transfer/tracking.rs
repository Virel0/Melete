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
    /// When this run began, and the bytes it has copied itself, of the files done; the speed
    /// counts only these, not what earlier runs left.
    started: Instant,
    fresh: u64,
    /// Where this run took up the file being copied: its part file's length.
    offset: Option<u64>,
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
            fresh: 0,
            offset: None,
            written: None,
        }
    }

    /// A file of `size` bytes is done (copied, or placed by an earlier run): the next starts
    /// after it.
    pub(crate) async fn file_done(&mut self, size: u64) -> Result<(), Stop> {
        if let Some(offset) = self.offset.take() {
            self.fresh += size.saturating_sub(offset);
        }
        self.before += size;
        self.write(0).await
    }

    /// The file being copied has `done` bytes so far: written when it's the first word on it
    /// (`now`) or a second has passed since the last.
    pub(crate) async fn copying(&mut self, done: u64, now: bool) -> Result<(), Stop> {
        if now {
            self.offset = Some(done);
        }
        if !now && self.written.is_some_and(|at| at.elapsed() < EVERY) {
            return Ok(());
        }
        self.write(done).await
    }

    async fn write(&mut self, done: u64) -> Result<(), Stop> {
        let copied = self.before + done;
        let fresh = self.fresh + self.offset.map_or(0, |offset| done.saturating_sub(offset));
        let elapsed = self.started.elapsed().as_secs_f64();
        // Bytes a second this run, once it has run a second.
        let rate = (elapsed >= 1.0 && fresh > 0).then(|| (fresh as f64 / elapsed) as i64);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_speed_counts_only_what_this_run_copied() {
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO downloads (id, infohash, title, decision, state, added_at, updated_at)
             VALUES (1, 'a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'x', '{}', 'transferring', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let file = |name: &str, size| Planned {
            remote: format!("/Completed/{name}"),
            size,
            folder: String::new(),
            name: name.into(),
        };
        let files = [file("a.mkv", 1_000), file("b.mkv", 3_000)];
        let mut tracker = Tracker::new(&pool, 1, &files);
        tracker.started = Instant::now() - Duration::from_secs(2);
        // The first was placed by an earlier run, the second half copied by it.
        tracker.file_done(1_000).await.unwrap();
        tracker.copying(1_000, true).await.unwrap();
        tracker.written = None;
        tracker.copying(3_000, false).await.unwrap();
        let written: (i64, i64, Option<i64>) =
            sqlx::query_as("SELECT copied, copy_total, copy_rate FROM downloads WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        // 4,000 of 4,000 bytes, 2,000 of them copied by this run in about two seconds.
        assert_eq!((written.0, written.1), (4_000, 4_000));
        let rate = written.2.unwrap();
        assert!((900..=1_000).contains(&rate), "{rate}");
    }
}
