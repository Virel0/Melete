//! Whether each library's folder answers. A frozen or unmounted share makes every read of it
//! wait with no end, so each library's folder is read regularly, on a thread of its own and
//! with a time limit. One that doesn't answer in time, can't be read, or is empty (an unmounted
//! share looks empty) is marked as not responding: scans leave it alone, copies into it wait,
//! players are told so rather than left waiting, and System says so, until it answers again.
//!
//! What's known lives here, for every part of Melete to ask without touching the disk, and in
//! `libraries.unreachable_since`, so a restart starts from it. The database is written only
//! after a read has come back or given up, never while the folder is being read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use sqlx::SqlitePool;

use super::Library;
use crate::admin_health::{Finding, Level};
use crate::db::unix_now;

mod settle;
pub use settle::*;

/// How often each library's folder is read.
pub const EVERY: Duration = Duration::from_secs(60);
/// How long a read of a library's folder may take before the library counts as not responding.
pub const LIMIT: Duration = Duration::from_secs(15);

/// A library that isn't responding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Down {
    pub id: i64,
    /// Its name, from `MELETE_LIBRARY_<NAME>`.
    pub name: String,
    /// When it stopped answering (Unix seconds).
    pub since: i64,
    /// Why it counts as not responding, in plain words.
    pub why: String,
}

impl Down {
    /// "the movies library isn't responding", for players and messages.
    pub fn saying(&self) -> String {
        format!("the {} library isn't responding", self.name)
    }
}

#[derive(Default)]
struct Known {
    /// Every library, by its folder.
    libraries: HashMap<PathBuf, (i64, String)>,
    /// The ones not responding, by folder.
    down: HashMap<PathBuf, Down>,
}

static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);
/// Folders whose read hasn't come back yet, stuck ones included: never read again until then,
/// so a frozen share ties up one thread, not one a minute.
static READING: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Mutex::default);

fn known() -> std::sync::MutexGuard<'static, Known> {
    KNOWN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The library `path` is in (its folder or anything under it), when it isn't responding.
pub fn down_at(path: &Path) -> Option<Down> {
    known()
        .down
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.as_os_str().len())
        .map(|(_, down)| down.clone())
}

/// Every library not responding, by name.
pub fn all_down() -> Vec<Down> {
    let mut down: Vec<Down> = known().down.values().cloned().collect();
    down.sort_by(|a, b| a.name.cmp(&b.name));
    down
}

/// Counts the library `path` is in as not responding from now, for `why`, when something else
/// found it doesn't answer (a copy into it that timed out); the next read clears it if it does.
pub fn mark_at(path: &Path, why: &str) -> Option<Down> {
    let mut known = known();
    let (root, (id, name)) = known
        .libraries
        .iter()
        .filter(|(root, _)| path.starts_with(root))
        .max_by_key(|(root, _)| root.as_os_str().len())
        .map(|(root, library)| (root.clone(), library.clone()))?;
    let down = known.down.entry(root).or_insert_with(|| Down {
        id,
        name,
        since: unix_now(),
        why: why.to_owned(),
    });
    Some(down.clone())
}

/// Runs file work for the library `path` is in, off the async threads and given up on after
/// `limit`. Refused at once while that library isn't responding, and a run that takes longer
/// counts it as not responding: either way the answer is why. The work itself, if stuck, is
/// left on its thread.
pub async fn within<T: Send + 'static>(
    path: &Path,
    limit: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Down> {
    if let Some(down) = down_at(path) {
        return Err(down);
    }
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(done)) => Ok(done),
        Ok(Err(err)) => std::panic::resume_unwind(err.into_panic()),
        Err(_) => {
            let why = format!(
                "its folder didn't answer within {} seconds",
                limit.as_secs()
            );
            Err(mark_at(path, &why).unwrap_or_else(|| Down {
                id: 0,
                name: path.display().to_string(),
                since: unix_now(),
                why,
            }))
        }
    }
}

/// Whether `file` opens, asked off the async threads with [`LIMIT`]: refused at once while its
/// library isn't responding, and a file that doesn't open in time counts it as not responding.
/// Why not, when it doesn't.
pub async fn opens(file: &Path) -> Result<(), String> {
    let at = file.to_path_buf();
    match within(file, LIMIT, move || std::fs::File::open(at).map(drop)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(down) => Err(format!("{}: {}", down.saying(), down.why)),
    }
}

/// Waits for `work` (a walk of the library at `root`, say), giving up once that library is
/// counted as not responding.
pub async fn unless_down<T>(root: &Path, work: impl Future<Output = T>) -> anyhow::Result<T> {
    let mut work = std::pin::pin!(work);
    loop {
        tokio::select! {
            done = &mut work => return Ok(done),
            () = tokio::time::sleep(Duration::from_secs(5)) => {
                if let Some(down) = down_at(root) {
                    anyhow::bail!("{}: {}", down.saying(), down.why);
                }
            }
        }
    }
}

/// Reads the folder: answered when it has something in it.
fn read(root: &Path) -> Result<(), String> {
    let mut entries =
        std::fs::read_dir(root).map_err(|err| format!("its folder can't be read: {err}"))?;
    match entries.next() {
        Some(Ok(_)) => Ok(()),
        Some(Err(err)) => Err(format!("its folder can't be read: {err}")),
        None => Err("its folder is empty; is the share mounted?".into()),
    }
}

