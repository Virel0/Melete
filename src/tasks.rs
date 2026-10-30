//! The background tasks as an admin sees them: each one's last start and end, how it went, when
//! it runs next, and running one now where that's safe. Kept in memory since this server
//! started; a task that hasn't run since isn't running here (its service is off).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::db::unix_now;
use crate::jobs;

/// A background task: its name, what it does, and how it's asked to run now (none when it
/// can't be run by hand safely, or has its own way).
pub struct Task {
    /// Its name in the API.
    pub name: &'static str,
    /// What it does, in plain words.
    pub what: &'static str,
    /// What asks it to run now.
    pub wake: Option<fn()>,
}

/// Every background task, in the order the System page lists them.
pub const TASKS: [Task; 16] = [
    Task {
        name: "library",
        what: "scans the libraries on their schedule and imported folders at once, probes files, deals with replaced files and the trash, and runs TMDB matching (run now: a whole scan)",
        wake: Some(jobs::request_rescan),
    },
    Task {
        name: "tmdb-matching",
        what: "matches new titles on TMDB and refreshes the details, ratings, cast and trailers it has for them, after each scan (run now: without a scan)",
        wake: Some(|| jobs::TMDB.notify_one()),
    },
    Task {
        name: "music-matching",
        what: "matches new albums on MusicBrainz",
        wake: Some(|| jobs::MUSIC.notify_one()),
    },
    Task {
        name: "music-analysis",
        what: "measures tracks' loudness, tempo and sections for Aoide",
        wake: Some(|| jobs::ANALYSIS.notify_one()),
    },
    Task {
        name: "segments",
        what: "finds episodes' intros and credits, and movies' credits",
        wake: Some(|| jobs::SEGMENTS.notify_one()),
    },
    Task {
        name: "trickplay",
        what: "makes seek-bar previews from files' keyframes",
        wake: Some(|| jobs::TRICKPLAY.notify_one()),
    },
    Task {
        name: "import-subtitles",
        what: "fetches from OpenSubtitles the subtitles new imports lack, in the languages set for them",
        wake: Some(|| jobs::IMPORT_SUBTITLES.notify_one()),
    },
    Task {
        name: "works",
        what: "works out composers and works of classical music",
        wake: Some(|| jobs::WORKS.notify_one()),
    },
    Task {
        name: "lyrics",
        what: "reads lyrics beside tracks and in their tags",
        wake: Some(|| jobs::LYRICS.notify_one()),
    },
    Task {
        name: "cover-colours",
        what: "works out album covers' colours",
        wake: Some(|| jobs::COVERS.notify_one()),
    },
    Task {
        name: "following",
        what: "looks for followed artists' new albums on MusicBrainz",
        wake: Some(|| jobs::FOLLOW.notify_one()),
    },
    Task {
        name: "rss-sync",
        what: "asks every indexer for its newest releases and grabs what's wanted (run now: Sync RSS)",
        wake: None,
    },
    Task {
        name: "import-lists",
        what: "syncs the import lists (run now: each list's sync)",
        wake: None,
    },
    Task {
        name: "offline-copies",
        what: "makes the offline copies asked for, while they're turned on, and deletes those whose time is up",
        wake: None,
    },
    Task {
        name: "gpu-check",
        what: "tests the GPU encoder, at startup and after a run failed on it, falling back to the CPU while it fails",
        wake: None,
    },
    Task {
        name: "backup",
        what: "copies the database once a day (run now: `melete backup` on the server)",
        wake: None,
    },
];

/// How a task has gone since this server started.
#[derive(Debug, Clone, Default)]
struct Runs {
    running: bool,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    result: Option<String>,
    next_at: Option<i64>,
    progress: Vec<Progress>,
}

/// How far one step of a running task has got, such as probing files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// The step, in plain words.
    pub what: &'static str,
    /// Items done so far.
    pub done: usize,
    /// Items in this run.
    pub total: usize,
    /// Of those done, the ones that failed.
    pub failed: usize,
}

static RUNS: LazyLock<Mutex<HashMap<&'static str, Runs>>> = LazyLock::new(Default::default);

