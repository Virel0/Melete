//! Putting a compressed copy in a file's place: the file moved, never over anything, to
//! `.melete-trash/compressed-<when>-<file>/` at the top of its library, the new copy placed where
//! it was (as `.mkv`), never over anything, and the file's row pointed at it, so watching
//! progress and everything else kept for the file stays. The trash entry says what replaced it;
//! it's deleted 30 days later with the rest of the trash, or put back.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use super::*;

/// Replaces library file `file_id` with `new`, a finished copy outside the library, if the file
/// is still as it was scanned: the bytes it saved, or why not; nothing is changed then.
pub async fn replace_with_compressed(
    pool: &SqlitePool,
    file_id: i64,
    new: &Path,
) -> anyhow::Result<Result<i64, String>> {
    let _one_at_a_time = TRASH_LOCK.lock().await;
    let row: Option<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT f.library_id, l.path, f.rel_path, f.size, f.modified_at FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         WHERE f.id = ? AND f.missing_since IS NULL",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let Some((library_id, root, rel_path, size, modified)) = row else {
        return Ok(Err("it's no longer in the library".into()));
    };
    let root = PathBuf::from(root);
    let new_rel = Path::new(&rel_path)
        .with_extension("mkv")
        .to_string_lossy()
        .into_owned();
    let trash_path = format!("{TRASH}/compressed-{}-{file_id}/{rel_path}", unix_now());
    let (Some(old), Some(trash), Some(target)) = (
        inside(&root, &rel_path),
        inside(&root, &trash_path),
        inside(&root, &new_rel),
    ) else {
        return Ok(Err("its path isn't a plain one inside the library".into()));
    };
    let new = new.to_owned();
    let (old_back, trash_back) = (old.clone(), trash.clone());
    let placed = tokio::task::spawn_blocking(move || {
        swap_on_disk(&old, &trash, &new, &target, size, modified)
    })
    .await?;
    let (new_size, new_modified) = match placed {
        Ok(placed) => placed,
        Err(why) => return Ok(Err(why)),
    };
    let replacement = Replacement {
        file_id,
        library_id,
        rel_path: &rel_path,
        trash_path: &trash_path,
        size,
        new_rel: &new_rel,
        new_size,
        new_modified,
    };
    if let Err(err) = record(pool, &replacement).await {
        // Unrecorded, the old file would sit in the trash and the row point nowhere: undo.
        let target = inside(&root, &new_rel);
        let undone = tokio::task::spawn_blocking(move || {
            if let Some(target) = target {
                let _ = std::fs::remove_file(&target);
            }
            crate::transfer::move_without_replacing(&trash_back, &old_back)
        })
        .await?;
        if let Err(why) = undone {
            tracing::error!(error = %err, why, "a compressed file couldn't be recorded or undone");
        }
        return Err(err.into());
    }
    crate::jobs::request_rescan();
    Ok(Ok((size - new_size).max(0)))
}

/// Moves the old file to the trash and puts the new copy where it was; puts the old one back if
/// the new one can't be placed. The new copy's size and modification time. Blocking.
fn swap_on_disk(
    old: &Path,
    trash: &Path,
    new: &Path,
    target: &Path,
    size: i64,
    modified: i64,
) -> Result<(i64, i64), String> {
    let meta = std::fs::symlink_metadata(old).map_err(|err| err.to_string())?;
    let seconds = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    if !meta.file_type().is_file() || meta.len() as i64 != size || seconds != Some(modified) {
        return Err("on disk it isn't the file that was scanned; rescan its folder first".into());
    }
    if let Some(parent) = trash.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("making the trash folder: {err}"))?;
    }
    crate::transfer::move_without_replacing(old, trash)?;
    // Copied beside where it goes, then put in place without replacing anything.
    let partial = target.with_extension("mkv.partial");
    let placed = std::fs::copy(new, &partial)
        .map_err(|err| format!("copying the new file in: {err}"))
        .and_then(|_| crate::transfer::move_without_replacing(&partial, target));
    if let Err(why) = placed {
        let _ = std::fs::remove_file(&partial);
        if let Err(back) = crate::transfer::move_without_replacing(trash, old) {
            tracing::error!(
                why = back,
                "a file set aside for its compressed copy couldn't go back"
            );
        }
        return Err(why);
    }
    let _ = std::fs::remove_file(new);
    let placed = std::fs::metadata(target).map_err(|err| err.to_string())?;
    let at = placed
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    Ok((i64::try_from(placed.len()).unwrap_or(i64::MAX), at))
}

/// A file and the compressed copy that took its place, as recorded.
struct Replacement<'a> {
    file_id: i64,
    library_id: i64,
    rel_path: &'a str,
    trash_path: &'a str,
    size: i64,
    new_rel: &'a str,
    new_size: i64,
    new_modified: i64,
}

/// Records the old file in the trash, replaced by the new one, and points the file's row at the
/// new one (its probe to be made again), in one transaction.
async fn record(pool: &SqlitePool, r: &Replacement<'_>) -> sqlx::Result<()> {
    let at = unix_now();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, download_id,
             replaced_by, trashed_at, reason)
         VALUES (?, ?, ?, ?, NULL, ?, ?, 'upgrade')",
    )
    .bind(r.library_id)
    .bind(r.rel_path)
    .bind(r.trash_path)
    .bind(r.size)
    .bind(r.new_rel)
    .bind(at)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE media_files SET rel_path = ?, size = ?, modified_at = ? WHERE id = ?")
        .bind(r.new_rel)
        .bind(r.new_size)
        .bind(r.new_modified)
        .bind(r.file_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM media_probes WHERE file_id = ?")
        .bind(r.file_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}
