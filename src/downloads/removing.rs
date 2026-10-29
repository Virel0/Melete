//! Taking downloads off the list, with only the hidden part files their copies home left.

use super::*;

/// Why a download wasn't removed.
#[derive(Debug, PartialEq, Eq)]
pub enum NotRemoved {
    NotFound,
    /// It's being copied home or is in the library, or a Soulseek download not yet failed:
    /// removing it now could leave half a copy, or lose what an upgrade replaces.
    Busy(String),
}

/// What a removal took away: the torrent's hash, for Deluge, when it was one.
#[derive(Debug, PartialEq, Eq)]
pub struct Removed {
    /// The torrent's hash, to remove it from Deluge too; None for a Soulseek download.
    pub infohash: Option<String>,
}

/// Takes a download off the list: a torrent still downloading or waiting on the
/// seedbox, or any download that failed. Nothing in a library is touched; the wanted title
/// stays wanted, so searches and RSS look for it again.
pub async fn removable(pool: &SqlitePool, id: i64) -> anyhow::Result<Result<Removed, NotRemoved>> {
    let row: Option<(String, String, Option<String>)> =
        sqlx::query_as("SELECT source, state, infohash FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((source, state, infohash)) = row else {
        return Ok(Err(NotRemoved::NotFound));
    };
    let allowed = state == "failed"
        || (source == "torrent" && matches!(state.as_str(), "downloading" | "completed"));
    if !allowed {
        return Ok(Err(NotRemoved::Busy(state)));
    }
    Ok(Ok(Removed { infohash }))
}

/// Removes a download [`removable`] allowed, if it's still in a state that allows it, and
/// deletes the hidden part files its copies home left in the library, only
/// those. Gone from the list first: a transfer can't take it up once it's gone, and one under
/// way keeps it from going.
pub async fn remove(pool: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    let removed: Option<String> = sqlx::query_scalar(
        "DELETE FROM downloads WHERE id = ?
           AND (state = 'failed' OR (source = 'torrent' AND state IN ('downloading', 'completed')))
         RETURNING partials",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(partials) = removed else {
        return Ok(false);
    };
    tidy_partials(pool, id, &partials).await;
    Ok(true)
}

/// Deletes a removed download's part files; what can't be is logged (scans skip them, so one
/// left behind is in the way of nothing).
pub(super) async fn tidy_partials(pool: &SqlitePool, id: i64, partials: &str) {
    let partials: Vec<String> = serde_json::from_str(partials).unwrap_or_default();
    if partials.is_empty() {
        return;
    }
    match crate::transfer::delete_partials(pool, id, &partials).await {
        Ok((deleted, left)) => {
            if deleted > 0 {
                tracing::info!(
                    download = id,
                    deleted,
                    "deleted a removed download's part files"
                );
            }
            if !left.is_empty() {
                tracing::warn!(download = id, ?left, "left a removed download's part files");
            }
        }
        Err(err) => tracing::warn!(
            download = id,
            error = format!("{err:#}"),
            "couldn't delete a removed download's part files"
        ),
    }
}

/// What failed downloads to clear: every one, or those for movies, series or music.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum For {
    Movies,
    Series,
    Music,
}

/// Removes every failed download, or only those of one kind, with their part files, as
/// [`remove`] does; returns how many.
pub async fn clear_failed(pool: &SqlitePool, only: Option<For>) -> anyhow::Result<u64> {
    let of = match only {
        None => "1",
        Some(For::Movies) => "wanted_movie_id IS NOT NULL",
        Some(For::Series) => "wanted_series_id IS NOT NULL",
        Some(For::Music) => "(wanted_album_id IS NOT NULL OR wanted_song_id IS NOT NULL)",
    };
    let removed: Vec<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "DELETE FROM downloads WHERE state = 'failed' AND {of} RETURNING id, partials"
    )))
    .fetch_all(pool)
    .await?;
    for (id, partials) in &removed {
        tidy_partials(pool, *id, partials).await;
    }
    Ok(removed.len() as u64)
}
