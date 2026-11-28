//! A quick look at each library when Melete starts, for what changed while it was down: only
//! folder times are read (each title folder's and the folders right inside it), never a full
//! walk, and only in a library the first library check found answering. A title folder changed
//! since the library's last whole scan is scanned on its own; a title folder that's new at the
//! top (it may be one renamed) or a music library with anything changed is scanned whole, as
//! the folder watch does. Nothing here marks anything missing: a scan it asks for does that as
//! any scan does. A library never scanned is left for someone to ask, as before.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use sqlx::SqlitePool;

use super::{Library, LibraryKind, health};

/// How long the look at one library may take.
const LIMIT: Duration = Duration::from_secs(300);
/// A folder changed this long before the last whole scan started counts too: clocks and file
/// systems' times are only so exact.
const SLACK: i64 = 120;
/// More changed title folders than this and the library is scanned whole.
const MOST_FOLDERS: usize = 200;

/// What a library needs scanned.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Changed {
    #[default]
    Nothing,
    /// These title folders (their paths in the library).
    Folders(Vec<String>),
    Whole,
}

/// Looks at every library once the first library check is in, and asks for the scans what
/// changed while Melete was down needs.
pub async fn look(pool: SqlitePool, libraries: Vec<Library>) {
    health::checked().await;
    for library in &libraries {
        match look_at(&pool, library).await {
            Ok(Changed::Nothing) => {}
            Ok(Changed::Whole) => {
                tracing::info!(
                    library = library.name,
                    "changed while Melete was down: scanning it"
                );
                crate::jobs::request_folder_scan(library.id, None);
            }
            Ok(Changed::Folders(folders)) => {
                tracing::info!(
                    library = library.name,
                    folders = folders.len(),
                    "folders changed while Melete was down: scanning them"
                );
                for folder in folders {
                    crate::jobs::request_folder_scan(library.id, Some(folder));
                }
            }
            Err(why) => tracing::info!(
                library = library.name,
                why,
                "not looked at for changes made while Melete was down"
            ),
        }
    }
}

/// What one library needs scanned, or why it wasn't looked at.
async fn look_at(pool: &SqlitePool, library: &Library) -> Result<Changed, String> {
    if let Some(down) = health::down_at(&library.path) {
        return Err(format!("{}: {}", down.saying(), down.why));
    }
    let last: Option<i64> = sqlx::query_scalar("SELECT last_scan_at FROM libraries WHERE id = ?")
        .bind(library.id)
        .fetch_optional(pool)
        .await
        .map_err(|err| err.to_string())?
        .flatten();
    let Some(last) = last else {
        return Ok(Changed::Nothing);
    };
    let known: HashSet<String> = match library.kind {
        LibraryKind::Music => HashSet::new(),
        kind => {
            let table = if kind == LibraryKind::Movies {
                "movies"
            } else {
                "series"
            };
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT rel_path FROM {table} WHERE library_id = ? AND missing_since IS NULL"
            )))
            .bind(library.id)
            .fetch_all(pool)
            .await
            .map_err(|err| err.to_string())?
            .into_iter()
            .collect()
        }
    };
    let (root, kind) = (library.path.clone(), library.kind);
    health::within(&library.path, LIMIT, move || {
        changed(&root, kind, last - SLACK, &known)
    })
    .await
    .map_err(|down| format!("{}: {}", down.saying(), down.why))?
}

/// When a folder or file was last changed, in Unix seconds.
fn changed_at(path: &Path) -> Option<i64> {
    let at = std::fs::symlink_metadata(path).ok()?.modified().ok()?;
    let secs = at.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Whether the title at `path` (a folder, or a loose file) changed after `since`: its own time,
/// or that of a folder right inside it (a season, an album, an extras folder). Blocking.
fn title_changed(path: &Path, since: i64) -> bool {
    if changed_at(path).is_some_and(|at| at > since) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.file_type().is_ok_and(|t| t.is_dir())
            && changed_at(&entry.path()).is_some_and(|at| at > since)
    })
}

/// What changed under `root` after `since`, against the title folders scans know (`known`).
/// Blocking.
pub(super) fn changed(
    root: &Path,
    kind: LibraryKind,
    since: i64,
    known: &HashSet<String>,
) -> Result<Changed, String> {
    let entries =
        std::fs::read_dir(root).map_err(|err| format!("its folder can't be read: {err}"))?;
    // A folder moved or renamed at the top keeps its own time but changes the library's.
    let top_changed = changed_at(root).is_some_and(|at| at > since);
    let mut folders = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("its folder can't be read: {err}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // Hidden names (the trash, unfinished copies) are never titles.
        if name.starts_with('.') {
            continue;
        }
        let new = kind != LibraryKind::Music && !known.contains(&name);
        if new && top_changed {
            // New at the top: perhaps one renamed, which only a whole scan can tell.
            return Ok(Changed::Whole);
        }
        if !title_changed(&entry.path(), since) {
            continue;
        }
        // Music is scanned whole.
        if kind == LibraryKind::Music || new {
            return Ok(Changed::Whole);
        }
        folders.push(name);
        if folders.len() > MOST_FOLDERS {
            return Ok(Changed::Whole);
        }
    }
    folders.sort();
    Ok(if folders.is_empty() {
        Changed::Nothing
    } else {
        Changed::Folders(folders)
    })
}

#[cfg(test)]
mod tests;
