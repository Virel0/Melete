//! Stalled torrents: one Deluge says is downloading whose progress hasn't gone up for
//! [`STALLED_AFTER`] (since it was added, or last moved) is set aside and its title searched for
//! again, passing over its release. When another release is grabbed, the stalled torrent is
//! removed from Deluge with what it downloaded on the seedbox, and its release stays
//! blocklisted. Otherwise it goes back to downloading, so a rare title keeps its only copy, says
//! so, and is looked at again after as long. Paused and queued torrents aren't stalled; the
//! library is never touched.

use super::*;
use crate::search::Target;
use crate::transfer::{Again, search_again_for};

/// How long a downloading torrent may go without progress before another release is looked for.
pub const STALLED_AFTER: i64 = 4 * 3600;

/// How often downloads are looked at for stalls.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(sqlx::FromRow)]
struct Stalled {
    id: i64,
    infohash: String,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    season: Option<i64>,
    episode: Option<i64>,
    wanted_album_id: Option<i64>,
}

/// Looks for stalled torrents every few minutes, for as long as the server runs.
pub async fn watch_stalls(pool: SqlitePool, sources: crate::fulfil::Sources) {
    loop {
        tokio::time::sleep(CHECK_EVERY).await;
        if let Err(err) = replace_stalled(&pool, &sources).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't look for stalled downloads"
            );
        }
    }
}

/// Deals with each torrent stalled now; how many were replaced.
pub async fn replace_stalled(
    pool: &SqlitePool,
    sources: &crate::fulfil::Sources,
) -> anyhow::Result<usize> {
    let since = unix_now() - STALLED_AFTER;
    let stalled: Vec<Stalled> = sqlx::query_as(
        "SELECT id, infohash, wanted_movie_id, wanted_series_id, season, episode, wanted_album_id
         FROM downloads
         WHERE state = 'downloading' AND source = 'torrent' AND infohash IS NOT NULL
           AND missing_since IS NULL AND message LIKE 'Downloading%'
           AND coalesce(progress_at, added_at) <= ?1
           AND (stall_checked_at IS NULL OR stall_checked_at <= ?1)
         ORDER BY id",
    )
    .bind(since)
    .fetch_all(pool)
    .await?;
    let mut replaced = 0;
    for download in stalled {
        if replace(pool, sources, &download).await? {
            replaced += 1;
        }
    }
    Ok(replaced)
}

/// Sets one stalled torrent aside and searches again; whether another release was grabbed.
async fn replace(
    pool: &SqlitePool,
    sources: &crate::fulfil::Sources,
    download: &Stalled,
) -> anyhow::Result<bool> {
    let hours = STALLED_AFTER / 3600;
    // Failed for now, so the search doesn't count it as on its way.
    let set_aside = sqlx::query(
        "UPDATE downloads SET state = 'failed', message = ?, updated_at = ?
         WHERE id = ? AND state = 'downloading'",
    )
    .bind(format!(
        "no progress for {hours} hours; searching for another release"
    ))
    .bind(unix_now())
    .bind(download.id)
    .execute(pool)
    .await?
    .rows_affected();
    if set_aside == 0 {
        return Ok(false);
    }
    let title: String = sqlx::query_scalar("SELECT title FROM downloads WHERE id = ?")
        .bind(download.id)
        .fetch_one(pool)
        .await?;
    let listed_before = crate::blocklist::blocks(pool, &title, Some(&download.infohash)).await?;
    let entry = crate::blocklist::add(pool, crate::blocklist::From::Download(download.id)).await?;
    let again = match search(pool, sources, download).await {
        Ok(again) => again,
        Err(err) => Again::Not(format!("couldn't search again: {err:#}")),
    };
    match again {
        Again::Grabbed(other) => {
            drop_stalled(pool, sources, download, &other).await?;
            Ok(true)
        }
        Again::Not(why) => {
            if !listed_before && let Some(entry) = entry {
                crate::blocklist::remove(pool, entry.id).await?;
            }
            keep_going(pool, download.id, &why).await?;
            Ok(false)
        }
    }
}

