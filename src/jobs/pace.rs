//! How a long background job is getting on. While it runs, a line in the log every
//! [`EVERY_ITEMS`] items or every [`EVERY`], whichever comes first, says how many are done of how
//! many, how many failed so far, and about how long is left; a line is due on time even when no
//! item has finished, so a slow job and a stuck one can be told apart. The admin Tasks page shows
//! the same counts. Each job still ends with its own closing line; a run that ends before its
//! last item (an error, the server stopping, the job turned off) says so in one line of its own.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Items between progress lines, at most.
pub const EVERY_ITEMS: usize = 250;
/// Time between progress lines, at most.
pub const EVERY: Duration = Duration::from_secs(30);

/// One job's counts, while it runs.
#[derive(Debug)]
struct State {
    task: &'static str,
    what: &'static str,
    total: usize,
    done: usize,
    failed: usize,
    started: Instant,
    /// When the last line was due, and how many were done then.
    last_line: (Instant, usize),
}

impl State {
    /// Whether a progress line is due: never once every item is done (the job's closing line
    /// says the rest).
    fn due(&self, now: Instant) -> bool {
        self.done < self.total
            && (self.done - self.last_line.1 >= EVERY_ITEMS
                || now.duration_since(self.last_line.0) >= EVERY)
    }

    /// About how long is left at the pace so far; none before the first item is done.
    fn left(&self, now: Instant) -> Option<Duration> {
        let elapsed = now.duration_since(self.started);
        let rest = u32::try_from(self.total.saturating_sub(self.done)).ok()?;
        let done = u32::try_from(self.done).ok().filter(|d| *d > 0)?;
        Some(elapsed / done * rest)
    }

    fn log_if_due(&mut self, now: Instant) {
        if !self.due(now) {
            return;
        }
        self.last_line = (now, self.done);
        let left = self.left(now).map_or("not known yet".to_owned(), about);
        tracing::info!(
            done = self.done,
            total = self.total,
            failed = self.failed,
            left,
            "{} under way",
            self.what
        );
    }

    fn publish(&self) {
        crate::tasks::progress(
            self.task,
            self.what,
            Some(crate::tasks::Progress {
                what: self.what,
                done: self.done,
                total: self.total,
                failed: self.failed,
            }),
        );
    }
}

/// A length of time in round words: "about 2h 5m", "about 12m", "under a minute".
fn about(left: Duration) -> String {
    if left < Duration::from_secs(60) {
        return "under a minute".to_owned();
    }
    match left.as_secs().div_ceil(60) {
        m if m < 60 => format!("about {m}m"),
        m => format!("about {}h {}m", m / 60, m % 60),
    }
}

/// A job's progress, from its start to its end. Each item is counted when its [`Item`] is
/// dropped, so an item left early (`continue`) still counts.
pub struct Pace {
    state: Arc<Mutex<State>>,
    ticker: Option<tokio::task::JoinHandle<()>>,
    finished: bool,
}

impl Pace {
    /// Starts counting `total` items of the background task `task` (as the Tasks page names it),
    /// in words `what` ("probing files"). Needs a Tokio runtime: the line due on time is its.
    pub fn start(task: &'static str, what: &'static str, total: usize) -> Pace {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State {
            task,
            what,
            total,
            done: 0,
            failed: 0,
            started: now,
            last_line: (now, 0),
        }));
        let ticker = (total > 0).then(|| {
            let state = state.clone();
            tokio::spawn(async move {
                let mut every = tokio::time::interval_at((now + EVERY).into(), EVERY);
                loop {
                    every.tick().await;
                    lock(&state).log_if_due(Instant::now());
                }
            })
        });
        if total > 0 {
            lock(&state).publish();
        }
        Pace {
            state,
            ticker,
            finished: false,
        }
    }

    /// One item begun; it counts as done once dropped.
    pub fn item(&self) -> Item<'_> {
        Item {
            pace: self,
            failed: false,
        }
    }

    fn count(&self, failed: bool) {
        let mut state = lock(&self.state);
        state.done += 1;
        state.failed += usize::from(failed);
        state.total = state.total.max(state.done);
        state.log_if_due(Instant::now());
        state.publish();
    }

    /// The job went through every item; its own closing line follows.
    pub fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for Pace {
    fn drop(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            ticker.abort();
        }
        let state = lock(&self.state);
        if !self.finished && state.done < state.total {
            tracing::warn!(
                done = state.done,
                total = state.total,
                failed = state.failed,
                "{} stopped before the end",
                state.what
            );
        }
        crate::tasks::progress(state.task, state.what, None);
    }
}

