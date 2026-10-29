//! Seek-bar previews ("trickplay"): a small picture every 10 seconds of each video, so a player
//! can show where a seek lands. Made in the background while it's turned on
//! (`MELETE_TRICKPLAY`, off by default, as the first run reads every video once), one file at a
//! time, from keyframes only (`-skip_frame nokey`), which is many times faster than decoding
//! every frame. The pictures are 320 pixels wide, tiled 10 by 10 into JPEG sheets, with an index
//! giving each one's time as the file has it, so they match direct play and every conversion.
//! Kept in the data folder under `trickplay/<file version>/`, never in the library, and removed
//! once the file changes or goes.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::process::Command;

use crate::db::unix_now;

mod making;

pub use making::*;

/// A picture every this many seconds, at most.
pub const INTERVAL: f64 = 10.0;
/// Each picture's width in pixels; the height keeps the video's shape.
pub const WIDTH: i64 = 320;
/// Pictures across a sheet…
pub const COLUMNS: i64 = 10;
/// …and down it.
pub const ROWS: i64 = 10;
/// Reading a whole film's keyframes can take a while over a network share.
const MAKE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A file's previews, as `index.json` beside the sheets keeps them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    /// Each picture's width in pixels.
    pub width: i64,
    /// Each picture's height in pixels.
    pub height: i64,
    /// Pictures across a sheet.
    pub columns: i64,
    /// Pictures down a sheet.
    pub rows: i64,
    /// How many sheets there are, named `0.jpg`, `1.jpg`…
    pub sheets: i64,
    /// Each picture's time in seconds, in order: picture `i` is on sheet `i / (columns × rows)`,
    /// at column `i % columns` and row `(i / columns) % rows`.
    pub times: Vec<f64>,
}

/// Where a file version's previews are kept, inside the trickplay folder.
pub fn version_dir(root: &Path, file_id: i64, size: i64, modified_at: i64) -> PathBuf {
    root.join(format!("{file_id}-{size}-{modified_at}"))
}

/// The trickplay folder in the data folder.
pub fn folder(data_dir: &Path) -> PathBuf {
    data_dir.join("trickplay")
}

/// Whether previews were made for a file as it is now.
pub async fn made(pool: &SqlitePool, file_id: i64) -> sqlx::Result<bool> {
    Ok(current_version(pool, file_id).await?.is_some())
}

/// The size and date of a file as it is now, when previews were made for that version.
async fn current_version(pool: &SqlitePool, file_id: i64) -> sqlx::Result<Option<(i64, i64)>> {
    sqlx::query_as(
        "SELECT f.size, f.modified_at FROM media_trickplay t JOIN media_files f ON f.id = t.file_id
         WHERE t.file_id = ? AND t.size = f.size AND t.modified_at = f.modified_at
           AND t.error IS NULL AND f.missing_since IS NULL",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await
}

/// The index of a file's previews as it is now; None when none were made for this version.
pub async fn index_of(
    pool: &SqlitePool,
    root: &Path,
    file_id: i64,
) -> anyhow::Result<Option<Index>> {
    let Some((size, modified_at)) = current_version(pool, file_id).await? else {
        return Ok(None);
    };
    let path = version_dir(root, file_id, size, modified_at).join("index.json");
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// A sheet of a file's current previews, by number; None when there's no such sheet.
pub async fn sheet_path(
    pool: &SqlitePool,
    root: &Path,
    file_id: i64,
    sheet: i64,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(index) = index_of(pool, root, file_id).await? else {
        return Ok(None);
    };
    if !(0..index.sheets).contains(&sheet) {
        return Ok(None);
    }
    let (size, modified_at): (i64, i64) =
        sqlx::query_as("SELECT size, modified_at FROM media_files WHERE id = ?")
            .bind(file_id)
            .fetch_one(pool)
            .await?;
    Ok(Some(
        version_dir(root, file_id, size, modified_at).join(format!("{sheet}.jpg")),
    ))
}

/// A folder being made that's older than this was left by a run that stopped.
const ABANDONED: Duration = Duration::from_secs(3 * 3600);

async fn older_than(entry: &tokio::fs::DirEntry, age: Duration) -> bool {
    let modified = entry.metadata().await.and_then(|m| m.modified());
    modified.is_ok_and(|t| t.elapsed().is_ok_and(|e| e > age))
}

/// Removes previews nothing will ask for again: folders of file versions that are no longer
/// in the library as they were, and half-made ones. Names it doesn't recognise are left alone.
/// Returns how many folders it removed.
pub async fn clean(pool: &SqlitePool, root: &Path) -> anyhow::Result<usize> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let current: std::collections::HashSet<String> = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT id, size, modified_at FROM media_files WHERE missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, size, modified)| format!("{id}-{size}-{modified}"))
    .collect();
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let ours = name.split('-').count() == 3
            && name
                .split('-')
                .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        // One being made now is left alone: making one takes far less than an hour.
        let abandoned = name.starts_with(".making-") && older_than(&entry, ABANDONED).await;
        if (ours && !current.contains(&name)) || abandoned {
            tokio::fs::remove_dir_all(entry.path()).await?;
            removed += 1;
        }
    }
    Ok(removed)
}
