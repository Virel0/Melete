//! RSS sync: every 15 minutes each turned-on indexer is asked for its newest releases. Those
//! naming a monitored wanted title are decided on as a search's results are, and kept with their
//! reasons, so it can be shown why one was grabbed or not. With downloads on, the best accepted
//! release for a wanted title is grabbed when nothing is on disk where it goes and nothing is
//! downloading, or when it's an upgrade of exactly the files there with nothing on the way or
//! unsettled (the files it replaces go to the trash once it's confirmed). Each release is looked
//! at once.
//!
//! An RSS release arrives unasked, so it's checked harder than a search result: one that fits
//! more than one wanted title is refused for all of them, and one whose indexer gives an id
//! that isn't the wanted title's is refused.
//!
//! Wanted albums are looked out for the same way: when any are monitored, each
//! indexer is also asked for its newest music releases, which are checked against the albums
//! as album searches' results are, and the best accepted one is grabbed for an album with
//! nothing on disk and nothing downloading. Album upgrades come later.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::album_search;
use crate::db::unix_now;
use crate::downloads::{self, Pipeline, Refused};
use crate::music_profiles;
use crate::release::{self, Episodes, Release};
use crate::search::{self, IndexerOutcome, Target, Title, Wanted};
use crate::torznab;

mod asking;
mod deciding;
mod grabbing;
mod matching;
mod view;

use asking::*;
use deciding::*;
pub use grabbing::*;
use matching::*;
pub use view::*;

/// How often indexers are asked for their newest releases.
pub const EVERY: Duration = Duration::from_secs(15 * 60);
/// Syncs and the releases they saw are kept this long.
const KEEP_SECONDS: i64 = 30 * 24 * 3600;
/// Accepted releases tried, best first, when grabbing for one title.
const GRAB_TRIES: i64 = 3;

/// One sync at a time: the scheduled one and one asked for through the API.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Syncs every [`EVERY`] for as long as the server runs.
pub async fn run(
    pool: SqlitePool,
    tmdb: Option<Arc<crate::tmdb::Client>>,
    pipeline: Option<Arc<Pipeline>>,
) {
    let client = match torznab::Client::new() {
        Ok(client) => client,
        Err(err) => {
            tracing::error!(error = format!("{err:#}"), "RSS sync can't start");
            return;
        }
    };
    loop {
        // Not at once: the startup scan should have the library in first.
        crate::tasks::scheduled("rss-sync", EVERY);
        tokio::time::sleep(EVERY).await;
        crate::tasks::started("rss-sync");
        let synced = sync(&pool, &client, tmdb.as_deref(), pipeline.as_deref()).await;
        let result = synced.as_ref().err().and_then(|_| crate::tasks::failed());
        crate::tasks::finished("rss-sync", result, Some(EVERY));
        match synced {
            Ok(Some(view)) if !view.grabbed.is_empty() => {
                tracing::info!(
                    sync = view.id,
                    grabbed = view.grabbed.len(),
                    "RSS sync grabbed releases"
                )
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = format!("{err:#}"), "RSS sync failed"),
        }
    }
}

/// Runs a sync and keeps it; `None` when no indexer is turned on.
pub async fn sync(
    pool: &SqlitePool,
    client: &torznab::Client,
    tmdb: Option<&crate::tmdb::Client>,
    pipeline: Option<&Pipeline>,
) -> anyhow::Result<Option<SyncView>> {
    let _one_at_a_time = RUNNING.lock().await;
    let indexers = search::enabled_indexers(pool).await?;
    if indexers.is_empty() {
        return Ok(None);
    }
    let albums = album_search::monitored(pool).await?;
    let outcomes = ask_indexers(client, &indexers, !albums.is_empty()).await?;
    let fresh = fresh_releases(pool, &outcomes).await?;
    let candidates = wanted_candidates(pool).await?;
    let groups = group_by_title(pool, &candidates, &fresh).await?;
    let album_groups = group_by_album(&albums, &fresh);

    let now = unix_now();
    let summary: Vec<&IndexerOutcome> = outcomes.iter().map(|(o, _)| o).collect();
    let sync_id = start_record(pool, now, &summary, fresh.len()).await?;
    let stored = decide_titles(pool, tmdb, &candidates, &groups, sync_id, &summary).await?;
    let stored_albums = decide_albums(pool, &albums, &album_groups, sync_id, &summary).await?;
    let grabbed = match pipeline {
        Some(pipeline) => grab_all(pool, pipeline, &candidates, &stored, &stored_albums).await?,
        None => Vec::new(),
    };
    finish_record(pool, sync_id, &grabbed, &fresh, now).await?;
    get(pool, sync_id).await
}

/// Records a sync as it starts: when, what each indexer answered and how many releases were new.
async fn start_record(
    pool: &SqlitePool,
    now: i64,
    summary: &[&IndexerOutcome],
    new_releases: usize,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO rss_syncs (synced_at, indexers, new_releases, grabbed)
         VALUES (?, ?, ?, '[]') RETURNING id",
    )
    .bind(now)
    .bind(serde_json::to_string(&summary)?)
    .bind(new_releases as i64)
    .fetch_one(pool)
    .await?)
}

/// Records what a sync grabbed and that its new releases were looked at, and forgets syncs and
/// releases older than [`KEEP_SECONDS`], in one transaction.
async fn finish_record(
    pool: &SqlitePool,
    sync_id: i64,
    grabbed: &[Grabbed],
    fresh: &Fresh<'_>,
    now: i64,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE rss_syncs SET grabbed = ? WHERE id = ?")
        .bind(serde_json::to_string(&grabbed)?)
        .bind(sync_id)
        .execute(&mut *tx)
        .await?;
    for (indexer_id, _, key) in fresh {
        sqlx::query(
            "INSERT INTO rss_seen (indexer_id, release_key, seen_at) VALUES (?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(indexer_id)
        .bind(key)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM rss_seen WHERE seen_at < ?")
        .bind(now - KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rss_syncs WHERE synced_at < ?")
        .bind(now - KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