/// One item of a job, counted as done when dropped.
pub struct Item<'p> {
    pace: &'p Pace,
    failed: bool,
}

impl Item<'_> {
    /// Counts this item as failed.
    pub fn failed(mut self) {
        self.failed = true;
    }
}

impl Drop for Item<'_> {
    fn drop(&mut self) {
        self.pace.count(self.failed);
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // A panic while held leaves counts worth reading still.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::DEFAULT_FILTER;
    use crate::logging::testing::capture;

    fn state(total: usize, done: usize, started: Instant) -> State {
        State {
            task: "library",
            what: "probing files",
            total,
            done,
            failed: 0,
            started,
            last_line: (started, 0),
        }
    }

    #[test]
    fn a_line_is_due_every_few_hundred_items_or_every_half_minute() {
        let start = Instant::now();
        let mut s = state(1000, 10, start);
        assert!(!s.due(start + Duration::from_secs(5)));
        assert!(s.due(start + EVERY), "on time, even with few done");
        s.done = EVERY_ITEMS;
        assert!(s.due(start + Duration::from_secs(5)), "after enough items");
        s.log_if_due(start + Duration::from_secs(5));
        assert!(!s.due(start + Duration::from_secs(6)), "not again at once");
        assert!(s.due(start + Duration::from_secs(5) + EVERY));
        s.done = 1000;
        assert!(
            !s.due(start + Duration::from_secs(600)),
            "never once all are done"
        );
    }

    #[test]
    fn a_line_is_due_on_time_with_nothing_done_so_a_stuck_job_shows() {
        let start = Instant::now();
        let s = state(5, 0, start);
        assert!(s.due(start + EVERY));
        assert_eq!(s.left(start + EVERY), None);
    }

    #[test]
    fn the_time_left_is_at_the_pace_so_far() {
        let start = Instant::now();
        let s = state(400, 100, start);
        assert_eq!(
            s.left(start + Duration::from_secs(60)),
            Some(Duration::from_secs(180))
        );
        assert_eq!(about(Duration::from_secs(180)), "about 3m");
        assert_eq!(about(Duration::from_secs(20)), "under a minute");
        assert_eq!(about(Duration::from_secs(3 * 3600 + 5 * 60)), "about 3h 5m");
    }

    #[tokio::test]
    async fn a_long_run_says_how_it_is_getting_on() {
        let log = capture(DEFAULT_FILTER);
        let pace = Pace::start("backup", "counting a long run", 600);
        for n in 0..300 {
            let item = pace.item();
            if n % 100 == 0 {
                item.failed();
            }
        }
        pace.finish();
        let text = log.written.text();
        assert_eq!(
            text.matches("counting a long run under way").count(),
            1,
            "{text}"
        );
        assert!(text.contains("done=250 total=600 failed=3"), "{text}");
        assert!(text.contains("left="), "{text}");
        assert!(!text.contains("stopped"), "{text}");
    }

    #[tokio::test]
    async fn a_run_that_ends_early_says_so_and_a_whole_one_doesnt() {
        let log = capture(DEFAULT_FILTER);
        {
            let pace = Pace::start("backup", "an early end", 3);
            pace.item().failed();
        }
        {
            let pace = Pace::start("backup", "a whole run", 1);
            drop(pace.item());
            pace.finish();
        }
        let text = log.written.text();
        assert!(
            text.contains("an early end stopped before the end done=1 total=3 failed=1"),
            "{text}"
        );
        assert!(!text.contains("a whole run stopped"), "{text}");
    }

    #[tokio::test]
    async fn the_tasks_page_sees_the_counts_while_it_runs() {
        // Other tests count under this task too, each its own step.
        let shown = || {
            let task = crate::tasks::list()
                .into_iter()
                .find(|t| t.name == "backup")
                .expect("a task");
            task.progress
                .into_iter()
                .filter(|p| p.what == "counting a test's items")
                .collect::<Vec<_>>()
        };
        crate::tasks::started("backup");
        let pace = Pace::start("backup", "counting a test's items", 3);
        pace.item().failed();
        drop(pace.item());
        assert_eq!(
            shown(),
            [crate::tasks::Progress {
                what: "counting a test's items",
                done: 2,
                total: 3,
                failed: 1,
            }]
        );
        drop(pace);
        assert!(shown().is_empty(), "gone once it ends");
    }
}
