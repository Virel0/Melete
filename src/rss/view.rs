//! The RSS syncs as the API shows them: one, or the most recent.

use super::*;

/// A sync as shown.
#[derive(Debug, Serialize)]
pub struct SyncView {
    /// The sync's row id.
    pub id: i64,
    /// When it ran, in Unix seconds.
    pub synced_at: i64,
    /// How each indexer's feed answered.
    pub indexers: Vec<IndexerOutcome>,
    /// Releases not seen in an earlier sync.
    pub new_releases: i64,
    /// The searches it stored: one per wanted title its new releases named.
    pub searches: Vec<SyncSearch>,
    /// What it grabbed or tried to.
    pub grabbed: Vec<Grabbed>,
}

/// A search a sync stored, in short: what it was for and how many releases it decided on.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SyncSearch {
    /// The search's row id, for its full results.
    pub search_id: i64,
    /// The wanted movie it was for; null for other kinds.
    pub wanted_movie_id: Option<i64>,
    /// The wanted series it was for; null for other kinds.
    pub wanted_series_id: Option<i64>,
    /// The wanted album it was for; null for other kinds.
    pub wanted_album_id: Option<i64>,
    /// The season the releases named, for a series.
    pub season: Option<i64>,
    /// The episode the releases named; null for a whole season.
    pub episode: Option<i64>,
    /// How many releases it decided on.
    pub releases: i64,
    /// How many of those the profile accepted.
    pub accepted: i64,
}

/// A sync by its id; `None` when there's no such sync.
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<SyncView>> {
    let row: Option<(i64, i64, String, i64, String)> = sqlx::query_as(
        "SELECT id, synced_at, indexers, new_releases, grabbed FROM rss_syncs WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((id, synced_at, indexers, new_releases, grabbed)) = row else {
        return Ok(None);
    };
    let searches: Vec<SyncSearch> = sqlx::query_as(
        "SELECT s.id AS search_id, s.wanted_movie_id, s.wanted_series_id, s.wanted_album_id,
                s.season, s.episode,
                count(d.id) AS releases, coalesce(sum(d.accepted), 0) AS accepted
         FROM searches s LEFT JOIN release_decisions d ON d.search_id = s.id
         WHERE s.rss_sync_id = ? GROUP BY s.id ORDER BY s.id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(Some(SyncView {
        id,
        synced_at,
        indexers: serde_json::from_str(&indexers)?,
        new_releases,
        searches,
        grabbed: serde_json::from_str(&grabbed)?,
    }))
}

/// The latest syncs, newest first.
pub async fn recent(pool: &SqlitePool, limit: i64) -> anyhow::Result<Vec<SyncView>> {
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM rss_syncs ORDER BY id DESC LIMIT ?")
        .bind(limit)
        .fetch_all(pool)
        .await?;
    let mut views = Vec::with_capacity(ids.len());
    for id in ids {
        views.extend(get(pool, id).await?);
    }
    Ok(views)
}