/// Another release was grabbed: the stalled torrent goes from Deluge with its data, and stays
/// failed, its release blocklisted.
async fn drop_stalled(
    pool: &SqlitePool,
    sources: &crate::fulfil::Sources,
    download: &Stalled,
    other: &str,
) -> sqlx::Result<()> {
    let removed = match &sources.pipeline {
        Some(pipeline) => match pipeline.deluge.remove(&download.infohash, true).await {
            Ok(_) => "it was removed from Deluge with its data".to_owned(),
            Err(err) => format!("Deluge couldn't remove it ({err}), so remove it there"),
        },
        None => "downloads are off, so it's still in Deluge".to_owned(),
    };
    let hours = STALLED_AFTER / 3600;
    sqlx::query("UPDATE downloads SET message = ?, updated_at = ? WHERE id = ?")
        .bind(format!(
            "no progress for {hours} hours, so Melete grabbed {other} instead; {removed}, and the release is blocklisted"
        ))
        .bind(unix_now())
        .bind(download.id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Nothing else was grabbed: the torrent goes back to downloading, saying why, until it's
/// looked at again.
async fn keep_going(pool: &SqlitePool, id: i64, why: &str) -> sqlx::Result<()> {
    let hours = STALLED_AFTER / 3600;
    let note = format!(
        "no progress for {hours} hours, but {why}, so it keeps going and is looked at again in {hours} hours"
    );
    sqlx::query(
        "UPDATE downloads SET state = 'downloading', message = 'Downloading; ' || ?1,
             stall_note = ?1, stall_checked_at = ?2, updated_at = ?2
         WHERE id = ?3 AND state = 'failed'",
    )
    .bind(note)
    .bind(unix_now())
    .bind(id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Searches again for what the download was for: its movie or season, or its album.
async fn search(
    pool: &SqlitePool,
    sources: &crate::fulfil::Sources,
    download: &Stalled,
) -> anyhow::Result<Again> {
    let target = match (
        download.wanted_movie_id,
        download.wanted_series_id,
        download.season,
    ) {
        (Some(id), _, _) => Some(Target::Movie(id)),
        (None, Some(id), Some(season)) => Some(Target::Series {
            id,
            season,
            episode: download.episode,
        }),
        _ => None,
    };
    if let Some(target) = target {
        return search_again_for(pool, sources, target).await;
    }
    match download.wanted_album_id {
        Some(album) => search_album(pool, sources, album).await,
        None => Ok(Again::Not(
            "it isn't for a wanted title any more, so nothing was searched for".into(),
        )),
    }
}

/// Searches again for a wanted album, still monitored, and grabs the best of what's found.
async fn search_album(
    pool: &SqlitePool,
    sources: &crate::fulfil::Sources,
    album: i64,
) -> anyhow::Result<Again> {
    let monitored: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM wanted_albums WHERE id = ? AND monitored = 1)",
    )
    .bind(album)
    .fetch_one(pool)
    .await?;
    if !monitored {
        return Ok(Again::Not(
            "its album isn't monitored, so it wasn't searched for again".into(),
        ));
    }
    let client = crate::torznab::Client::new()?;
    let search_id =
        match crate::album_search::search(pool, &client, sources.slskd.as_deref(), album).await? {
            Ok(search_id) => search_id,
            Err(refused) => return Ok(Again::Not(format!("couldn't search again: {refused}"))),
        };
    let music = crate::artist_search::Sources {
        slskd: sources.slskd.clone(),
        pipeline: sources.pipeline.clone(),
    };
    let mut grabbed = Vec::new();
    crate::artist_search::grab_best(pool, &music, search_id, &mut grabbed).await?;
    Ok(
        match grabbed.into_iter().find(|g| g.download_id.is_some()) {
            Some(g) => Again::Grabbed(g.title),
            None => Again::Not("searched again; nothing else to grab yet".into()),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Secret;
    use crate::rss::tests::{indexer, releases, setup, want_dune};

    const BEST: &str =
        "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR";
    const NEXT: &str = "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX";

    async fn torrents() -> (crate::fulfil::Sources, deluge::testing::Shared) {
        let (url, fake) = deluge::testing::start().await;
        let pipeline = Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        };
        let sources = crate::fulfil::Sources {
            pipeline: Some(std::sync::Arc::new(pipeline)),
            ..crate::fulfil::Sources::default()
        };
        (sources, fake)
    }

    /// Dune wanted, the best of `listed` grabbed, and its download stuck at 0% for `hours`.
    async fn stuck(
        listed: &[&str],
        hours: i64,
    ) -> (
        SqlitePool,
        crate::fulfil::Sources,
        deluge::testing::Shared,
        i64,
    ) {
        let pool = setup().await;
        let dune = want_dune(&pool).await;
        let feed = indexer(&pool, "[2000]").await;
        let items: Vec<(&str, &str)> = listed.iter().map(|t| (*t, "")).collect();
        feed.lock().unwrap().0 = releases(&items);
        let (sources, fake) = torrents().await;
        let first = search_again_for(&pool, &sources, Target::Movie(dune))
            .await
            .unwrap();
        assert!(matches!(first, Again::Grabbed(ref t) if t == BEST));
        let id: i64 = sqlx::query_scalar(
            "UPDATE downloads SET message = 'Downloading', added_at = ? WHERE title = ? RETURNING id",
        )
        .bind(unix_now() - hours * 3600)
        .bind(BEST)
        .fetch_one(&pool)
        .await
        .unwrap();
        (pool, sources, fake, id)
    }

    async fn row(pool: &SqlitePool, id: i64) -> (String, String) {
        sqlx::query_as("SELECT state, message FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn blocked(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT title FROM blocklist")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_torrent_stuck_for_hours_is_replaced_by_another_release() {
        let (pool, sources, fake, id) = stuck(&[NEXT, BEST], 5).await;
        let hash: String = sqlx::query_scalar("SELECT infohash FROM downloads WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(replace_stalled(&pool, &sources).await.unwrap(), 1);

        let (state, message) = row(&pool, id).await;
        assert_eq!(state, "failed");
        assert!(
            message.contains(&format!("grabbed {NEXT} instead"))
                && message.contains("removed from Deluge with its data"),
            "{message}"
        );
        assert_eq!(fake.lock().unwrap().data_removed, [hash]);
        assert_eq!(blocked(&pool).await, [BEST]);
        let now: Vec<(String, String)> =
            sqlx::query_as("SELECT title, state FROM downloads WHERE id <> ?")
                .bind(id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(now, [(NEXT.to_owned(), "downloading".to_owned())]);
    }

    #[tokio::test]
    async fn with_nothing_else_to_grab_it_keeps_going_and_says_so() {
        let (pool, sources, fake, id) = stuck(&[BEST], 5).await;

        assert_eq!(replace_stalled(&pool, &sources).await.unwrap(), 0);

        let (state, message) = row(&pool, id).await;
        assert_eq!(state, "downloading");
        assert!(
            message.starts_with("Downloading; no progress for 4 hours")
                && message.contains("nothing else to grab yet")
                && message.ends_with("looked at again in 4 hours"),
            "{message}"
        );
        assert!(fake.lock().unwrap().data_removed.is_empty());
        assert!(
            blocked(&pool).await.is_empty(),
            "its release isn't blocklisted"
        );
        // Not looked at again until another 4 hours have gone by.
        assert_eq!(replace_stalled(&pool, &sources).await.unwrap(), 0);
        assert_eq!(
            fake.lock()
                .unwrap()
                .calls
                .iter()
                .filter(|c| c.contains("remove"))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn a_moving_paused_or_young_torrent_is_left_alone() {
        let (pool, sources, _fake, id) = stuck(&[NEXT, BEST], 5).await;
        for (message, added_hours, moved_hours) in [
            ("Downloading", 5, Some(1)),
            ("Paused", 5, None),
            ("Queued", 5, None),
            ("Downloading", 3, None),
        ] {
            sqlx::query(
                "UPDATE downloads SET message = ?, added_at = ?, progress_at = ? WHERE id = ?",
            )
            .bind(message)
            .bind(unix_now() - added_hours * 3600)
            .bind(moved_hours.map(|h: i64| unix_now() - h * 3600))
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
            assert_eq!(
                replace_stalled(&pool, &sources).await.unwrap(),
                0,
                "{message}"
            );
            assert_eq!(row(&pool, id).await.0, "downloading", "{message}");
        }
        assert!(blocked(&pool).await.is_empty());
    }
}
