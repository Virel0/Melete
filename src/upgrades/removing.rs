//! Removing a whole title an admin chose on the Make room page: every regular file in its folder
//! (or its one loose file) moved, never over anything, to `.melete-trash/removed-<when>-<title>/`
//! at the top of its library, each recorded so it can be put back, and deleted 30 days later
//! with the rest of the trash. Links aren't followed, and nothing moves unless every video is
//! still as it was scanned. A wanted title stops being monitored, so it isn't downloaded again.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use super::*;

/// What was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// Files moved to the trash.
    pub files: usize,
    /// Their size together, in bytes.
    pub size: i64,
}

/// Why a title wasn't removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotRemoved {
    NotFound,
    /// Nothing moved; the reason.
    Failed(String),
}

#[derive(sqlx::FromRow)]
struct TitleRow {
    library_id: i64,
    root: String,
    rel_path: String,
}

/// Moves a movie's (or with `series`, a series') files to its library's trash.
pub async fn remove_title(
    pool: &SqlitePool,
    series: bool,
    id: i64,
) -> anyhow::Result<Result<Removed, NotRemoved>> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    let (table, column) = if series {
        ("series", "series_id")
    } else {
        ("movies", "movie_id")
    };
    let title: Option<TitleRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT x.library_id, l.path AS root, x.rel_path FROM {table} x
         JOIN libraries l ON l.id = x.library_id
         WHERE x.id = ? AND x.missing_since IS NULL"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(title) = title else {
        return Ok(Err(NotRemoved::NotFound));
    };
    let scanned: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT rel_path, size, modified_at FROM media_files
         WHERE {column} = ? AND missing_since IS NULL"
    )))
    .bind(id)
    .fetch_all(pool)
    .await?;
    let root = PathBuf::from(&title.root);
    let tag = format!("{}{id}", if series { "s" } else { "m" });
    let base = format!("{TRASH}/removed-{}-{tag}", unix_now());
    let (top, rel_top) = (title.rel_path.clone(), title.rel_path.clone());
    let (root_moved, base_moved) = (root.clone(), base.clone());
    let moved =
        tokio::task::spawn_blocking(move || move_all(&root_moved, &top, &base_moved, &scanned))
            .await?;
    let moved = match moved {
        Ok(moved) => moved,
        Err(why) => return Ok(Err(NotRemoved::Failed(why))),
    };
    if let Err(err) = record(pool, &title, series, id, &moved).await {
        // Unrecorded, they'd sit in the trash forever: put them back.
        let root = root.clone();
        let back = tokio::task::spawn_blocking(move || put_all_back(&root, &moved)).await?;
        if let Err(why) = back {
            tracing::error!(error = %err, why, "a removed title couldn't be recorded or put back");
        }
        return Err(err.into());
    }
    let size = moved.iter().map(|m| m.size).sum();
    let files = moved.len();
    // Its emptied folders go; the trash keeps every file.
    let _ = tokio::task::spawn_blocking(move || prune(&root, &rel_top)).await;
    crate::jobs::request_rescan();
    Ok(Ok(Removed { files, size }))
}

/// One file moved: where it was and where it is, relative to the library, and its size.
#[derive(Debug, Clone)]
struct Moved {
    rel_path: String,
    trash_path: String,
    size: i64,
}

/// Moves every regular file under `top` (a folder, or one file) to `base` in the trash, keeping
/// its path under it; first checks every scanned video is as it was scanned. On any failure,
/// what was moved goes back and the reason is given. Blocking.
fn move_all(
    root: &Path,
    top: &str,
    base: &str,
    scanned: &[(String, i64, i64)],
) -> Result<Vec<Moved>, String> {
    let not_plain = || "its path isn't a plain one inside the library".to_owned();
    let start = inside(root, top).ok_or_else(not_plain)?;
    for (rel, size, modified) in scanned {
        let path = inside(root, rel).ok_or_else(not_plain)?;
        if !as_scanned(&path, *size, *modified) {
            return Err(format!(
                "{rel} isn't as it was scanned; rescan its folder first"
            ));
        }
    }
    let mut found = Vec::new();
    collect(&start, &mut found).map_err(|err| format!("reading its folder: {err}"))?;
    let mut moved: Vec<Moved> = Vec::new();
    for path in found {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().into_owned();
        let trash_path = format!("{base}/{rel}");
        match move_one(root, &path, &trash_path) {
            Ok(size) => moved.push(Moved {
                rel_path: rel,
                trash_path,
                size,
            }),
            Err(why) => {
                if let Err(back) = put_all_back(root, &moved) {
                    tracing::error!(why = back, "a removed title's files couldn't all go back");
                }
                return Err(format!("moving {rel}: {why}"));
            }
        }
    }
    if moved.is_empty() {
        return Err("it has no files to remove".into());
    }
    Ok(moved)
}

