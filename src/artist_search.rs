//! Searching a whole artist: every monitored wanted album of the
//! artist that isn't on disk or downloading is searched for (indexers and Soulseek together,
//! as one album's search is) and, unless asked not to, the best accepted result is grabbed,
//! through Deluge or slskd, trying the next best when one can't be fetched. Albums are taken one
//! at a time, oldest first, and runs one after another, since slskd searches one at a time; a
//! run records each album as it's dealt with, so it can be watched.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::album_search;
use crate::db::unix_now;
use crate::downloads::{self, Pipeline, Refused};
use crate::rss::Grabbed;
use crate::{slskd, soulseek_downloads, torznab};

/// Accepted releases tried, best first, for one album.
const GRAB_TRIES: i64 = 3;

/// Runs go one after another: the scheduled ones share slskd and the indexers.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What a run needs from outside.
#[derive(Clone)]
pub struct Sources {
    /// slskd, for Soulseek; none when it isn't set up.
    pub slskd: Option<Arc<slskd::Client>>,
    /// Deluge, for torrents; None when downloads are off.
    pub pipeline: Option<Arc<Pipeline>>,
}

/// What happened to one album.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumRun {
    /// The wanted album's row id.
    pub wanted_album_id: i64,
    /// The album's title.
    pub title: String,
    /// Why it wasn't searched for: it's on disk or downloading.
    pub skipped: Option<String>,
    /// The search run for it; none when it was skipped or the search couldn't start.
    pub search_id: Option<i64>,
    /// Why the search couldn't run.
    pub error: Option<String>,
    /// What was grabbed for it; empty when nothing was, or the run was told not to grab.
    pub grabbed: Vec<Grabbed>,
}

/// One run over a wanted artist's albums, as stored and shown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArtistSearch {
    /// The run's row id.
    pub id: i64,
    /// The wanted artist it's for.
    pub wanted_artist_id: i64,
    /// Whether the best accepted result for each album is grabbed, or only searched for.
    pub grab: bool,
    /// `queued` (waiting for another run), `running`, `done` or `failed`.
    pub state: String,
    /// Each album as it was dealt with, in order.
    pub albums: Vec<AlbumRun>,
    /// The closing message, or why it failed; none while it runs.
    pub message: Option<String>,
    /// When it was asked for, in seconds since the epoch.
    pub started_at: i64,
    /// When it ended, in seconds since the epoch; none while queued or running.
    pub finished_at: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    wanted_artist_id: i64,
    grab: bool,
    state: String,
    albums: String,
    message: Option<String>,
    started_at: i64,
    finished_at: Option<i64>,
}

/// A run by its id; none when there's no such run.
///
/// # Errors
///
/// Fails when the database can't be read or the run's stored albums aren't valid JSON.
pub async fn get(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<ArtistSearch>> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, wanted_artist_id, grab, state, albums, message, started_at, finished_at
         FROM artist_searches WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(|r| {
        Ok(ArtistSearch {
            id: r.id,
            wanted_artist_id: r.wanted_artist_id,
            grab: r.grab,
            state: r.state,
            albums: serde_json::from_str(&r.albums)?,
            message: r.message,
            started_at: r.started_at,
            finished_at: r.finished_at,
        })
    })
    .transpose()
}

/// Why a run wasn't started.
#[derive(Debug, PartialEq)]
pub enum NotStarted {
    NoArtist,
    /// One for this artist is queued or running: its id.
    Already(i64),
}

/// Marks runs a restart cut off.
pub async fn recover(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE artist_searches SET state = 'failed', message = 'a restart cut it off',
             finished_at = ?
         WHERE state IN ('queued', 'running')",
    )
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Starts a run for a wanted artist in the background; returns it as it starts.
pub async fn start(
    pool: &SqlitePool,
    sources: Sources,
    wanted_artist_id: i64,
    grab: bool,
) -> anyhow::Result<Result<ArtistSearch, NotStarted>> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wanted_artists WHERE id = ?)")
            .bind(wanted_artist_id)
            .fetch_one(pool)
            .await?;
    if !exists {
        return Ok(Err(NotStarted::NoArtist));
    }
    let already: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM artist_searches
         WHERE wanted_artist_id = ? AND state IN ('queued', 'running') ORDER BY id LIMIT 1",
    )
    .bind(wanted_artist_id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = already {
        return Ok(Err(NotStarted::Already(id)));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO artist_searches (wanted_artist_id, grab, state, started_at)
         VALUES (?, ?, 'queued', ?) RETURNING id",
    )
    .bind(wanted_artist_id)
    .bind(grab)
    .bind(unix_now())
    .fetch_one(pool)
    .await?;
    let started = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("artist search {id} vanished"))?;
    let pool = pool.clone();
    tokio::spawn(async move {
        let _turn = RUNNING.lock().await;
        let outcome = run(&pool, &sources, id, wanted_artist_id, grab).await;
        let (state, message) = match outcome {
            Ok(message) => ("done", message),
            Err(err) => ("failed", format!("{err:#}")),
        };
        if let Err(err) = sqlx::query(
            "UPDATE artist_searches SET state = ?, message = ?, finished_at = ? WHERE id = ?",
        )
        .bind(state)
        .bind(message)
        .bind(unix_now())
        .bind(id)
        .execute(&pool)
        .await
        {
            tracing::warn!(error = %err, "couldn't record how an artist search ended");
        }
    });
    Ok(Ok(started))
}