/// Reads `root` with `read` on a thread of its own, giving up after `limit`. A read from before
/// that hasn't come back yet isn't started again.
async fn answers(
    root: &Path,
    limit: Duration,
    read: fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if !READING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(root.to_path_buf())
    {
        return Err("an earlier read of its folder still hasn't come back".into());
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let path = root.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("library-check".into())
        .spawn(move || {
            let read = read(&path);
            READING
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&path);
            let _ = tx.send(read);
        });
    if let Err(err) = spawned {
        READING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(root);
        return Err(format!("couldn't start reading its folder: {err}"));
    }
    match tokio::time::timeout(limit, rx).await {
        Ok(Ok(read)) => read,
        Ok(Err(_)) => Err("the read of its folder stopped".into()),
        Err(_) => Err(format!(
            "its folder didn't answer within {} seconds",
            limit.as_secs()
        )),
    }
}

/// A library that stopped or started answering at the latest check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Stopped(Down),
    /// Answers again after being down since `since`.
    Back {
        name: String,
        since: i64,
    },
}

/// Takes in the libraries and what the database says of them: one not responding when Melete
/// stopped still counts as not responding until it answers.
pub async fn load(pool: &SqlitePool, libraries: &[Library]) -> sqlx::Result<()> {
    let rows: Vec<(i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, unreachable_since, unreachable_why FROM libraries
         WHERE unreachable_since IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut known = known();
    for library in libraries {
        known
            .libraries
            .insert(library.path.clone(), (library.id, library.name.clone()));
        if let Some((_, since, why)) = rows.iter().find(|r| r.0 == library.id) {
            known.down.insert(
                library.path.clone(),
                Down {
                    id: library.id,
                    name: library.name.clone(),
                    since: *since,
                    why: why.clone().unwrap_or_default(),
                },
            );
        }
    }
    Ok(())
}

/// Takes in how a read of the library's folder went; answers what changed. The lock is let go
/// before anything is written to the database.
fn note(library: &Library, read: Result<(), String>, now: i64) -> Option<Change> {
    let mut known = known();
    let Err(why) = read else {
        return known.down.remove(&library.path).map(|was| Change::Back {
            name: library.name.clone(),
            since: was.since,
        });
    };
    let new = !known.down.contains_key(&library.path);
    let down = known
        .down
        .entry(library.path.clone())
        .or_insert_with(|| Down {
            id: library.id,
            name: library.name.clone(),
            since: now,
            why: why.clone(),
        });
    down.why = why;
    new.then(|| Change::Stopped(down.clone()))
}

/// Reads every library's folder once, each at the same time, records what changed and
/// answers it.
pub async fn check(
    pool: &SqlitePool,
    libraries: &[Library],
    limit: Duration,
) -> sqlx::Result<Vec<Change>> {
    check_with(pool, libraries, limit, read).await
}

async fn check_with(
    pool: &SqlitePool,
    libraries: &[Library],
    limit: Duration,
    read: fn(&Path) -> Result<(), String>,
) -> sqlx::Result<Vec<Change>> {
    let reads = futures_util::future::join_all(
        libraries
            .iter()
            .map(|library| async move { (library, answers(&library.path, limit, read).await) }),
    )
    .await;
    let now = unix_now();
    let mut changes = Vec::new();
    for (library, read) in reads {
        let change = note(library, read, now);
        let down = down_at(&library.path);
        sqlx::query("UPDATE libraries SET unreachable_since = ?, unreachable_why = ? WHERE id = ?")
            .bind(down.as_ref().map(|d| d.since))
            .bind(down.as_ref().map(|d| d.why.clone()))
            .bind(library.id)
            .execute(pool)
            .await?;
        match &change {
            Some(Change::Stopped(down)) => tracing::warn!(
                library = down.name,
                why = down.why,
                "a library isn't responding; nothing reads it until it does"
            ),
            Some(Change::Back { name, since }) => tracing::info!(
                library = name,
                minutes_down = (now - since).max(0) / 60,
                "a library answers again"
            ),
            None => {}
        }
        changes.extend(change);
    }
    Ok(changes)
}

/// Reads every library's folder each minute for as long as the server runs, and tells admins
/// when one stops or starts answering (`alerts`).
pub async fn run(pool: SqlitePool, libraries: Vec<Library>) {
    loop {
        match check(&pool, &libraries, LIMIT).await {
            // Admins are told, without holding up the next check.
            Ok(changes) => {
                for change in changes {
                    let (pool, alert) = (
                        pool.clone(),
                        crate::alerts::Alert::library(&change, unix_now()),
                    );
                    tokio::spawn(async move { crate::alerts::send(&pool, &alert).await });
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "couldn't record whether the libraries answer")
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// System's problem for each of the database's libraries not responding (by its folder).
pub async fn findings(pool: &SqlitePool) -> sqlx::Result<Vec<Finding>> {
    let folders: Vec<String> = sqlx::query_scalar("SELECT path FROM libraries")
        .fetch_all(pool)
        .await?;
    let now = unix_now();
    Ok(all_down()
        .into_iter()
        .filter(|down| {
            folders
                .iter()
                .any(|f| down_at(Path::new(f)).is_some_and(|d| d.id == down.id))
        })
        .map(|down| Finding {
            level: Level::Problem,
            code: "library_unreachable",
            message: format!(
                "The {} library hasn't responded for {} min: {}. Scans leave it alone, copies into it wait, and players say it isn't responding, until it answers again.",
                down.name,
                (now - down.since).max(0) / 60,
                down.why
            ),
            count: 0,
            id: Some(down.id),
        })
        .collect())
}

#[cfg(test)]
mod tests;
