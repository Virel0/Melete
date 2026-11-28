//! The library's housekeeping schedule: scans, TMDB matching and probing run at
//! startup, then on the scan schedule, and sooner when something asks (an import, an admin).

use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use futures_util::FutureExt;

use tokio::sync::Notify;

pub mod pace;
pub mod scans;

/// How often the loops woken after a scan run when nothing wakes them sooner.
pub const EVERY: Duration = Duration::from_secs(6 * 3600);

/// How often the housekeeping looks whether a whole scan is due by the scan schedule, when
/// nothing asks sooner; a look with nothing to do does nothing.
pub const TICK: Duration = Duration::from_secs(60);

/// Asks for a run soon. Requests made while one is running add up to one more run.
pub static RESCAN: Notify = Notify::const_new();

/// Wakes music matching (after a scan found albums to match).
pub static MUSIC: Notify = Notify::const_new();
/// Wakes the measuring of music tracks' audio (after a scan).
pub static ANALYSIS: Notify = Notify::const_new();
/// Wakes the working out of album covers' colours (after a scan, or a cover fetched).
pub static COVERS: Notify = Notify::const_new();
/// Wakes the working out of composers and works, and of tracks' artists (after a scan, or a
/// match).
pub static WORKS: Notify = Notify::const_new();
/// Wakes the reading of lyrics beside tracks and in their tags (after a scan).
pub static LYRICS: Notify = Notify::const_new();
/// Wakes the search for episodes' intros and credits (after a scan and probe).
pub static SEGMENTS: Notify = Notify::const_new();
/// Wakes the making of seek-bar previews (after a scan and probe).
pub static TRICKPLAY: Notify = Notify::const_new();
/// Wakes the fetching of subtitles new imports lack (after a scan and probe).
pub static IMPORT_SUBTITLES: Notify = Notify::const_new();

/// Wakes TMDB matching on its own, without a scan (an admin running it from Tasks).
pub static TMDB: Notify = Notify::const_new();

/// Wakes the look for followed artists' new albums.
pub static FOLLOW: Notify = Notify::const_new();

/// Wakes the lookup of library artists' portraits (an admin running it from Tasks).
pub static PORTRAITS: Notify = Notify::const_new();

/// Wakes the making of For You lists (someone's is due, or an admin running it from Tasks).
pub static FOR_YOU: Notify = Notify::const_new();

/// Wakes the library scan now rather than at its next run, every library whole.
pub fn request_rescan() {
    scans::ask_whole();
    RESCAN.notify_one();
}

/// Wakes a scan of one library now, whole or only one title folder (its path in the library);
/// returns the request's number, which [`scans::finished_with`] waits on.
pub fn request_folder_scan(library_id: i64, folder: Option<String>) -> u64 {
    let ticket = scans::ask_folder(library_id, folder);
    RESCAN.notify_one();
    ticket
}

/// Runs `job` now, then after each `interval` or as soon as `asked` is notified, whichever
/// comes first. Runs never overlap.
pub async fn repeat<F, Fut>(interval: Duration, asked: &Notify, mut job: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    loop {
        job().await;
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = asked.notified() => {}
        }
    }
}

/// [`repeat`], with each run noted as the task `name`'s (see [`crate::tasks`]): when it started
/// and ended, and that it runs again after `interval`.
pub async fn repeat_task<F, Fut>(name: &'static str, interval: Duration, asked: &Notify, mut job: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    repeat(interval, asked, || {
        let run = job();
        async move {
            crate::tasks::started(name);
            run.await;
            crate::tasks::finished(name, None, Some(interval));
        }
    })
    .await;
}

/// When this server started, for the tasks held back after it ([`after_start`]).
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Notes the server starting, so the holds after it count from now.
pub fn note_start() {
    LazyLock::force(&STARTED);
}

/// What's left of `hold` since the server started: all of it at the start, none once it's past.
pub fn after_start(hold: Duration) -> Duration {
    hold.saturating_sub(STARTED.elapsed())
}

/// How long after the server starts its tasks that ask MusicBrainz begin, each its own, so
/// they don't all ask at once while the server comes up (MusicBrainz answered "busy").
pub mod hold {
    use std::time::Duration;

    /// Music matching.
    pub const MUSIC_MATCHING: Duration = Duration::from_secs(2 * 60);
    /// Playlist imports left queued or running.
    pub const PLAYLIST_IMPORTS: Duration = Duration::from_secs(3 * 60);
    /// The look for followed artists' new albums.
    pub const FOLLOWING: Duration = Duration::from_secs(4 * 60);
    /// Library artists' portraits, which ask Wikidata and Commons too (Wikidata asked for a
    /// pause when asked in the first seconds).
    pub const PORTRAITS: Duration = Duration::from_secs(6 * 60);
}

/// [`repeat_task`], its first run held back until `hold` after the server started
/// ([`after_start`]). Asks while held, as a scan's at start, are answered by that first run.
pub async fn repeat_task_after<F, Fut>(
    name: &'static str,
    hold: Duration,
    interval: Duration,
    asked: &Notify,
    job: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    held(name, after_start(hold), asked).await;
    repeat_task(name, interval, asked, job).await;
}

/// Waits `left`, noting the task `name` as due then; an ask meanwhile is let go, since the run
/// after the wait answers it.
async fn held(name: &'static str, left: Duration, asked: &Notify) {
    if left.is_zero() {
        return;
    }
    crate::tasks::scheduled(name, left);
    tokio::time::sleep(left).await;
    let _ = asked.notified().now_or_never();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn runs_now_on_schedule_and_when_asked() {
        let asked = Arc::new(Notify::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let (n, notify) = (runs.clone(), asked.clone());
        tokio::spawn(async move {
            repeat(Duration::from_millis(600), &notify, || {
                let n = n.clone();
                async move {
                    n.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await
        });
        let settle = || tokio::time::sleep(Duration::from_millis(50));
        settle().await;
        assert_eq!(runs.load(Ordering::SeqCst), 1, "at once");
        asked.notify_one();
        settle().await;
        assert_eq!(runs.load(Ordering::SeqCst), 2, "when asked");
        // A burst of requests: one wakes a run, the rest add up to one more after it.
        asked.notify_one();
        asked.notify_one();
        asked.notify_one();
        settle().await;
        settle().await;
        assert_eq!(runs.load(Ordering::SeqCst), 4);
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 5, "on schedule");
    }

    #[tokio::test]
    async fn a_held_task_waits_and_its_first_run_answers_asks_meanwhile() {
        let asked = Arc::new(Notify::new());
        let runs = Arc::new(AtomicUsize::new(0));
        let (n, notify) = (runs.clone(), asked.clone());
        tokio::spawn(async move {
            held("test-held", Duration::from_millis(400), &notify).await;
            repeat(Duration::from_secs(3600), &notify, || {
                let n = n.clone();
                async move {
                    n.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        asked.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 0, "held, even when asked");
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1, "once, for the ask too");
        asked.notify_one();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 2, "asked after");
    }

    #[test]
    fn a_hold_counts_from_the_start() {
        note_start();
        let hour = Duration::from_secs(3600);
        let left = after_start(hour);
        assert!(left <= hour && left + STARTED.elapsed() >= hour, "{left:?}");
        assert_eq!(after_start(Duration::ZERO), Duration::ZERO);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(
            after_start(Duration::from_millis(10)),
            Duration::ZERO,
            "past"
        );
    }
}
