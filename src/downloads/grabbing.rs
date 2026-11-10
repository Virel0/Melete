//! Sending an accepted search result to Deluge, grabbing each torrent hash at most once.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct ReleaseRow {
    pub(super) title: String,
    pub(super) download_url: Option<String>,
    pub(super) accepted: bool,
    pub(super) decision: String,
    pub(super) wanted_movie_id: Option<i64>,
    pub(super) wanted_series_id: Option<i64>,
    pub(super) wanted_album_id: Option<i64>,
    pub(super) season: Option<i64>,
    pub(super) episode: Option<i64>,
    pub(super) from_soulseek: bool,
    /// Whether the indexer that found it goes through the proxy; None when it's been removed.
    pub(super) through_proxy: Option<bool>,
}

/// Sends an accepted search result to Deluge and starts following it. Its torrent is fetched
/// the way the indexer that found it goes; one whose indexer was removed, through the proxy
/// whenever one is set, as a way not known can't be taken to be direct.
pub async fn grab(
    pool: &SqlitePool,
    deluge: &deluge::Client,
    fetcher: &Fetcher,
    release_id: i64,
) -> anyhow::Result<Result<Download, Refused>> {
    grab_only(pool, deluge, fetcher, release_id, None).await
}

/// The episodes of its season a season pack is taken for, and the files their copies replace;
/// for a pack of several seasons, its other seasons taken with it and their episodes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Only<'a> {
    pub(crate) episodes: &'a [i64],
    pub(crate) replaces: &'a [crate::upgrades::Replaced],
    pub(crate) others: &'a [(i64, Vec<i64>)],
}

/// Like [`grab`], for only some of a season pack's episodes when `only` says which: only their
/// files are brought home, and only the files given are replaced.
pub(crate) async fn grab_only(
    pool: &SqlitePool,
    deluge: &deluge::Client,
    fetcher: &Fetcher,
    release_id: i64,
    only: Option<Only<'_>>,
) -> anyhow::Result<Result<Download, Refused>> {
    grab_with(pool, deluge, fetcher, release_id, only, None).await
}

/// Like [`grab`], for a single episode a season's search found, recorded as `episode`'s (its
/// first, for one of several) rather than the season's.
pub(crate) async fn grab_episode(
    pool: &SqlitePool,
    deluge: &deluge::Client,
    fetcher: &Fetcher,
    release_id: i64,
    episode: i64,
) -> anyhow::Result<Result<Download, Refused>> {
    grab_with(pool, deluge, fetcher, release_id, None, Some(episode)).await
}

async fn grab_with(
    pool: &SqlitePool,
    deluge: &deluge::Client,
    fetcher: &Fetcher,
    release_id: i64,
    only: Option<Only<'_>>,
    episode: Option<i64>,
) -> anyhow::Result<Result<Download, Refused>> {
    // No series changes episode order between checking the result was searched for in the order
    // it follows and recording the download with the season and episode it was searched as.
    let _no_switching = crate::episode_order::SWITCHING.read().await;
    let mut row = match grabbable(pool, release_id, episode.is_some()).await? {
        Ok(row) => row,
        Err(refused) => return Ok(Err(refused)),
    };
    if episode.is_some() {
        row.episode = episode;
    }
    let Some(link) = row.download_url.as_deref() else {
        return Ok(Err(Refused::NoLink));
    };
    let sealing = crate::secrets::sealing(pool).await?;
    let link = crate::secrets::from_store(sealing, crate::secrets::DOWNLOAD_URL, link)?;
    let through = row
        .through_proxy
        .unwrap_or_else(|| crate::proxy::current().setup.at.is_some());
    let (torrent, hash) = match fetch_torrent(fetcher, &link, through).await? {
        Ok(fetched) => fetched,
        Err(refused) => return Ok(Err(refused)),
    };
    if let Some(id) = existing(pool, &hash).await? {
        return Ok(Err(Refused::Already(id)));
    }
    // The same torrent under another name.
    if crate::blocklist::blocks(pool, &row.title, Some(&hash)).await? {
        return Ok(Err(Refused::Blocklisted));
    }
    let (hash, message) = match add_to_deluge(deluge, &torrent, &hash).await? {
        Ok(added) => added,
        Err(refused) => return Ok(Err(refused)),
    };
    let id = insert_download(pool, &row, &hash, &message, only).await?;
    // Another grab of the same torrent got there first.
    let Some(id) = id else {
        let id = existing(pool, &hash).await?.unwrap_or_default();
        return Ok(Err(Refused::Already(id)));
    };
    super::started();
    Ok(Ok(get(pool, id).await?.ok_or_else(|| {
        anyhow::anyhow!("download {id} vanished")
    })?))
}

