//! The trash: listed, restored, emptied of files past their 30 days, or emptied now.
//!
//! Each file is deleted or put back on a thread of its own and only then recorded, in a short
//! write ([`crate::library::health::settle`]). One that doesn't come back within
//! [`TRASH_LIMIT`] marks its library as not responding and is recorded whenever it does come
//! back; the rest go on meanwhile. Every delete is logged with its path in the library, size,
//! time taken and result.

use std::time::{Duration, Instant};

use super::*;
use crate::library::health::{self, Settled};

mod emptying;
pub use emptying::*;

/// One trash operation at a time: a purge never deletes what's being put back. Held while a file
/// operation is out within its time limit, so one stuck on its share lets go after the limit.
pub(super) static TRASH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long one move into or out of the trash, or one delete from it, may take before its
/// library counts as not responding. Deleting a very large file can take seconds on some file
/// systems; a share that stops answering takes hours.
pub const TRASH_LIMIT: Duration = Duration::from_secs(120);

#[derive(sqlx::FromRow)]
pub(super) struct TrashRow {
    pub(super) id: i64,
    /// The library's name, for the log.
    pub(super) library: String,
    pub(super) root: String,
    pub(super) trash_path: String,
    pub(super) size: i64,
}

const ROWS: &str = "SELECT t.id, l.name AS library, l.path AS root, t.trash_path, t.size
    FROM trashed_files t JOIN libraries l ON l.id = t.library_id
    WHERE t.deleted_at IS NULL AND t.restored_at IS NULL";

/// Deletes files that have been in the trash for 30 days, the same way as emptying it now;
/// returns how many went.
pub async fn purge(pool: &SqlitePool) -> anyhow::Result<usize> {
    let due: Vec<TrashRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ROWS} AND t.trashed_at < ? ORDER BY t.id"
    )))
    .bind(unix_now() - KEEP_SECONDS)
    .fetch_all(pool)
    .await?;
    let mut deleted = 0;
    for row in due {
        if delete_row(pool, row).await? == Deletion::Gone {
            deleted += 1;
        }
    }
    Ok(deleted)
}

/// What deleting from the trash now did: how many files went, and how many were left because
/// what's there isn't what the trash put there (or the library's share is away).
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Emptied {
    /// Files deleted.
    pub deleted: usize,
    /// Files left in place.
    pub left: usize,
}

/// What became of one file asked to be deleted from the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deletion {
    Gone,
    /// Left in place; why.
    Left(String),
    /// The delete is still out after [`TRASH_LIMIT`]: its library now counts as not responding,
    /// and the delete is recorded once it comes back.
    Waiting,
}

/// Deletes from the trash now rather than after 30 days: one file, or with `None` every
/// file in it, one after another. The same care as [`purge`]: only inside a library's trash
/// folder, and only the file the trash put there. `None` back when the one asked for isn't in
/// the trash. A file whose delete is still out counts as left.
pub async fn delete_now(pool: &SqlitePool, id: Option<i64>) -> anyhow::Result<Option<Emptied>> {
    let rows: Vec<TrashRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ROWS} AND (?1 IS NULL OR t.id = ?1) ORDER BY t.id"
    )))
    .bind(id)
    .fetch_all(pool)
    .await?;
    if id.is_some() && rows.is_empty() {
        return Ok(None);
    }
    let mut emptied = Emptied::default();
    for row in rows {
        if delete_row(pool, row).await? == Deletion::Gone {
            emptied.deleted += 1;
        } else {
            emptied.left += 1;
        }
    }
    Ok(Some(emptied))
}

/// Deletes one file from the trash now; `None` when it isn't in the trash.
pub async fn delete_one(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Deletion>> {
    let row: Option<TrashRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ROWS} AND t.id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    match row {
        Some(row) => Ok(Some(delete_row(pool, row).await?)),
        None => Ok(None),
    }
}

/// Deletes one trashed file, and the folders it leaves empty up to the trash folder, then
/// records it deleted. Only ever inside the library's trash folder, and only the regular file of
/// the size the trash put there; anything else is left.
pub(super) async fn delete_row(pool: &SqlitePool, row: TrashRow) -> anyhow::Result<Deletion> {
    delete_row_with(pool, row, TRASH_LIMIT, unlink_trashed).await
}