/// Deals with each album in turn; returns the run's closing message.
async fn run(
    pool: &SqlitePool,
    sources: &Sources,
    id: i64,
    wanted_artist_id: i64,
    grab: bool,
) -> anyhow::Result<String> {
    sqlx::query("UPDATE artist_searches SET state = 'running' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let albums: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM wanted_albums
         WHERE wanted_artist_id = ? AND monitored = 1
         ORDER BY release_date IS NULL, release_date, id",
    )
    .bind(wanted_artist_id)
    .fetch_all(pool)
    .await?;
    let client = torznab::Client::new()?;
    let mut done: Vec<AlbumRun> = Vec::new();
    let (mut searched, mut grabbed) = (0, 0);
    for (album_id, title) in albums {
        let mut entry = AlbumRun {
            wanted_album_id: album_id,
            title,
            skipped: None,
            search_id: None,
            error: None,
            grabbed: Vec::new(),
        };
        if album_search::taken(pool, album_id).await? {
            entry.skipped = Some("it's on disk or downloading".into());
        } else {
            match album_search::search(pool, &client, sources.slskd.as_deref(), album_id).await? {
                Ok(search_id) => {
                    searched += 1;
                    entry.search_id = Some(search_id);
                    if grab {
                        grab_best(pool, sources, search_id, &mut entry.grabbed).await?;
                        if entry.grabbed.iter().any(|g| g.download_id.is_some()) {
                            grabbed += 1;
                        }
                    }
                }
                Err(refused) => entry.error = Some(refused.to_string()),
            }
        }
        done.push(entry);
        sqlx::query("UPDATE artist_searches SET albums = ? WHERE id = ?")
            .bind(serde_json::to_string(&done)?)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(if grab {
        format!(
            "{searched} of {} albums searched for, {grabbed} grabbed",
            done.len()
        )
    } else {
        format!("{searched} of {} albums searched for", done.len())
    })
}

/// Grabs the best accepted result of an album's search, trying the next when one can't be
/// fetched; a torrent needs Deluge, a Soulseek result slskd.
pub(crate) async fn grab_best(
    pool: &SqlitePool,
    sources: &Sources,
    search_id: i64,
    grabbed: &mut Vec<Grabbed>,
) -> anyhow::Result<()> {
    let best: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT id, title, soulseek IS NOT NULL FROM release_decisions
         WHERE search_id = ? AND accepted = 1
           AND lower(title) NOT IN (SELECT lower(title) FROM blocklist)
         ORDER BY json_extract(decision, '$.rank') DESC NULLS LAST, seeders DESC NULLS LAST, id
         LIMIT ?",
    )
    .bind(search_id)
    .bind(GRAB_TRIES)
    .fetch_all(pool)
    .await?;
    for (release_id, title, soulseek) in best {
        let outcome = match (soulseek, &sources.slskd, &sources.pipeline) {
            (true, Some(slskd), _) => soulseek_downloads::grab(pool, slskd, release_id).await?,
            (true, None, _) => Err(Refused::Soulseek),
            (false, _, Some(pipeline)) => {
                downloads::grab(pool, &pipeline.deluge, &pipeline.fetcher, release_id).await?
            }
            (false, _, None) => {
                grabbed.push(Grabbed {
                    release_id,
                    title,
                    upgrade: false,
                    download_id: None,
                    refused: Some("downloads are off (MELETE_DOWNLOADS)".into()),
                });
                continue;
            }
        };
        let (download_id, refused, try_next) = match outcome {
            Ok(download) => (Some(download.id), None, false),
            Err(
                refused @ (Refused::Fetch(_)
                | Refused::Deluge(_)
                | Refused::NoLink
                | Refused::Blocklisted
                | Refused::Slskd(_)
                | Refused::Soulseek),
            ) => (None, Some(refused.to_string()), true),
            Err(refused) => (None, Some(refused.to_string()), false),
        };
        grabbed.push(Grabbed {
            release_id,
            title,
            upgrade: false,
            download_id,
            refused,
        });
        if !try_next {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::slskd::testing;

    /// Radiohead wanted, with OK Computer to get, Kid A on disk, Amnesiac downloading and
    /// Hail to the Thief not monitored; Soulseek has OK Computer.
    async fn fixture() -> (SqlitePool, Sources, Arc<std::sync::Mutex<testing::Seen>>) {
        let pool = db::open_in_memory().await;
        let preset = crate::music_profiles::lossless(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/media/music');
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, profile_id, library_id, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', ?, 3, 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, release_date, monitored, source, added_at) VALUES
                 (7, 1, 'rg-ok', 'OK Computer', '1997-05-21', 1, 'lidarr', 0),
                 (8, 1, 'rg-kid', 'Kid A', '2000-10-02', 1, 'lidarr', 0),
                 (9, 1, 'rg-amn', 'Amnesiac', '2001-06-05', 1, 'lidarr', 0),
                 (10, 1, 'rg-htt', 'Hail to the Thief', '2003-06-09', 0, 'lidarr', 0);
             INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation, mb_release_group_id)
                 VALUES (3, 'dir:Kid A', 'Kid A', 'Radiohead', 'Radiohead/Kid A', 1, 'rg-kid');
             INSERT INTO downloads (infohash, title, wanted_album_id, decision, state, added_at, updated_at)
                 VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Radiohead - Amnesiac', 9, '{}', 'downloading', 0, 0);",
        )
        .bind(preset)
        .execute(&pool)
        .await
        .unwrap();
        let files: Vec<serde_json::Value> = (1..=3)
            .map(|n| serde_json::json!({"filename": format!("@@m\\Radiohead\\OK Computer\\0{n}.flac"), "size": 10, "bitDepth": 16}))
            .collect();
        let (slskd, seen) = testing::start(
            serde_json::json!([{"username": "alice", "hasFreeUploadSlot": true, "files": files}]),
            1,
        )
        .await;
        let sources = Sources {
            slskd: Some(Arc::new(slskd)),
            pipeline: None,
        };
        (pool, sources, seen)
    }

    async fn finished(pool: &SqlitePool, id: i64) -> ArtistSearch {
        for _ in 0..200 {
            let run = get(pool, id).await.unwrap().unwrap();
            if run.state == "done" || run.state == "failed" {
                return run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("artist search {id} didn't finish");
    }

    #[tokio::test]
    async fn searches_and_grabs_each_album_still_to_get() {
        let (pool, sources, seen) = fixture().await;
        let started = start(&pool, sources, 1, true).await.unwrap().unwrap();
        assert!(["queued", "running"].contains(&started.state.as_str()));
        let run = finished(&pool, started.id).await;
        assert_eq!(run.state, "done", "{:?}", run.message);
        assert_eq!(
            run.message.as_deref(),
            Some("1 of 3 albums searched for, 1 grabbed")
        );
        let summary: Vec<(&str, Option<&str>, bool)> = run
            .albums
            .iter()
            .map(|a| {
                (
                    a.title.as_str(),
                    a.skipped.as_deref(),
                    a.search_id.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("OK Computer", None, true),
                ("Kid A", Some("it's on disk or downloading"), false),
                ("Amnesiac", Some("it's on disk or downloading"), false),
            ],
            "oldest first; an album not monitored is left out"
        );
        let grab = &run.albums[0].grabbed[0];
        assert_eq!(grab.title, "Radiohead - OK Computer");
        let download = crate::downloads::get(&pool, grab.download_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (download.source.as_str(), download.wanted_album_id),
            ("soulseek", Some(7))
        );
        assert_eq!(seen.lock().unwrap().texts, ["Radiohead OK Computer"]);
        assert_eq!(seen.lock().unwrap().enqueued.len(), 1);
    }

    #[tokio::test]
    async fn can_search_without_grabbing() {
        let (pool, sources, seen) = fixture().await;
        let started = start(&pool, sources, 1, false).await.unwrap().unwrap();
        let run = finished(&pool, started.id).await;
        assert_eq!(run.message.as_deref(), Some("1 of 3 albums searched for"));
        assert!(run.albums[0].grabbed.is_empty());
        assert!(seen.lock().unwrap().enqueued.is_empty());
    }

    #[tokio::test]
    async fn one_run_per_artist_at_a_time_and_none_for_no_one() {
        let (pool, sources, _) = fixture().await;
        // Holding the turn keeps the first run queued.
        let turn = RUNNING.lock().await;
        let first = start(&pool, sources.clone(), 1, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.state, "queued");
        assert_eq!(
            start(&pool, sources.clone(), 1, true).await.unwrap(),
            Err(NotStarted::Already(first.id))
        );
        assert_eq!(
            start(&pool, sources, 99, true).await.unwrap(),
            Err(NotStarted::NoArtist)
        );
        drop(turn);
        assert_eq!(finished(&pool, first.id).await.state, "done");
    }

    #[tokio::test]
    async fn a_run_a_restart_cut_off_is_marked_failed() {
        let (pool, _, _) = fixture().await;
        sqlx::query(
            "INSERT INTO artist_searches (id, wanted_artist_id, grab, state, started_at)
             VALUES (4, 1, 1, 'running', 0), (5, 1, 1, 'done', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        recover(&pool).await.unwrap();
        let cut = get(&pool, 4).await.unwrap().unwrap();
        assert_eq!(
            (cut.state.as_str(), cut.message.as_deref()),
            ("failed", Some("a restart cut it off"))
        );
        assert_eq!(get(&pool, 5).await.unwrap().unwrap().state, "done");
    }
}
