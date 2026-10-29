//! Watching the library folders, so a file put there by hand or by another tool is picked up
//! without waiting for the scheduled scan. Off by default (`MELETE_SCAN_WATCH`), and best
//! effort: network and pooled filesystems often deliver no events, and a large library can
//! need more inotify watches than the system allows, so the schedule stays the safety net.
//!
//! A change only asks for a scan of what it touched, once the folder has been quiet for
//! [`QUIET`]: the title folder it's in, or the library whole when a title folder was renamed
//! or moved at the top (so a whole scan can tell the renamed folder is the same title) and for
//! music (whose scans are whole). A finished write, a file or folder moved in or out, and a
//! file deleted count; hidden names (Melete's own unfinished copies among them) don't. Nothing
//! on disk is written.

use std::collections::HashMap;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::{Library, LibraryKind};

mod inotify;

/// How long a library has to be quiet before the scans its changes ask for are asked.
pub const QUIET: Duration = Duration::from_secs(30);
/// How often the setting is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(60);

/// A scan a change asks for: one library, whole or one title folder in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The library's row id.
    pub library_id: i64,
    /// The title folder (or loose file) at the library's top, or None for the whole library.
    pub folder: Option<String>,
}

/// Watches while the setting is on, for as long as Melete runs: looked at once a minute, so
/// turning it on or off needs no restart.
pub async fn run(pool: SqlitePool, libraries: Vec<Library>) {
    let (sink, asks) = unbounded_channel();
    tokio::spawn(gather(asks, QUIET, |ask: Ask| {
        crate::jobs::request_folder_scan(ask.library_id, ask.folder);
    }));
    let mut watching: Option<Watcher> = None;
    loop {
        let on = crate::settings::scan_watch(&pool).await.unwrap_or(false);
        match (on, watching.is_some()) {
            (true, false) => match Watcher::start(&libraries, sink.clone()) {
                Ok(watcher) => {
                    tracing::info!(
                        folders = watcher.folders,
                        "watching the library folders for changes"
                    );
                    watching = Some(watcher);
                }
                Err(err) => tracing::warn!(
                    error = format!("{err:#}"),
                    "couldn't watch the library folders; the scan schedule still finds changes"
                ),
            },
            (false, true) => {
                watching = None;
                tracing::info!("stopped watching the library folders");
            }
            _ => {}
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

/// Collects asks and passes each on once (`ask`), after `quiet` without a new one.
pub(super) async fn gather(
    mut asks: UnboundedReceiver<Ask>,
    quiet: Duration,
    mut ask: impl FnMut(Ask),
) {
    let mut pending: Vec<Ask> = Vec::new();
    loop {
        let next = if pending.is_empty() {
            asks.recv().await
        } else {
            match tokio::time::timeout(quiet, asks.recv()).await {
                Ok(next) => next,
                Err(_) => {
                    pending.drain(..).for_each(&mut ask);
                    continue;
                }
            }
        };
        match next {
            Some(next) if !pending.contains(&next) => pending.push(next),
            Some(_) => {}
            None => {
                pending.drain(..).for_each(&mut ask);
                return;
            }
        }
    }
}

/// The watch, on a thread of its own; stopped when dropped.
pub(super) struct Watcher {
    stop: Arc<AtomicBool>,
    /// How many folders are watched.
    pub(super) folders: usize,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// What one library's watch knows.
struct Watched {
    library_id: i64,
    music: bool,
    root: PathBuf,
}

impl Watcher {
    /// Watches every folder of `libraries`, sending what changes ask for to `sink`.
    pub(super) fn start(libraries: &[Library], sink: UnboundedSender<Ask>) -> anyhow::Result<Self> {
        let mut inotify = inotify::Inotify::new()?;
        let mut dirs: HashMap<i32, (usize, String)> = HashMap::new();
        let watched: Vec<Watched> = libraries
            .iter()
            .map(|l| Watched {
                library_id: l.id,
                music: l.kind == LibraryKind::Music,
                root: l.path.clone(),
            })
            .collect();
        for (n, library) in watched.iter().enumerate() {
            add_tree(&mut inotify, &mut dirs, (n, &library.root), "");
        }
        let folders = dirs.len();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("library-watch".into())
            .spawn(move || listen(inotify, dirs, &watched, &sink, &stopped))?;
        Ok(Self { stop, folders })
    }
}

/// Watches `rel` (a folder in library `n`, at `root`) and every folder below it, leaving out
/// hidden ones and links. Stops adding once the system's limit is reached, saying so once.
fn add_tree(
    inotify: &mut inotify::Inotify,
    dirs: &mut HashMap<i32, (usize, String)>,
    (n, root): (usize, &Path),
    rel: &str,
) {
    let mut pending = vec![rel.to_owned()];
    while let Some(rel) = pending.pop() {
        let path = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        match inotify.add(&path) {
            Ok(wd) => {
                dirs.insert(wd, (n, rel.clone()));
            }
            Err(err) => {
                if !inotify.full && err.raw_os_error() == Some(libc::ENOSPC) {
                    inotify.full = true;
                    tracing::warn!(
                        "the system's limit on inotify watches is reached; folders past it are found by the scan schedule (raise fs.inotify.max_user_watches to watch them)"
                    );
                }
                continue;
            }
        }
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !name.starts_with('.') && entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(if rel.is_empty() {
                    name
                } else {
                    format!("{rel}/{name}")
                });
            }
        }
    }
}

/// Reads events until `stop`, sending what each asks for.
fn listen(
    mut inotify: inotify::Inotify,
    mut dirs: HashMap<i32, (usize, String)>,
    watched: &[Watched],
    sink: &UnboundedSender<Ask>,
    stop: &AtomicBool,
) {
    let mut buffer = vec![0u8; 64 * 1024];
    while !stop.load(Ordering::Relaxed) {
        let read = match inotify.file.read(&mut buffer) {
            Ok(read) => read,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
            Err(err) => {
                tracing::warn!(error = %err, "stopped watching the library folders");
                return;
            }
        };
        for event in inotify::events(&buffer[..read]) {
            let Some((n, dir)) = dirs.get(&event.wd).cloned() else {
                continue;
            };
            if event.mask & libc::IN_IGNORED != 0 {
                dirs.remove(&event.wd);
                continue;
            }
            let library = &watched[n];
            let rel = if dir.is_empty() {
                event.name.clone()
            } else {
                format!("{dir}/{}", event.name)
            };
            if inotify::is_new_folder(&event) && !event.name.starts_with('.') {
                add_tree(&mut inotify, &mut dirs, (n, &library.root), &rel);
            }
            if let Some(ask) = ask_for(library, &dir, &event) {
                // The receiver goes only when Melete stops.
                let _ = sink.send(ask);
            }
        }
    }
}

/// The scan an event in `dir` (a folder in `library`, "" for its top) asks for, if any.
fn ask_for(library: &Watched, dir: &str, event: &inotify::Event) -> Option<Ask> {
    if event.name.is_empty() || event.name.starts_with('.') || !inotify::counts(event) {
        return None;
    }
    let whole = Ask {
        library_id: library.library_id,
        folder: None,
    };
    if library.music {
        return Some(whole);
    }
    let folder = match dir.split('/').next() {
        Some(top) if !top.is_empty() => top.to_owned(),
        _ if inotify::is_move(event) => return Some(whole),
        _ => event.name.clone(),
    };
    Some(Ask {
        library_id: library.library_id,
        folder: Some(folder),
    })
}

#[cfg(test)]
mod tests;