fn runs() -> std::sync::MutexGuard<'static, HashMap<&'static str, Runs>> {
    // A panic while held leaves nothing half-written worth refusing.
    RUNS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Notes a task waiting for its first run, `next_in` from now.
pub fn scheduled(name: &'static str, next_in: Duration) {
    let mut all = runs();
    let task = all.entry(name).or_default();
    task.next_at = Some(unix_now() + next_in.as_secs() as i64);
}

/// What a failed run says: never the error itself, which may name an address; the server's
/// log has it.
pub fn failed() -> Option<String> {
    Some("failed; the server's log says why".into())
}

/// Notes a task that runs in this server but only when asked, so it's shown as ready before its
/// first run. What's known of earlier runs is kept.
pub fn ready(name: &'static str) {
    runs().entry(name).or_default();
}

/// Notes a task that no longer runs in this server, as when its service was turned off.
pub fn inactive(name: &'static str) {
    runs().remove(name);
}

/// Notes a task starting.
pub fn started(name: &'static str) {
    let mut all = runs();
    let task = all.entry(name).or_default();
    task.running = true;
    task.started_at = Some(unix_now());
    task.next_at = None;
}

/// Notes a run of a task once it's done, from `started_at`, and how it went: for a task that
/// looks often and only sometimes has something to do.
pub fn ran(name: &'static str, started_at: i64, result: String) {
    let mut all = runs();
    let task = all.entry(name).or_default();
    task.started_at = Some(started_at);
    task.finished_at = Some(unix_now());
    task.result = Some(result);
}

/// Notes how far the step `what` of a running task has got, or with `None` that it's over. A
/// task this server hasn't started isn't shown running for it.
pub fn progress(name: &'static str, what: &'static str, now: Option<Progress>) {
    let mut all = runs();
    if let Some(task) = all.get_mut(name) {
        task.progress.retain(|p| p.what != what);
        task.progress.extend(now);
    }
}

/// Notes a task finishing, how it went (`None` for fine), and when it runs next on its own.
pub fn finished(name: &'static str, result: Option<String>, next_in: Option<Duration>) {
    let now = unix_now();
    let mut all = runs();
    let task = all.entry(name).or_default();
    task.running = false;
    task.finished_at = Some(now);
    task.result = Some(result.unwrap_or_else(|| "done".into()));
    task.next_at = next_in.map(|d| now + d.as_secs() as i64);
}

/// A task as the API shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskView {
    /// Its name.
    pub name: &'static str,
    /// What it does.
    pub what: &'static str,
    /// Whether it's running now.
    pub running: bool,
    /// Whether it runs in this server; false when its service is off.
    pub active: bool,
    /// When its last run started, in Unix seconds.
    pub last_started_at: Option<i64>,
    /// When its last run ended.
    pub last_finished_at: Option<i64>,
    /// How the last run went, in plain words.
    pub result: Option<String>,
    /// When it runs next on its own, when it has a schedule.
    pub next_at: Option<i64>,
    /// Whether it can be run now from here.
    pub can_run: bool,
    /// How far each of its steps running now has got; empty when none is.
    pub progress: Vec<Progress>,
}

/// Every task, in [`TASKS`]' order.
pub fn list() -> Vec<TaskView> {
    let all = runs();
    TASKS
        .iter()
        .map(|task| {
            let runs = all.get(task.name).cloned().unwrap_or_default();
            TaskView {
                name: task.name,
                what: task.what,
                running: runs.running,
                active: all.contains_key(task.name),
                last_started_at: runs.started_at,
                last_finished_at: runs.finished_at,
                result: runs.result,
                next_at: runs.next_at,
                can_run: task.wake.is_some(),
                progress: runs.progress,
            }
        })
        .collect()
}

/// Why a task wasn't run now.
#[derive(Debug, PartialEq, Eq)]
pub enum NotRun {
    NotFound,
    /// It can't be run from here (it has its own way, or none).
    NotByHand,
    /// It isn't running in this server: its service is off.
    Inactive,
    /// It's running now; a second run at once isn't started.
    Running,
}

/// Runs a task now: wakes it, so it runs once more as soon as it can.
pub fn run_now(name: &str) -> Result<(), NotRun> {
    let task = TASKS
        .iter()
        .find(|t| t.name == name)
        .ok_or(NotRun::NotFound)?;
    let wake = task.wake.ok_or(NotRun::NotByHand)?;
    let all = runs();
    match all.get(task.name) {
        None => Err(NotRun::Inactive),
        Some(runs) if runs.running => Err(NotRun::Running),
        Some(_) => {
            wake();
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
