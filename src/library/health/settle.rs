//! File work that has to be recorded once it's done (a file moved to the trash, a trashed file
//! deleted), run so that a share that stops answering holds up neither the caller nor the
//! database: the file work runs first, off the async threads, and only then the short write that
//! records it. Waited on for a time limit; past it the library counts as not responding and the
//! caller moves on, while the work and its record finish on their own whenever the share
//! answers. No database connection is held while the file work runs.

use std::path::Path;
use std::time::Duration;

use super::{Down, down_at, mark_at};
use crate::db::unix_now;

/// How file work and its record went.
#[derive(Debug)]
pub enum Settled<R> {
    /// Done in time, and recorded: what the record answered.
    Done(R),
    /// Not started: the library isn't responding.
    Refused(Down),
    /// Started but not back within the limit. The library now counts as not responding; the
    /// work is recorded once it comes back.
    StillOut(Down),
}

/// Runs `work` (blocking file work in the library `path` is in) on a thread of its own, then
/// `record` with what it answered (a short database write), as a task of its own. Refused at
/// once while that library isn't responding. Waits `limit` for the file work; once it's back,
/// waits for the record however long the database takes, since that says nothing about the
/// share.
pub async fn settle<T, R, F, Fut>(
    path: &Path,
    limit: Duration,
    work: impl FnOnce() -> T + Send + 'static,
    record: F,
) -> Settled<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: FnOnce(T) -> Fut + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
{
    if let Some(down) = down_at(path) {
        return Settled::Refused(down);
    }
    let (back, came_back) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let done = match tokio::task::spawn_blocking(work).await {
            Ok(done) => done,
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            // The runtime is shutting down: nothing to record.
            Err(_) => return None,
        };
        let _ = back.send(());
        Some(record(done).await)
    });
    if tokio::time::timeout(limit, came_back).await.is_err() {
        let why = format!(
            "a file operation in its folder didn't finish within {} seconds",
            limit.as_secs()
        );
        tracing::warn!(
            path = %path.display(),
            seconds = limit.as_secs(),
            "a file operation is stuck; it's recorded once it finishes"
        );
        return Settled::StillOut(mark_at(path, &why).unwrap_or_else(|| Down {
            id: 0,
            name: path.display().to_string(),
            since: unix_now(),
            why,
        }));
    }
    match task.await {
        Ok(Some(recorded)) => Settled::Done(recorded),
        Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
        Ok(None) | Err(_) => Settled::StillOut(Down {
            id: 0,
            name: path.display().to_string(),
            since: unix_now(),
            why: "Melete is stopping".into(),
        }),
    }
}

#[cfg(test)]
mod tests;
