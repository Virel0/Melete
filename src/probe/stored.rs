//! Keeping a probe's result: a library file's in `media_probes`, an extra's on its own row.
//! A file's chapters are kept by [`store_probed`](super::store_probed); stored here, it has none
//! until it's read for them.

use super::*;

/// Stores a probe result. Timeouts aren't stored, so the file is tried again next time.
pub async fn store(
    pool: &SqlitePool,
    file_id: i64,
    size: i64,
    modified_at: i64,
    result: &Result<MediaInfo, ProbeError>,
) -> anyhow::Result<()> {
    let (info, error) = match result {
        Ok(info) => (Some(serde_json::to_string(info)?), None),
        Err(ProbeError::Failed(reason)) => (None, Some(reason.clone())),
        Err(ProbeError::TimedOut | ProbeError::Unreadable(_) | ProbeError::Unavailable(_)) => {
            return Ok(());
        }
    };
    if crate::library::extras::is_extra_id(file_id) {
        sqlx::query(
            "UPDATE media_extras SET probed_size = ?, probed_modified_at = ?, probe_info = ?,
                 probe_error = ?
             WHERE id = ?",
        )
        .bind(size)
        .bind(modified_at)
        .bind(info)
        .bind(error)
        .bind(file_id)
        .execute(pool)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info, error)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id) DO UPDATE SET
             size = excluded.size, modified_at = excluded.modified_at,
             probed_at = excluded.probed_at, info = excluded.info, error = excluded.error,
             chapters = NULL",
    )
    .bind(file_id)
    .bind(size)
    .bind(modified_at)
    .bind(unix_now())
    .bind(info)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}
