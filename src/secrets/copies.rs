//! The backup copies made before the switch still hold secrets in plain text: the copy made
//! just before migrating to sealed storage, and the daily and weekly ones before it. Once the
//! live database is switched, each copy in the backups folder is opened and its plain secrets
//! are sealed in place, in one transaction with `secure_delete` on, then the copy is vacuumed so
//! no freed page keeps the old bytes. A copy from before sealed storage has the old schema (no
//! triggers, a two-factor check that wants 20 bytes), so its checks are set aside for this
//! connection only. A copy that can't be done is named in a warning, never deleted, and tried
//! again at the next start.

use std::path::Path;
use std::time::SystemTime;

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection, SqlitePool};

use super::Key;
use crate::backup;
use crate::db::unix_now;

/// Seals the plain secrets of every copy in `data_dir`'s backups folder, once after the switch:
/// recorded as done when every copy was. How many values were sealed.
pub(super) async fn seal_copies(
    pool: &SqlitePool,
    data_dir: &Path,
    key: &Key,
) -> anyhow::Result<usize> {
    let done: Option<i64> = sqlx::query_scalar("SELECT copies_sealed_at FROM secrets_sealed")
        .fetch_optional(pool)
        .await?
        .flatten();
    if done.is_some() {
        return Ok(0);
    }
    let folder = data_dir.join(backup::FOLDER);
    let mut sealed = 0;
    let mut all_done = true;
    for (name, _, _) in backup::restore::copies(data_dir)? {
        let path = folder.join(&name);
        match seal_copy(&path, key).await {
            Ok(count) => sealed += count,
            Err(err) => {
                all_done = false;
                tracing::warn!(
                    copy = %path.display(),
                    error = format!("{err:#}"),
                    "couldn't seal the secrets in this backup copy; it's kept as it is, and tried again at the next start"
                );
            }
        }
    }
    if all_done {
        sqlx::query("UPDATE secrets_sealed SET copies_sealed_at = ?")
            .bind(unix_now())
            .execute(pool)
            .await?;
    }
    Ok(sealed)
}

/// Seals one copy's plain secrets in place, keeping its time of change; how many.
async fn seal_copy(path: &Path, key: &Key) -> anyhow::Result<usize> {
    let changed = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .with_context(|| format!("reading {}", path.display()))?;
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        // Copies are single files; never leave a WAL beside one.
        .journal_mode(SqliteJournalMode::Delete)
        .pragma("secure_delete", "ON")
        .pragma("ignore_check_constraints", "ON")
        .pragma("temp_store", "memory")
        .connect()
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut tx = conn.begin().await?;
    let sealed = super::sealing::seal_all(&mut tx, key).await?;
    tx.commit().await?;
    if sealed > 0 {
        sqlx::query("VACUUM").execute(&mut conn).await?;
    }
    conn.close().await?;
    keep_time(path, changed)?;
    Ok(sealed)
}

/// Puts back a copy's time of change, which says when it was made.
fn keep_time(path: &Path, changed: SystemTime) -> anyhow::Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(changed))
        .with_context(|| format!("keeping the time of {}", path.display()))
}
