//! Keeping a search and its decisions.

use super::*;

/// Stores a search (from an RSS sync, with its id) with its decisions; returns its id. It's kept
/// as searched when it `started`, before its target was worked out: a series' result is grabbed
/// only when the series hasn't changed episode order since.
pub(crate) async fn store(
    pool: &SqlitePool,
    target: Target,
    profile_id: i64,
    rss_sync_id: Option<i64>,
    summary: &[&IndexerOutcome],
    decided: &[(i64, &torznab::Item, Decision)],
    started: i64,
) -> anyhow::Result<i64> {
    let now = unix_now();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM searches WHERE searched_at < ?")
        .bind(now - KEEP_SECONDS)
        .execute(&mut *tx)
        .await?;
    let (movie_id, series_id, season, episode) = match target {
        Target::Movie(id) => (Some(id), None, None, None),
        Target::Series {
            id,
            season,
            episode,
        } => (None, Some(id), Some(season), episode),
    };
    let search_id: i64 = sqlx::query_scalar(
        "INSERT INTO searches (wanted_movie_id, wanted_series_id, season, episode, profile_id,
             searched_at, indexers, rss_sync_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(movie_id)
    .bind(series_id)
    .bind(season)
    .bind(episode)
    .bind(profile_id)
    .bind(started.min(now))
    .bind(serde_json::to_string(&summary)?)
    .bind(rss_sync_id)
    .fetch_one(&mut *tx)
    .await?;
    let sealing = crate::secrets::sealing(&mut *tx).await?;
    let link =
        |what, link: &Option<String>| crate::secrets::link_to_store(sealing, what, link.as_deref());
    for (indexer_id, item, decision) in decided {
        sqlx::query(
            "INSERT INTO release_decisions (search_id, indexer_id, title, size, seeders, peers,
                     published, infohash, download_url, guid, accepted, decision)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(search_id)
        .bind(indexer_id)
        .bind(&item.title)
        .bind(item.size.and_then(|s| i64::try_from(s).ok()))
        .bind(item.seeders)
        .bind(item.peers)
        .bind(item.published)
        .bind(&item.infohash)
        .bind(link(crate::secrets::DOWNLOAD_URL, &item.download_url)?)
        .bind(link(crate::secrets::GUID, &item.guid)?)
        .bind(decision.accepted)
        .bind(serde_json::to_string(&decision)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(search_id)
}
