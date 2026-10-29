//! Asking Deluge how downloads on the seedbox are doing: finished ones completed, long-unlisted
//! ones failed.

use super::*;

/// How long a download may be missing from Deluge before it counts as removed: Deluge's
/// daemon lists nothing while it starts, and a wrong daemon may be connected for a while.
pub(super) const MISSING_FOR: i64 = 30 * 60;

/// Asks Deluge how the downloads on the seedbox are doing; marks finished ones completed and
/// ones Deluge hasn't listed for half an hour failed. Returns how many it asked about.
pub async fn refresh(pool: &SqlitePool, deluge: &deluge::Client) -> anyhow::Result<usize> {
    let rows: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT id, infohash, missing_since FROM downloads
         WHERE state = 'downloading' AND source = 'torrent'",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let hashes: Vec<String> = rows.iter().map(|(_, h, _)| h.clone()).collect();
    let statuses = deluge.statuses(&hashes).await?;
    let now = unix_now();
    let mut tx = pool.begin().await?;
    for (id, hash, missing_since) in &rows {
        // Only downloads still downloading: another step may have moved one on meanwhile.
        match statuses.get(hash) {
            None => {
                let since = missing_since.unwrap_or(now);
                let gone = now - since >= MISSING_FOR;
                sqlx::query(
                    "UPDATE downloads SET state = ?, missing_since = ?, message = ?, updated_at = ?
                     WHERE id = ? AND state = 'downloading'",
                )
                .bind(if gone { "failed" } else { "downloading" })
                .bind(since)
                .bind(if gone {
                    "it was removed from Deluge before it finished"
                } else {
                    "Deluge doesn't list it at the moment"
                })
                .bind(now)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
            Some(status) => {
                // Deluge marks a torrent finished even when moving it afterwards failed (the
                // Error state); its files may be half moved.
                let finished = status.is_finished && status.state != "Error";
                let message = match status.state.as_str() {
                    "Error" => format!("Deluge: {}", status.message),
                    state => state.to_owned(),
                };
                sqlx::query(
                    "UPDATE downloads SET state = ?, progress = ?, size = ?, save_path = ?, name = ?,
                         files = ?, message = ?, missing_since = NULL, updated_at = ?, completed_at = ?
                     WHERE id = ? AND state = 'downloading'",
                )
                .bind(if finished { "completed" } else { "downloading" })
                .bind(if finished { 100.0 } else { status.progress.clamp(0.0, 100.0) })
                .bind(status.total_size)
                .bind(&status.save_path)
                .bind(&status.name)
                .bind(serde_json::to_string(&status.files)?)
                .bind(message)
                .bind(now)
                .bind(finished.then_some(now))
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(rows.len())
}
