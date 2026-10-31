//! The trash: listed, restored, emptied of files past their 30 days, or emptied now.

use super::*;

/// One trash operation at a time: a purge never deletes what's being put back.
pub(super) static TRASH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(sqlx::FromRow)]
pub(super) struct TrashRow {
    pub(super) id: i64,
    pub(super) root: String,
    pub(super) trash_path: String,
    pub(super) size: i64,
}

/// Deletes files that have been in the trash for 30 days; returns how many.
pub async fn purge(pool: &SqlitePool) -> anyhow::Result<usize> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    let due: Vec<TrashRow> = sqlx::query_as(
        "SELECT t.id, l.path AS root, t.trash_path, t.size FROM trashed_files t
         JOIN libraries l ON l.id = t.library_id
         WHERE t.deleted_at IS NULL AND t.restored_at IS NULL AND t.trashed_at < ?",
    )
    .bind(unix_now() - KEEP_SECONDS)
    .fetch_all(pool)
    .await?;
    let mut deleted = 0;
    for row in due {
        if delete_trashed(pool, row).await? {
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

/// Deletes from the trash now rather than after 30 days: one file, or with `None` every
/// file in it. The same care as [`purge`]: only inside a library's trash folder, and only the
/// file the trash put there. `None` back when the one asked for isn't in the trash.
pub async fn delete_now(pool: &SqlitePool, id: Option<i64>) -> anyhow::Result<Option<Emptied>> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    let rows: Vec<TrashRow> = sqlx::query_as(
        "SELECT t.id, l.path AS root, t.trash_path, t.size FROM trashed_files t
         JOIN libraries l ON l.id = t.library_id
         WHERE t.deleted_at IS NULL AND t.restored_at IS NULL AND (?1 IS NULL OR t.id = ?1)",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    if id.is_some() && rows.is_empty() {
        return Ok(None);
    }
    let mut emptied = Emptied::default();
    for row in rows {
        if delete_trashed(pool, row).await? {
            emptied.deleted += 1;
        } else {
            emptied.left += 1;
        }
    }
    Ok(Some(emptied))
}

/// Deletes one trashed file, and the folders it leaves empty up to the trash folder, then
/// records it deleted. Only ever inside the library's trash folder, and only the regular file of
/// the size the trash put there; anything else is left. Whether it's gone.
pub(super) async fn delete_trashed(pool: &SqlitePool, row: TrashRow) -> anyhow::Result<bool> {
    let root = PathBuf::from(&row.root);
    // Only ever inside the trash folder.
    let Some(path) = inside(&root, &row.trash_path).filter(|p| p.starts_with(root.join(TRASH)))
    else {
        return Ok(false);
    };
    let size = row.size;
    let gone = tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() && meta.len() as i64 == size => {
                std::fs::remove_file(&path)?;
            }
            // Already gone (by hand, say), unless the whole share is: then wait.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if !root.join(TRASH).is_dir() {
                    return Ok(false);
                }
            }
            // Something else is there now: leave it.
            _ => return Ok(false),
        }
        // Empty folders the file left, up to the trash folder.
        let trash = root.join(TRASH);
        let mut dir = path.parent();
        while let Some(d) = dir.filter(|d| *d != trash && d.starts_with(&trash)) {
            if std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
        Ok(true)
    })
    .await??;
    if gone {
        sqlx::query("UPDATE trashed_files SET deleted_at = ? WHERE id = ?")
            .bind(unix_now())
            .bind(row.id)
            .execute(pool)
            .await?;
    }
    Ok(gone)
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
    let moved = tokio::task::spawn_blocking(move || {
        let trashed = std::fs::symlink_metadata(&from)
            .is_ok_and(|m| m.file_type().is_file() && m.len() as i64 == size);
        if !trashed {
            return Err("the trash doesn't hold the file it put there".to_owned());
        }
        crate::transfer::move_without_replacing(&from, &to)
    })
    .await?;
    if let Err(why) = moved {
        return Ok(Err(NotRestored::Failed(why)));
    }
    sqlx::query("UPDATE trashed_files SET restored_at = ? WHERE id = ?")
        .bind(unix_now())
        .bind(id)
        .execute(pool)
        .await?;
    crate::jobs::request_rescan();
    Ok(Ok(()))
}