/// Whether a decision's only refusal is that it's single episodes of the season searched for:
/// as its episode's, it's accepted.
pub(crate) fn single_episodes_only(decision: &serde_json::Value) -> bool {
    decision["rejections"]
        .as_array()
        .is_some_and(|r| r.len() == 1 && r[0] == crate::search::SINGLE_EPISODES)
}

/// The search result, when it's accepted (or, `as_episode`, refused only for being single
/// episodes of a season searched for), from an indexer and not blocklisted by its title.
async fn grabbable(
    pool: &SqlitePool,
    release_id: i64,
    as_episode: bool,
) -> anyhow::Result<Result<ReleaseRow, Refused>> {
    let row: Option<ReleaseRow> = sqlx::query_as(
        "SELECT d.title, d.download_url, d.accepted, d.decision, s.wanted_movie_id,
                s.wanted_series_id, s.wanted_album_id, s.season, s.episode,
                d.soulseek IS NOT NULL AS from_soulseek,
                (SELECT i.proxy FROM indexers i WHERE i.id = d.indexer_id) AS through_proxy
         FROM release_decisions d JOIN searches s ON s.id = d.search_id
         WHERE d.id = ?",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(Err(Refused::NoRelease));
    };
    let decision: serde_json::Value = serde_json::from_str(&row.decision)?;
    if !row.accepted && !(as_episode && single_episodes_only(&decision)) {
        // A video decision's rejections, or an album decision's reasons.
        let why = decision["rejections"][0]
            .as_str()
            .or_else(|| decision["reasons"][0].as_str())
            .unwrap_or("no reason recorded")
            .to_owned();
        return Ok(Err(Refused::NotAccepted(why)));
    }
    if row.from_soulseek {
        return Ok(Err(Refused::Soulseek));
    }
    if reordered_since(pool, release_id).await? {
        return Ok(Err(Refused::Reordered));
    }
    if crate::blocklist::blocks(pool, &row.title, None).await? {
        return Ok(Err(Refused::Blocklisted));
    }
    Ok(Ok(row))
}

/// Whether the result is a series search's from before the series last changed episode order
/// (or one whose episode the order leaves out): it was judged as an episode numbered another
/// way, so grabbing it could bring home the wrong episode.
async fn reordered_since(pool: &SqlitePool, release_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT s.season IS NULL
             OR EXISTS (SELECT 1 FROM series_orders o
                        WHERE o.series_tmdb_id = coalesce(w.tmdb_id,
                                  (SELECT t.tmdb_id FROM tmdb_series t WHERE t.tvdb_id = w.tvdb_id
                                   ORDER BY t.tmdb_id LIMIT 1))
                          AND o.chosen_at >= s.searched_at)
         FROM release_decisions d JOIN searches s ON s.id = d.search_id
         JOIN wanted_series w ON w.id = s.wanted_series_id
         WHERE d.id = ?",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await
    .map(|found| found.unwrap_or(false))
}

