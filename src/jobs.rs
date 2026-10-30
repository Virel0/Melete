//! The library's housekeeping schedule: scans, TMDB matching and probing run at
//! startup, then on the scan schedule, and sooner when something asks (an import, an admin).

use std::future::Future;
use std::time::Duration;

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

/// Wakes the look for followed artists' new albums.
pub static FOLLOW: Notify = Notify::const_new();

/// Wakes the library scan now rather than at its next run, every library whole.
pub fn request_rescan() {
    scans::ask_whole();
    RESCAN.notify_one();
}

/// Wakes a scan of one library now, whole or only one title folder (its path in the library).
pub fn request_folder_scan(library_id: i64, folder: Option<String>) {
    scans::ask_folder(library_id, folder);
    RESCAN.notify_one();
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
}