/// Moves one file to `trash_path` in its library, never over anything; its size. Blocking.
fn move_one(root: &Path, path: &Path, trash_path: &str) -> Result<i64, String> {
    let to = inside(root, trash_path)
        .ok_or_else(|| "its path isn't a plain one inside the library".to_owned())?;
    let size = std::fs::symlink_metadata(path)
        .map_err(|err| err.to_string())?
        .len();
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("making the trash folder: {err}"))?;
    }
    crate::transfer::move_without_replacing(path, &to)?;
    Ok(i64::try_from(size).unwrap_or(i64::MAX))
}

/// Every regular file under `path` (or `path` itself), never following a link and never into a
/// trash folder.
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_file() {
        out.push(path.to_owned());
    } else if meta.file_type().is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            if entry.file_name() == TRASH {
                continue;
            }
            collect(&entry.path(), out)?;
        }
    }
    Ok(())
}

/// Whether a file is the regular file a scan saw: its size and modification time.
fn as_scanned(path: &Path, size: i64, modified: i64) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| {
        let at = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        m.file_type().is_file() && m.len() as i64 == size && at == Some(modified)
    })
}

/// Moves files back out of the trash, never over anything. Blocking.
fn put_all_back(root: &Path, moved: &[Moved]) -> Result<(), String> {
    let mut failed = Vec::new();
    for m in moved {
        let (Some(from), Some(to)) = (inside(root, &m.trash_path), inside(root, &m.rel_path))
        else {
            continue;
        };
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(why) = crate::transfer::move_without_replacing(&from, &to) {
            failed.push(format!("{}: {why}", m.rel_path));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed.join("; "))
    }
}

/// Removes the title's folders left empty, deepest first; a file anyone put back stops it.
/// Blocking.
fn prune(root: &Path, top: &str) {
    fn empty_dirs(dir: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        let mut empty = true;
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if !(is_dir && empty_dirs(&path)) {
                empty = false;
            }
        }
        empty && std::fs::remove_dir(dir).is_ok()
    }
    if let Some(start) = inside(root, top)
        && std::fs::symlink_metadata(&start).is_ok_and(|m| m.file_type().is_dir())
    {
        empty_dirs(&start);
    }
}

/// Records each moved file in the trash, and the title, its files and extras as gone from the
/// library, and stops a wanted title being monitored, in one transaction.
async fn record(
    pool: &SqlitePool,
    title: &TitleRow,
    series: bool,
    id: i64,
    moved: &[Moved],
) -> sqlx::Result<()> {
    let at = unix_now();
    let (table, column, wanted) = if series {
        ("series", "series_id", "wanted_series")
    } else {
        ("movies", "movie_id", "wanted_movies")
    };
    let mut tx = pool.begin().await?;
    for m in moved {
        sqlx::query(
            "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, download_id,
                 replaced_by, trashed_at, reason)
             VALUES (?, ?, ?, ?, NULL, '', ?, 'removed')",
        )
        .bind(title.library_id)
        .bind(&m.rel_path)
        .bind(&m.trash_path)
        .bind(m.size)
        .bind(at)
        .execute(&mut *tx)
        .await?;
    }
    for gone in ["media_files", "media_extras"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {gone} SET missing_since = coalesce(missing_since, ?) WHERE {column} = ?"
        )))
        .bind(at)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET missing_since = coalesce(missing_since, ?) WHERE id = ?"
    )))
    .bind(at)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {wanted} SET monitored = 0 WHERE {column} = ?"
    )))
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}
