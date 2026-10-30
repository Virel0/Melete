//! Which file a download put at a name in the library, recorded when it's placed (size, last
//! change, inode and device), so a file is taken for Melete's own import only while the one at
//! that name is still that file: never one replaced by hand or put back there from elsewhere.

use super::*;

/// What identifies a file on disk.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(crate) struct Identity {
    pub(crate) size: i64,
    pub(crate) modified_ns: i64,
    pub(crate) inode: Option<i64>,
    pub(crate) device: Option<i64>,
}

/// The identity of the regular file at `path` now, never through a link; None when there's no
/// regular file there. Blocking.
pub(crate) fn identity_of(path: &Path) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let modified_ns = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(Identity {
        size: i64::try_from(meta.len()).ok()?,
        modified_ns: i64::try_from(modified_ns).ok()?,
        // Stored as SQLite's signed integers: the bits are what's compared.
        inode: Some(meta.ino() as i64),
        device: Some(meta.dev() as i64),
    })
}

/// Records that download `id` put the file now at `path` there.
pub(crate) async fn record_placed(pool: &SqlitePool, id: i64, path: &Path) -> Result<(), Stop> {
    let at = path.to_path_buf();
    let Some(identity) = blocking(move || Ok(identity_of(&at))).await? else {
        return Err(Stop::Retry(format!(
            "{} isn't a regular file just after it was put in place",
            path.display()
        )));
    };
    sqlx::query(
        "INSERT INTO placed_files (download_id, path, size, modified_ns, inode, device)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (download_id, path) DO UPDATE SET size = excluded.size,
             modified_ns = excluded.modified_ns, inode = excluded.inode, device = excluded.device",
    )
    .bind(id)
    .bind(path.to_string_lossy())
    .bind(identity.size)
    .bind(identity.modified_ns)
    .bind(identity.inode)
    .bind(identity.device)
    .execute(pool)
    .await
    .map_err(|err| Stop::Retry(format!("the database: {err}")))?;
    Ok(())
}

/// The download that put the file now at `path` there: the newest one that recorded placing a
/// file there whose size, last change and (where recorded) inode and device it still has.
pub(crate) async fn placed_by(pool: &SqlitePool, path: &Path) -> sqlx::Result<Option<i64>> {
    let recorded: Vec<(i64, Identity)> =
        sqlx::query_as::<_, (i64, i64, i64, Option<i64>, Option<i64>)>(
            "SELECT download_id, size, modified_ns, inode, device FROM placed_files
         WHERE path = ? ORDER BY download_id DESC",
        )
        .bind(path.to_string_lossy())
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, size, modified_ns, inode, device)| {
            (
                id,
                Identity {
                    size,
                    modified_ns,
                    inode,
                    device,
                },
            )
        })
        .collect();
    if recorded.is_empty() {
        return Ok(None);
    }
    let at = path.to_path_buf();
    let Ok(now) = tokio::task::spawn_blocking(move || identity_of(&at)).await else {
        return Ok(None);
    };
    let Some(now) = now else {
        return Ok(None);
    };
    Ok(recorded.into_iter().find_map(|(id, then)| {
        let same = then.size == now.size
            && then.modified_ns == now.modified_ns
            && then.inode.is_none_or(|i| Some(i) == now.inode)
            && then.device.is_none_or(|d| Some(d) == now.device);
        same.then_some(id)
    }))
}
