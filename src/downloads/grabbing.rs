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
}

/// Sends an accepted search result to Deluge and starts following it.
pub async fn grab(
    pool: &SqlitePool,
    deluge: &deluge::Client,
    fetcher: &Fetcher,
    release_id: i64,
) -> anyhow::Result<Result<Download, Refused>> {
    let row = match grabbable(pool, release_id).await? {
        Ok(row) => row,
        Err(refused) => return Ok(Err(refused)),
    };
    let Some(link) = row.download_url.as_deref() else {
        return Ok(Err(Refused::NoLink));
    };
    let sealing = crate::secrets::sealing(pool).await?;
    let link = crate::secrets::from_store(sealing, crate::secrets::DOWNLOAD_URL, link)?;
    let (torrent, hash) = match fetch_torrent(fetcher, &link).await? {
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
    let id = insert_download(pool, &row, &hash, &message).await?;
    // Another grab of the same torrent got there first.
    let Some(id) = id else {
        let id = existing(pool, &hash).await?.unwrap_or_default();
        return Ok(Err(Refused::Already(id)));
    };
    Ok(Ok(get(pool, id).await?.ok_or_else(|| {
        anyhow::anyhow!("download {id} vanished")
    })?))
}

/// The search result, when it's accepted, from an indexer and not blocklisted by its title.
async fn grabbable(
    pool: &SqlitePool,
    release_id: i64,
) -> anyhow::Result<Result<ReleaseRow, Refused>> {
    let row: Option<ReleaseRow> = sqlx::query_as(
        "SELECT d.title, d.download_url, d.accepted, d.decision, s.wanted_movie_id,
                s.wanted_series_id, s.wanted_album_id, s.season, s.episode,
                d.soulseek IS NOT NULL AS from_soulseek
         FROM release_decisions d JOIN searches s ON s.id = d.search_id
         WHERE d.id = ?",
    )
    .bind(release_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(Err(Refused::NoRelease));
    };
    if !row.accepted {
        let decision: serde_json::Value = serde_json::from_str(&row.decision)?;
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
    if crate::blocklist::blocks(pool, &row.title, None).await? {
        return Ok(Err(Refused::Blocklisted));
    }
    Ok(Ok(row))
}

/// The torrent behind a result's link, and its info hash.
async fn fetch_torrent(
    fetcher: &Fetcher,
    link: &str,
) -> anyhow::Result<Result<(Torrent, String), Refused>> {
    let torrent = match fetcher.fetch(link).await {
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
) -> anyhow::Result<Option<i64>> {
    // What it would replace in the library, should it be an upgrade.
    let replaces = crate::upgrades::replaced_by(
        pool,
        row.wanted_movie_id,
        row.wanted_series_id,
        row.season,
        &row.title,
    )
    .await?;
    let now = unix_now();
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO downloads (infohash, title, wanted_movie_id, wanted_series_id, wanted_album_id,
             season, episode, decision, state, message, added_at, updated_at, replaces, added_here)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'downloading', ?, ?, ?, ?, ?)
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