/// [`delete_row`], with the blocking delete and its time limit given.
pub(super) async fn delete_row_with(
    pool: &SqlitePool,
    row: TrashRow,
    limit: Duration,
    unlink: fn(&Path, &Path, i64) -> Result<(), String>,
) -> anyhow::Result<Deletion> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    // Still in the trash, now nothing else is acting on it.
    let still: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM trashed_files
             WHERE id = ? AND deleted_at IS NULL AND restored_at IS NULL)",
    )
    .bind(row.id)
    .fetch_one(pool)
    .await?;
    let said = Said {
        library: row.library.clone(),
        path: row.trash_path.clone(),
        bytes: row.size,
        started: Instant::now(),
    };
    if !still {
        return Ok(said.left("deleted or put back already".into()));
    }
    let root = PathBuf::from(&row.root);
    // Only ever inside the trash folder.
    let Some(path) = inside(&root, &row.trash_path).filter(|p| p.starts_with(root.join(TRASH)))
    else {
        return Ok(said.left("its path isn't inside the trash folder".into()));
    };
    let (pool_after, id, size, at) = (pool.clone(), row.id, row.size, path.clone());
    let late = said.clone();
    let settled = health::settle(
        &path,
        limit,
        move || unlink(&root, &at, size),
        move |unlinked| async move {
            if unlinked.is_ok() {
                sqlx::query(
                    "UPDATE trashed_files SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
                )
                .bind(unix_now())
                .bind(id)
                .execute(&pool_after)
                .await?;
            }
            Ok::<_, sqlx::Error>(late.result(unlinked))
        },
    )
    .await;
    Ok(match settled {
        Settled::Done(recorded) => recorded?,
        Settled::Refused(down) => said.left(format!("{}: {}", down.saying(), down.why)),
        Settled::StillOut(_) => {
            tracing::warn!(
                library = said.library,
                path = said.path,
                bytes = said.bytes,
                seconds = limit.as_secs(),
                "deleting from the trash: still out; its library counts as not responding, and the delete is recorded once it finishes"
            );
            Deletion::Waiting
        }
    })
}

/// One delete, for its log line.
#[derive(Clone)]
struct Said {
    library: String,
    /// Where it is in its library.
    path: String,
    bytes: i64,
    started: Instant,
}

impl Said {
    fn ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Logs how the delete went; what became of the file.
    fn result(&self, unlinked: Result<(), String>) -> Deletion {
        match unlinked {
            Ok(()) => {
                tracing::info!(
                    library = self.library,
                    path = self.path,
                    bytes = self.bytes,
                    ms = self.ms(),
                    "deleted from the trash"
                );
                Deletion::Gone
            }
            Err(why) => self.left(why),
        }
    }

    fn left(&self, why: String) -> Deletion {
        tracing::warn!(
            library = self.library,
            path = self.path,
            bytes = self.bytes,
            ms = self.ms(),
            why,
            "left in the trash"
        );
        Deletion::Left(why)
    }
}

