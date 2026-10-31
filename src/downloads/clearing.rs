//! Removing imported torrents from Deluge, when that's turned on: only torrents Melete added
//! itself, only once everything they brought is in the library (imported, never while being
//! copied or placed), and, when asked to, only after a share ratio or a time seeding.

use super::*;

/// What becomes of an imported torrent in Deluge (`MELETE_DELUGE_REMOVE`): never, kept_data or
/// with_data.
pub const REMOVE_SETTING: &str = "MELETE_DELUGE_REMOVE";
/// The share ratio to wait for first (`MELETE_DELUGE_REMOVE_RATIO`).
pub const RATIO_SETTING: &str = "MELETE_DELUGE_REMOVE_RATIO";
/// The hours seeding to wait for first (`MELETE_DELUGE_REMOVE_SEED_HOURS`).
pub const SEED_HOURS_SETTING: &str = "MELETE_DELUGE_REMOVE_SEED_HOURS";

/// When an imported torrent is removed from Deluge, as the settings say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clearing {
    /// Whether Deluge deletes its data on the seedbox too.
    pub with_data: bool,
    /// Wait until it has seeded to this share ratio...
    pub ratio: Option<f64>,
    /// ...or for this many seconds, whichever comes first; neither set, no wait.
    pub seed_for: Option<i64>,
}

impl Clearing {
    /// The settings as they stand; None when imported torrents stay in Deluge: the default,
    /// and also when a ratio or time to wait for isn't a number, rather than removing too soon.
    pub async fn from_settings(pool: &SqlitePool) -> sqlx::Result<Option<Clearing>> {
        let with_data = match crate::settings::value(pool, REMOVE_SETTING)
            .await?
            .as_deref()
        {
            Some("kept_data") => false,
            Some("with_data") => true,
            _ => return Ok(None),
        };
        let mut wait = [None, None];
        for (i, name) in [RATIO_SETTING, SEED_HOURS_SETTING].into_iter().enumerate() {
            let Some(set) = crate::settings::value(pool, name).await? else {
                continue;
            };
            if set.trim().is_empty() {
                continue;
            }
            match set.trim().parse::<f64>() {
                Ok(n) if n.is_finite() && n >= 0.0 => wait[i] = Some(n),
                _ => {
                    tracing::warn!(
                        setting = name,
                        "isn't a number; imported torrents stay in Deluge until it is"
                    );
                    return Ok(None);
                }
            }
        }
        Ok(Some(Clearing {
            with_data,
            ratio: wait[0],
            seed_for: wait[1].map(|hours| (hours * 3600.0) as i64),
        }))
    }

    /// Whether a torrent has seeded enough to go: no wait asked for, or either one reached.
    fn seeded_enough(&self, status: &deluge::Status) -> bool {
        if self.ratio.is_none() && self.seed_for.is_none() {
            return true;
        }
        self.ratio.is_some_and(|r| status.ratio >= r)
            || self.seed_for.is_some_and(|s| status.seeding_time >= s)
    }
}

/// Removes imported torrents Melete added from Deluge, as `clearing` says; returns how many it
/// removed. One Deluge hasn't listed for half an hour (its daemon lists nothing while it starts)
/// is noted as gone and left; one Deluge refuses to remove is tried again next time.
pub async fn clear_imported(
    pool: &SqlitePool,
    deluge: &deluge::Client,
    clearing: &Clearing,
) -> anyhow::Result<usize> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, infohash FROM downloads
         WHERE source = 'torrent' AND state = 'imported' AND added_here = 1
           AND deluge_removed IS NULL AND infohash IS NOT NULL
         ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let hashes: Vec<String> = rows.iter().map(|(_, h)| h.clone()).collect();
    let statuses = deluge.statuses(&hashes).await?;
    let mut removed = 0;
    for (id, hash) in rows {
        let Some(status) = statuses.get(&hash) else {
            unlisted(pool, id).await?;
            continue;
        };
        sqlx::query(
            "UPDATE downloads SET missing_since = NULL WHERE id = ? AND missing_since IS NOT NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;
        if !clearing.seeded_enough(status) {
            continue;
        }
        match deluge.remove(&hash, clearing.with_data).await {
            Ok(true) => {
                let how = if clearing.with_data {
                    "with_data"
                } else {
                    "kept_data"
                };
                if mark(pool, id, how).await? {
                    tracing::info!(
                        download = id,
                        with_data = clearing.with_data,
                        "removed an imported torrent from Deluge"
                    );
                    removed += 1;
                }
            }
            Ok(false) => {
                mark(pool, id, "gone").await?;
            }
            Err(err) => tracing::warn!(
                download = id,
                error = %err,
                "couldn't remove an imported torrent from Deluge; trying again later"
            ),
        }
    }
    Ok(removed)
}

/// Notes that Deluge didn't list an imported torrent; once it hasn't for [`MISSING_FOR`], it's
/// gone (removed there by someone else) and isn't asked about again.
async fn unlisted(pool: &SqlitePool, id: i64) -> anyhow::Result<()> {
    let now = unix_now();
    sqlx::query(
        "UPDATE downloads SET missing_since = coalesce(missing_since, ?)
         WHERE id = ? AND state = 'imported'",
    )
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE downloads SET deluge_removed = 'gone', deluge_removed_at = ?
         WHERE id = ? AND state = 'imported' AND deluge_removed IS NULL AND missing_since <= ?",
    )
    .bind(now)
    .bind(id)
    .bind(now - MISSING_FOR)
    .execute(pool)
    .await?;
    Ok(())
}

/// Records what became of an imported torrent in Deluge; false when it was recorded already.
async fn mark(pool: &SqlitePool, id: i64, how: &str) -> anyhow::Result<bool> {
    let changed = sqlx::query(
        "UPDATE downloads SET deluge_removed = ?, deluge_removed_at = ?
         WHERE id = ? AND state = 'imported' AND deluge_removed IS NULL",
    )
    .bind(how)
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

#[cfg(test)]
mod tests;
