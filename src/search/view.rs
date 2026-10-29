//! A stored search with its results and reasons, as the API shows it.

use super::*;

/// A stored search: what it was for, how each indexer answered, and every result.
#[derive(Debug, Serialize)]
pub struct SearchView {
    /// The search's row id.
    pub id: i64,
    /// The RSS sync that stored it; null for a search.
    pub rss_sync_id: Option<i64>,
    /// The wanted movie it was for; null for other kinds.
    pub wanted_movie_id: Option<i64>,
    /// The wanted series it was for; null for other kinds.
    pub wanted_series_id: Option<i64>,
    /// The wanted album it was for; null for other kinds.
    pub wanted_album_id: Option<i64>,
    /// The wanted song it was for; null for other kinds.
    pub wanted_song_id: Option<i64>,
    /// The season searched, for a series.
    pub season: Option<i64>,
    /// The episode searched; null for a whole season.
    pub episode: Option<i64>,
    /// The quality profile a movie or series search decided with; null once that profile is
    /// removed.
    pub profile_id: Option<i64>,
    /// An album or song search's music profile.
    pub music_profile_id: Option<i64>,
    /// When it ran, in Unix seconds.
    pub searched_at: i64,
    /// How each indexer answered.
    pub indexers: Vec<IndexerOutcome>,
    /// How asking Soulseek went; None when it wasn't asked.
    pub soulseek: Option<crate::soulseek::Outcome>,
    /// Accepted first, then by quality rank, score and seeders.
    pub releases: Vec<ReleaseView>,
}

/// A result as shown: without its download link or guid.
#[derive(Debug, Serialize)]
pub struct ReleaseView {
    /// The result's row id (its decision's), which grabbing and blocklisting take.
    pub id: i64,
    /// "torrent", or "soulseek" for a Soulseek user's album folder.
    pub source: &'static str,
    /// The indexer that listed it; null for Soulseek, or once that indexer is removed.
    pub indexer_id: Option<i64>,
    /// The release name.
    pub title: String,
    /// Its size in bytes.
    pub size: Option<i64>,
    /// Its seeders, as the indexer counted them.
    pub seeders: Option<i64>,
    /// Its peers, as the indexer counted them.
    pub peers: Option<i64>,
    /// When the indexer says it was posted, in Unix seconds.
    pub published: Option<i64>,
    /// Whether the profile accepts it for the title.
    pub accepted: bool,
    /// The decision with its reasons, as stored when it was made.
    pub decision: serde_json::Value,
    /// On the blocklist by its name: grabbing it is refused.
    pub blocklisted: bool,
}

#[derive(sqlx::FromRow)]
pub(super) struct SearchRow {
    pub(super) id: i64,
    pub(super) rss_sync_id: Option<i64>,
    pub(super) wanted_movie_id: Option<i64>,
    pub(super) wanted_series_id: Option<i64>,
    pub(super) wanted_album_id: Option<i64>,
    pub(super) wanted_song_id: Option<i64>,
    pub(super) season: Option<i64>,
    pub(super) episode: Option<i64>,
    pub(super) profile_id: Option<i64>,
    pub(super) music_profile_id: Option<i64>,
    pub(super) searched_at: i64,
    pub(super) indexers: String,
    pub(super) soulseek: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(super) struct DecisionRow {
    pub(super) id: i64,
    pub(super) indexer_id: Option<i64>,
    pub(super) title: String,
    pub(super) size: Option<i64>,
    pub(super) seeders: Option<i64>,
    pub(super) peers: Option<i64>,
    pub(super) published: Option<i64>,
    pub(super) accepted: bool,
    pub(super) decision: String,
    pub(super) from_soulseek: bool,
    pub(super) blocklisted: bool,
}

/// A stored search with its results; `None` when there's no such search (or it was removed
/// after 30 days).
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<SearchView>> {
    let row: Option<SearchRow> = sqlx::query_as(
        "SELECT id, rss_sync_id, wanted_movie_id, wanted_series_id, wanted_album_id,
                wanted_song_id, season, episode, profile_id, music_profile_id, searched_at,
                indexers, soulseek
         FROM searches WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let releases: Vec<DecisionRow> = sqlx::query_as(
        "SELECT id, indexer_id, title, size, seeders, peers, published, accepted, decision,
                soulseek IS NOT NULL AS from_soulseek,
                lower(title) IN (SELECT lower(title) FROM blocklist) AS blocklisted
         FROM release_decisions WHERE search_id = ?
         ORDER BY accepted DESC, json_extract(decision, '$.rank') DESC NULLS LAST,
                  json_extract(decision, '$.score') DESC, seeders DESC NULLS LAST, id",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    Ok(Some(SearchView {
        id: row.id,
        rss_sync_id: row.rss_sync_id,
        wanted_movie_id: row.wanted_movie_id,
        wanted_series_id: row.wanted_series_id,
        wanted_album_id: row.wanted_album_id,
        wanted_song_id: row.wanted_song_id,
        season: row.season,
        episode: row.episode,
        profile_id: row.profile_id,
        music_profile_id: row.music_profile_id,
        searched_at: row.searched_at,
        indexers: serde_json::from_str(&row.indexers)?,
        soulseek: row
            .soulseek
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        releases: releases
            .into_iter()
            .map(|r| {
                Ok(ReleaseView {
                    id: r.id,
                    source: if r.from_soulseek {
                        "soulseek"
                    } else {
                        "torrent"
                    },
                    indexer_id: r.indexer_id,
                    title: r.title,
                    size: r.size,
                    seeders: r.seeders,
                    peers: r.peers,
                    published: r.published,
                    accepted: r.accepted,
                    decision: serde_json::from_str(&r.decision)?,
                    blocklisted: r.blocklisted,
                })
            })
            .collect::<anyhow::Result<_>>()?,
    }))
}