/// The torrent behind a result's link, and its info hash.
async fn fetch_torrent(
    fetcher: &Fetcher,
    link: &str,
    through: bool,
) -> anyhow::Result<Result<(Torrent, String), Refused>> {
    let torrent = match fetcher.fetch(link, through).await {
        Ok(torrent) => torrent,
        Err(why) => return Ok(Err(Refused::Fetch(why))),
    };
    let hash = match &torrent {
        Torrent::File(bytes) => deluge::torrent_hash(bytes),
        Torrent::Magnet(uri) => deluge::magnet_hash(uri),
    };
    let Some(hash) = hash else {
        return Ok(Err(Refused::Fetch(
            "the magnet link has no info hash".into(),
        )));
    };
    Ok(Ok((torrent, hash)))
}

/// Adds the torrent to Deluge: the name Deluge follows it by, and a message when it was there
/// already.
async fn add_to_deluge(
    deluge: &deluge::Client,
    torrent: &Torrent,
    hash: &str,
) -> anyhow::Result<Result<(String, Option<String>), Refused>> {
    let added = match torrent {
        Torrent::File(bytes) => {
            deluge
                .add_torrent_file(&format!("{hash}.torrent"), bytes)
                .await
        }
        Torrent::Magnet(uri) => deluge.add_magnet(uri).await,
    };
    // Deluge's own name for the torrent is what it's followed by: for hybrid and v2 torrents
    // it isn't the SHA-1 computed above.
    let (hash, message) = match added {
        Ok(named) => (named, None),
        // Added before (by Radarr, say): follow that one.
        Err(err) => match deluge::already_added(&err) {
            Some(named) => (named, Some("it was already in Deluge".to_owned())),
            None => return Ok(Err(Refused::Deluge(err.to_string()))),
        },
    };
    Ok(Ok((hash, message)))
}

/// Records the download, with what it would replace in the library, and whether Melete added it
/// to Deluge (no `message` saying it was there already); None when another grab of the same
/// torrent recorded it first.
async fn insert_download(
    pool: &SqlitePool,
    row: &ReleaseRow,
    hash: &str,
    message: &Option<String>,
    only: Option<Only<'_>>,
) -> anyhow::Result<Option<i64>> {
    // What it would replace in the library, should it be an upgrade.
    let replaces = match only {
        Some(only) => only.replaces.to_vec(),
        None => {
            crate::upgrades::replaced_by(
                pool,
                row.wanted_movie_id,
                row.wanted_series_id,
                row.season,
                &row.title,
            )
            .await?
        }
    };
    let episodes = only
        .map(|o| serde_json::to_string(o.episodes))
        .transpose()?;
    let seasons = only
        .filter(|o| !o.others.is_empty())
        .map(|o| {
            let by_season: serde_json::Map<String, serde_json::Value> = o
                .others
                .iter()
                .map(|(season, episodes)| (season.to_string(), serde_json::json!(episodes)))
                .collect();
            serde_json::to_string(&by_season)
        })
        .transpose()?;
    let now = unix_now();
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO downloads (infohash, title, wanted_movie_id, wanted_series_id, wanted_album_id,
             season, episode, decision, state, message, added_at, updated_at, replaces, added_here,
             episodes, seasons)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'downloading', ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (infohash) DO NOTHING
         RETURNING id",
    )
    .bind(hash)
    .bind(&row.title)
    .bind(row.wanted_movie_id)
    .bind(row.wanted_series_id)
    .bind(row.wanted_album_id)
    .bind(row.season)
    .bind(row.episode)
    .bind(&row.decision)
    .bind(message)
    .bind(now)
    .bind(now)
    .bind(serde_json::to_string(&replaces)?)
    .bind(message.is_none())
    .bind(episodes)
    .bind(seasons)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

pub(super) async fn existing(pool: &SqlitePool, hash: &str) -> anyhow::Result<Option<i64>> {
    Ok(
        sqlx::query_scalar("SELECT id FROM downloads WHERE infohash = ?")
            .bind(hash)
            .fetch_optional(pool)
            .await?,
    )
}