/// Deletes the trashed file at `path` if it's the regular file of `size` the trash put there,
/// then the folders it leaves empty up to the trash folder; why not, when it isn't. Blocking.
fn unlink_trashed(root: &Path, path: &Path, size: i64) -> Result<(), String> {
    let trash = root.join(TRASH);
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() && meta.len() as i64 == size => {
            std::fs::remove_file(path).map_err(|err| format!("deleting it: {err}"))?;
        }
        // Already gone (by hand, say), unless the whole share is: then wait.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if !trash.is_dir() {
                return Err("its library's trash folder isn't there; is the share mounted?".into());
            }
        }
        Err(err) => return Err(format!("reading it: {err}")),
        // Something else is there now: leave it.
        Ok(_) => return Err("what's there isn't the file the trash put there".into()),
    }
    // Empty folders the file left, up to the trash folder.
    let mut dir = path.parent();
    while let Some(d) = dir.filter(|d| *d != trash && d.starts_with(&trash)) {
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// A file in the trash, as listed.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Trashed {
    /// The trash entry's id, for restoring or deleting it.
    pub id: i64,
    /// The library it came from.
    pub library_id: i64,
    /// Where it was, relative to the library's folder; restoring puts it back there.
    pub rel_path: String,
    /// Where it is now, relative to the library's folder (inside its trash folder).
    pub trash_path: String,
    /// Its size in bytes.
    pub size: i64,
    /// The download whose upgrade replaced it; None once that download is gone.
    pub download_id: Option<i64>,
    /// The file that replaced it, relative to the library's folder; empty for a file set aside
    /// as the wrong one.
    pub replaced_by: String,
    /// Why it's in the trash: "upgrade", replaced by a better release, or "wrong", set aside by
    /// an admin as the wrong file for its title.
    pub reason: String,
    /// When it went to the trash, in Unix seconds.
    pub trashed_at: i64,
    /// When it will be deleted.
    pub delete_after: i64,
}

/// Files in the trash now, newest first.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<Trashed>> {
    Ok(sqlx::query_as(
        "SELECT id, library_id, rel_path, trash_path, size, download_id, replaced_by, reason,
             trashed_at, trashed_at + ? AS delete_after
         FROM trashed_files WHERE deleted_at IS NULL AND restored_at IS NULL
         ORDER BY trashed_at DESC, id DESC",
    )
    .bind(KEEP_SECONDS)
    .fetch_all(pool)
    .await?)
}

/// Why a file couldn't be put back.
#[derive(Debug, PartialEq, Eq)]
pub enum NotRestored {
    NotInTrash,
    /// The move failed; the reason.
    Failed(String),
    /// The move is still out after [`TRASH_LIMIT`]; it's recorded once it comes back.
    Waiting(String),
}

/// Puts a trashed file back where it was, never over anything there now.
pub async fn restore(pool: &SqlitePool, id: i64) -> anyhow::Result<Result<(), NotRestored>> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    let row: Option<(String, String, String, i64)> = sqlx::query_as(
        "SELECT l.path, t.trash_path, t.rel_path, t.size FROM trashed_files t
         JOIN libraries l ON l.id = t.library_id
         WHERE t.id = ? AND t.deleted_at IS NULL AND t.restored_at IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((root, trash_path, rel_path, size)) = row else {
        return Ok(Err(NotRestored::NotInTrash));
    };
    let root = PathBuf::from(root);
    let (Some(from), Some(to)) = (inside(&root, &trash_path), inside(&root, &rel_path)) else {
        return Ok(Err(NotRestored::Failed(
            "its path isn't a plain one inside the library".into(),
        )));
    };
    let pool_after = pool.clone();
    let settled = health::settle(
        &from.clone(),
        TRASH_LIMIT,
        move || {
            let trashed = std::fs::symlink_metadata(&from)
                .is_ok_and(|m| m.file_type().is_file() && m.len() as i64 == size);
            if !trashed {
                return Err("the trash doesn't hold the file it put there".to_owned());
            }
            // A removed title's folder went with its last file.
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("making its folder: {err}"))?;
            }
            crate::transfer::move_without_replacing(&from, &to)
        },
        move |moved| async move {
            if let Err(why) = moved {
                return Ok(Err(NotRestored::Failed(why)));
            }
            sqlx::query("UPDATE trashed_files SET restored_at = ? WHERE id = ?")
                .bind(unix_now())
                .bind(id)
                .execute(&pool_after)
                .await?;
            crate::jobs::request_rescan();
            Ok::<_, sqlx::Error>(Ok(()))
        },
    )
    .await;
    Ok(match settled {
        Settled::Done(restored) => restored?,
        Settled::Refused(down) => Err(NotRestored::Failed(format!(
            "{}: {}",
            down.saying(),
            down.why
        ))),
        Settled::StillOut(down) => Err(NotRestored::Waiting(format!(
            "putting it back is taking long, and {}; it's recorded once the move finishes",
            down.saying()
        ))),
    })
}

#[cfg(test)]
mod tests;
