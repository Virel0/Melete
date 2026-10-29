//! Fetching what a request asked for once it's approved: a search
//! at once, and the best accepted result grabbed, instead of waiting for a new release to turn
//! up in an RSS sync. A movie is searched for; a series season by season (the one asked for,
//! or every monitored one); an album and a song like the Wanted page's searches; a whole
//! artist gets a whole-artist run. What's on disk or on its way is left alone, and nothing is
//! grabbed while downloads are off. Runs go one at a time, after approval, in the background.

use std::sync::Arc;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::downloads::Pipeline;
use crate::progress::{self, Step};
use crate::rss::Grabbed;
use crate::search::{self, Target};
use crate::{album_search, artist_search, music_requests, slskd, song_search, tmdb, torznab};

/// Runs go one after another: they share the indexers and slskd. A song's doesn't wait its
/// turn: it's one quick Soulseek search, and a track never waits behind album work.
static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What fetching needs from outside.
#[derive(Clone, Default)]
pub struct Sources {
    /// TMDB, for movie and series details; None when it isn't set up.
    pub tmdb: Option<Arc<tmdb::Client>>,
    /// slskd, for album and song searches on Soulseek; None when it isn't set up.
    pub slskd: Option<Arc<slskd::Client>>,
    /// Deluge, for torrents; None when downloads are off.
    pub pipeline: Option<Arc<Pipeline>>,
}

/// What a run did.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Fetched {
    /// The searches it stored, by row id.
    pub searches: Vec<i64>,
    /// What it grabbed or tried to.
    pub grabbed: Vec<Grabbed>,
    /// Why something wasn't searched for or grabbed.
    pub notes: Vec<String>,
}

#[derive(sqlx::FromRow)]
struct Approved {
    kind: String,
    season: Option<i64>,
    decided_by: Option<i64>,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    wanted_artist_id: Option<i64>,
    wanted_album_id: Option<i64>,
    wanted_song_id: Option<i64>,
}

/// Fetches an approved request's title in the background.
pub fn start(pool: &SqlitePool, sources: &Sources, request_id: i64) {
    let (pool, sources) = (pool.clone(), sources.clone());
    tokio::spawn(async move {
        let song: bool = sqlx::query_scalar("SELECT kind = 'song' FROM requests WHERE id = ?")
            .bind(request_id)
            .fetch_optional(&pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
        let _turn = if song {
            None
        } else {
            Some(RUNNING.lock().await)
        };
        match run(&pool, &sources, request_id).await {
            Ok(fetched) => tracing::info!(
                request = request_id,
                searches = fetched.searches.len(),
                grabbed = fetched.grabbed.iter().filter(|g| g.download_id.is_some()).count(),
                notes = ?fetched.notes,
                "fetched for a request"
            ),
            Err(err) => tracing::warn!(
                request = request_id,
                error = format!("{err:#}"),
                "couldn't fetch for a request"
            ),
        }
    });
}

/// Fetches approved requests' titles one after another in the background (a playlist's
/// tracks, approved together), each taking its turn as [`start`] would have it.
pub fn start_each(pool: &SqlitePool, sources: &Sources, request_ids: Vec<i64>) {
    if request_ids.is_empty() {
        return;
    }
    let (pool, sources) = (pool.clone(), sources.clone());
    tokio::spawn(async move {
        for request_id in request_ids {
            if let Err(err) = run(&pool, &sources, request_id).await {
                tracing::warn!(
                    request = request_id,
                    error = format!("{err:#}"),
                    "couldn't fetch for a request"
                );
            }
        }
    });
}

/// Searches for an approved request's title and grabs the best of what's found.
pub async fn run(pool: &SqlitePool, sources: &Sources, request_id: i64) -> anyhow::Result<Fetched> {
    let mut fetched = Fetched::default();
    let approved: Option<Approved> = sqlx::query_as(
        "SELECT kind, season, decided_by, wanted_movie_id, wanted_series_id, wanted_artist_id,
             wanted_album_id, wanted_song_id
         FROM requests WHERE id = ? AND state = 'approved'",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await?;
    let Some(approved) = approved else {
        fetched.notes.push("it isn't approved".into());
        return Ok(fetched);
    };
    let music = artist_search::Sources {
        slskd: sources.slskd.clone(),
        pipeline: sources.pipeline.clone(),
    };
    match approved.kind.as_str() {
        "movie" | "series" => fetch_titles(pool, sources, &approved, &mut fetched).await?,
        "album" => {
            let Some(id) = approved.wanted_album_id else {
                return Ok(fetched);
            };
            fetch_album(pool, &music, id, &mut fetched).await?;
        }
        "song" => {
            let Some(id) = approved.wanted_song_id else {
                return Ok(fetched);
            };
            fetch_song(pool, &music, id, &mut fetched).await?;
        }
        "artist" => {
            let Some(id) = approved.wanted_artist_id else {
                return Ok(fetched);
            };
            fetch_artist(pool, music, &approved, id, &mut fetched).await?;
        }
        _ => {}
    }
    Ok(fetched)
}

/// Searches for a movie, or a series season by season, and grabs the best of each, when
/// downloads are on.
async fn fetch_titles(
    pool: &SqlitePool,
    sources: &Sources,
    approved: &Approved,
    fetched: &mut Fetched,
) -> anyhow::Result<()> {
    let Some(pipeline) = sources.pipeline.as_deref() else {
        fetched
            .notes
            .push("downloads are off (MELETE_DOWNLOADS)".into());
        return Ok(());
    };
    let targets = match (approved.wanted_movie_id, approved.wanted_series_id) {
        (Some(id), _) => vec![Target::Movie(id)],
        (_, Some(id)) => seasons(pool, id, approved.season)
            .await?
            .into_iter()
            .map(|season| Target::Series {
                id,
                season,
                episode: None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let client = torznab::Client::new()?;
    for target in targets {
        match search::search(pool, &client, sources.tmdb.as_deref(), target).await? {
            Ok(search_id) => {
                fetched.searches.push(search_id);
                crate::rss::grab_for(pool, pipeline, target, search_id, &mut fetched.grabbed)
                    .await?;
            }
            Err(refused) => fetched.notes.push(refused.to_string()),
        }
    }
    Ok(())
}

/// Searches for a song on Soulseek and grabs the best result, or fetches its album when it
/// isn't to be had on its own; nothing when it's on disk or on its way.
async fn fetch_song(
    pool: &SqlitePool,
    music: &artist_search::Sources,
    id: i64,
    fetched: &mut Fetched,
) -> anyhow::Result<()> {
    let on_its_way = progress::of(pool, "song", id).await?.is_some_and(|p| {
        matches!(
            p.step,
            Step::Downloading | Step::Transferring | Step::Importing | Step::InLibrary
        )
    });
    if on_its_way {
        fetched.notes.push("it's on disk or downloading".into());
        return Ok(());
    }
    match music.slskd.as_deref() {
        None => fetched
            .notes
            .push("songs come through slskd, which isn't set up".into()),
        Some(slskd) => match song_search::search(pool, slskd, id).await? {
            Ok(search_id) => {
                fetched.searches.push(search_id);
                artist_search::grab_best(pool, music, search_id, &mut fetched.grabbed).await?;
            }
            Err(refused) => fetched.notes.push(refused.to_string()),
        },
    }
    // Not to be had on its own: the album it's on is fetched for it.
    if !fetched.grabbed.iter().any(|g| g.download_id.is_some()) {
        match music_requests::want_song_album(pool, id).await? {
            Some(album) => {
                fetched
                    .notes
                    .push("not found on its own; fetching its album".into());
                fetch_album(pool, music, album, fetched).await?;
            }
            None => fetched
                .notes
                .push("not found on its own, and its album isn't known".into()),
        }
    }
    Ok(())
}

/// Starts a whole-artist run for an artist approved by someone, when downloads are on.
async fn fetch_artist(
    pool: &SqlitePool,
    music: artist_search::Sources,
    approved: &Approved,
    id: i64,
    fetched: &mut Fetched,
) -> anyhow::Result<()> {
    // Approved by no one: it was followed whole already, and is looked out for.
    if approved.decided_by.is_none() {
        fetched.notes.push("it's followed already".into());
    } else if music.pipeline.is_none() && music.slskd.is_none() {
        fetched
            .notes
            .push("downloads are off (MELETE_DOWNLOADS), and slskd isn't set up".into());
    } else {
        match artist_search::start(pool, music, id, true).await? {
            Ok(run) => fetched
                .notes
                .push(format!("whole-artist search {} started", run.id)),
            Err(artist_search::NotStarted::Already(run)) => fetched
                .notes
                .push(format!("whole-artist search {run} is under way already")),
            Err(artist_search::NotStarted::NoArtist) => fetched
                .notes
                .push("the artist isn't wanted any more".into()),
        }
    }
    Ok(())
}

/// Searches for a wanted album and grabs the best of what's found, unless it's on disk or on
/// its way.
async fn fetch_album(
    pool: &SqlitePool,
    music: &artist_search::Sources,
    id: i64,
    fetched: &mut Fetched,
) -> anyhow::Result<()> {
    if music.pipeline.is_none() && music.slskd.is_none() {
        fetched
            .notes
            .push("downloads are off (MELETE_DOWNLOADS), and slskd isn't set up".into());
    } else if album_search::taken(pool, id).await? {
        fetched.notes.push("it's on disk or downloading".into());
    } else {
        let client = torznab::Client::new()?;
        match album_search::search(pool, &client, music.slskd.as_deref(), id).await? {
            Ok(search_id) => {
                fetched.searches.push(search_id);
                artist_search::grab_best(pool, music, search_id, &mut fetched.grabbed).await?;
            }
            Err(refused) => fetched.notes.push(refused.to_string()),
        }
    }
    Ok(())
}

/// How often to look for songs just in whose albums follow them (`MELETE_SONG_ALBUMS`).
const AFTER_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// A song in for longer than this when the setting is turned on doesn't bring its album.
const AFTER_WITHIN: i64 = 24 * 3600;

/// Wants the album of each song that comes in, for as long as the server runs, fetching it
/// after other work (`MELETE_SONG_ALBUMS=on`).
pub async fn albums_after_songs(pool: SqlitePool, sources: Sources) {
    loop {
        tokio::time::sleep(AFTER_EVERY).await;
        if let Err(err) = albums_after(&pool, &sources).await {
            tracing::warn!(error = format!("{err:#}"), "couldn't want songs' albums");
        }
    }
}

/// Wants the albums of songs that came in lately and haven't brought theirs yet, and queues a
/// fetch of each that takes its turn behind other fetches; returns the albums.
pub async fn albums_after(pool: &SqlitePool, sources: &Sources) -> anyhow::Result<Vec<i64>> {
    let songs: Vec<i64> = sqlx::query_scalar(
        "SELECT s.id FROM wanted_songs s
         WHERE s.wanted_album_id IS NULL AND s.mb_release_group_id IS NOT NULL
           AND EXISTS (SELECT 1 FROM downloads d WHERE d.wanted_song_id = s.id
                         AND d.state = 'imported' AND d.updated_at >= ?)
         ORDER BY s.id",
    )
    .bind(crate::db::unix_now() - AFTER_WITHIN)
    .fetch_all(pool)
    .await?;
    let music = artist_search::Sources {
        slskd: sources.slskd.clone(),
        pipeline: sources.pipeline.clone(),
    };
    let mut albums = Vec::new();
    for song in songs {
        let Some(album) = music_requests::want_song_album(pool, song).await? else {
            continue;
        };
        albums.push(album);
        let (pool, music) = (pool.clone(), music.clone());
        tokio::spawn(async move {
            let _turn = RUNNING.lock().await;
            let mut fetched = Fetched::default();
            match fetch_album(&pool, &music, album, &mut fetched).await {
                Ok(()) => tracing::info!(album, notes = ?fetched.notes, "fetched a song's album"),
                Err(err) => tracing::warn!(
                    album,
                    error = format!("{err:#}"),
                    "couldn't fetch a song's album"
                ),
            }
        });
    }
    Ok(albums)
}

/// The seasons to search for: the one asked for, else every monitored one but specials.
async fn seasons(pool: &SqlitePool, series: i64, asked: Option<i64>) -> sqlx::Result<Vec<i64>> {
    if let Some(season) = asked {
        return Ok(vec![season]);
    }
    sqlx::query_scalar(
        "SELECT season FROM wanted_seasons WHERE wanted_series_id = ? AND monitored = 1 AND season > 0
         ORDER BY season",
    )
    .bind(series)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Secret;
    use crate::db;
    use crate::deluge;
    use crate::downloads::{self, Fetcher};
    use crate::rss::tests::{indexer, releases, setup, want_dune, want_frieren};

    /// A new user each time, as each asks once.
    async fn user(pool: &SqlitePool) -> i64 {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(pool)
            .await
            .unwrap();
        crate::auth::create_user(pool, &format!("user{n}"), "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap()
            .id
    }

    /// An approved request of `kind` linked through `column` to wanted item `wanted`.
    async fn approved(
        pool: &SqlitePool,
        kind: &str,
        column: &str,
        wanted: i64,
        season: Option<i64>,
    ) -> i64 {
        let user = user(pool).await;
        let tmdb = ["movie", "series"].contains(&kind).then_some(1);
        let (artist, group) = match kind {
            "artist" => (Some("mb-a"), None),
            "album" => (Some("mb-a"), Some("rg")),
            _ => (None, None),
        };
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "INSERT INTO requests (user_id, kind, tmdb_id, season, mb_artist_id, mb_release_group_id,
                 title, state, {column}, created_at, decided_at, decided_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Asked', 'approved', ?7, 0, 0, ?1) RETURNING id"
        )))
        .bind(user)
        .bind(kind)
        .bind(tmdb)
        .bind(season)
        .bind(artist)
        .bind(group)
        .bind(wanted)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn torrents() -> (Sources, deluge::testing::Shared) {
        let (url, fake) = deluge::testing::start().await;
        let pipeline = Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        };
        let sources = Sources {
            pipeline: Some(Arc::new(pipeline)),
            ..Sources::default()
        };
        (sources, fake)
    }

    #[tokio::test]
    async fn an_approved_movie_is_searched_for_and_its_best_release_grabbed() {
        let pool = setup().await;
        let dune = want_dune(&pool).await;
        let request = approved(&pool, "movie", "wanted_movie_id", dune, None).await;
        let feed = indexer(&pool, "[2000]").await;
        feed.lock().unwrap().0 = releases(&[
            ("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", ""),
            (
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
                "",
            ),
        ]);
        let (sources, deluge) = torrents().await;
        let fetched = run(&pool, &sources, request).await.unwrap();
        assert_eq!(fetched.searches.len(), 1);
        let grabbed: Vec<(&str, bool)> = fetched
            .grabbed
            .iter()
            .map(|g| (g.title.as_str(), g.download_id.is_some()))
            .collect();
        assert_eq!(
            grabbed,
            [(
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
                true
            )]
        );
        assert_eq!(deluge.lock().unwrap().torrents.len(), 1);

        // Asked again while it downloads: searched, but nothing more is grabbed.
        let again = run(&pool, &sources, request).await.unwrap();
        assert_eq!((again.searches.len(), again.grabbed.len()), (1, 0));
    }

    #[tokio::test]
    async fn a_series_is_searched_for_season_by_season() {
        let pool = setup().await;
        // Season 1 monitored, season 2 not.
        let frieren = want_frieren(&pool, None).await;
        let whole = approved(&pool, "series", "wanted_series_id", frieren, None).await;
        let feed = indexer(&pool, "[5000]").await;
        feed.lock().unwrap().0 =
            releases(&[("Frieren.Beyond.Journeys.End.S01.1080p.BluRay.x264-GRP", "")]);
        let (sources, deluge) = torrents().await;
        let fetched = run(&pool, &sources, whole).await.unwrap();
        let searched: Vec<Option<i64>> =
            sqlx::query_scalar("SELECT season FROM searches ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(searched, [Some(1)], "only the monitored season");
        assert_eq!(fetched.grabbed.len(), 1);
        assert!(
            fetched.grabbed[0].download_id.is_some(),
            "{:?}",
            fetched.grabbed
        );
        assert_eq!(deluge.lock().unwrap().torrents.len(), 1);

        // One season asked for: that one alone.
        sqlx::query("DELETE FROM requests")
            .execute(&pool)
            .await
            .unwrap();
        let one = approved(&pool, "series", "wanted_series_id", frieren, Some(2)).await;
        run(&pool, &sources, one).await.unwrap();
        let last: Option<i64> =
            sqlx::query_scalar("SELECT season FROM searches ORDER BY id DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last, Some(2));
    }

    #[tokio::test]
    async fn a_song_is_searched_for_on_soulseek_and_grabbed() {
        let pool = db::open_in_memory().await;
        let preset = crate::music_profiles::lossless(&pool).await.unwrap();
        let song: i64 = sqlx::query_scalar(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/music');
             INSERT INTO wanted_songs (title, artist, library_id, music_profile_id, source, added_at)
             VALUES ('Paranoid Android', 'Radiohead', 3, ?, 'manual', 0) RETURNING id",
        )
        .bind(preset)
        .fetch_one(&pool)
        .await
        .unwrap();
        let request = approved(&pool, "song", "wanted_song_id", song, None).await;
        let files = serde_json::json!([{
            "filename": "@@m\\Radiohead\\OK Computer\\02 Paranoid Android.flac",
            "size": 10, "bitDepth": 16,
        }]);
        let (slskd, seen) = crate::slskd::testing::start(
            serde_json::json!([{"username": "alice", "hasFreeUploadSlot": true, "files": files}]),
            1,
        )
        .await;
        let sources = Sources {
            slskd: Some(Arc::new(slskd)),
            ..Sources::default()
        };
        let fetched = run(&pool, &sources, request).await.unwrap();
        assert_eq!(fetched.searches.len(), 1);
        let download = downloads::get(&pool, fetched.grabbed[0].download_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(download.source, "soulseek");
        assert_eq!(seen.lock().unwrap().enqueued.len(), 1);

        // On its way now: not searched for again.
        let again = run(&pool, &sources, request).await.unwrap();
        assert_eq!(
            (again.searches.len(), again.notes),
            (0, vec!["it's on disk or downloading".to_owned()])
        );
    }

    #[tokio::test]
    async fn nothing_is_fetched_without_a_way_to_or_before_approval() {
        let pool = setup().await;
        let dune = want_dune(&pool).await;
        let request = approved(&pool, "movie", "wanted_movie_id", dune, None).await;
        let feed = indexer(&pool, "[2000]").await;
        let off = run(&pool, &Sources::default(), request).await.unwrap();
        assert_eq!(
            off,
            Fetched {
                notes: vec!["downloads are off (MELETE_DOWNLOADS)".into()],
                ..Fetched::default()
            }
        );
        assert!(feed.lock().unwrap().1.is_empty(), "no indexer was asked");

        sqlx::query(
            "UPDATE requests SET state = 'requested', decided_at = NULL, decided_by = NULL,
                 wanted_movie_id = NULL",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (sources, _) = torrents().await;
        assert_eq!(
            run(&pool, &sources, request).await.unwrap().notes,
            ["it isn't approved"]
        );

        // A whole artist followed already isn't searched for again.
        let artist: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_artists (mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES ('mb-a', 'Radiohead', 1, 'all', 'manual', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let followed = approved(&pool, "artist", "wanted_artist_id", artist, None).await;
        sqlx::query("UPDATE requests SET decided_by = NULL WHERE id = ?")
            .bind(followed)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            run(&pool, &sources, followed).await.unwrap().notes,
            ["it's followed already"]
        );
    }

    #[tokio::test]
    async fn a_song_doesnt_wait_behind_other_fetches() {
        let pool = db::open_in_memory().await;
        let preset = crate::music_profiles::lossless(&pool).await.unwrap();
        let song: i64 = sqlx::query_scalar(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/music');
             INSERT INTO wanted_songs (title, artist, library_id, music_profile_id, source, added_at)
             VALUES ('Creep', 'Radiohead', 3, ?, 'manual', 0) RETURNING id",
        )
        .bind(preset)
        .fetch_one(&pool)
        .await
        .unwrap();
        let request = approved(&pool, "song", "wanted_song_id", song, None).await;
        let (slskd, seen) = crate::slskd::testing::start(serde_json::json!([]), 1).await;
        let sources = Sources {
            slskd: Some(Arc::new(slskd)),
            ..Sources::default()
        };
        // Another fetch holds the turn, an album's say, for the whole test.
        let _album = RUNNING.lock().await;
        start(&pool, &sources, request);
        let searched = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while seen.lock().unwrap().texts.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(searched.is_ok(), "the song's search waited for the turn");
        assert_eq!(seen.lock().unwrap().texts, ["Radiohead Creep"]);
    }

    /// Creep wanted, on OK Computer, with Radiohead known on MusicBrainz.
    async fn want_creep(pool: &SqlitePool) -> i64 {
        let preset = crate::music_profiles::lossless(pool).await.unwrap();
        sqlx::query_scalar(
            "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/music');
             INSERT INTO wanted_songs (title, artist, album, mb_artist_id, mb_release_group_id,
                 library_id, music_profile_id, source, added_at)
             VALUES ('Creep', 'Radiohead', 'OK Computer', 'mb-a', 'rg-ok', 3, ?, 'manual', 0)
             RETURNING id",
        )
        .bind(preset)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_song_not_found_on_its_own_fetches_its_album() {
        let pool = db::open_in_memory().await;
        let song = want_creep(&pool).await;
        let request = approved(&pool, "song", "wanted_song_id", song, None).await;
        // Soulseek has the album, but no file that is the song.
        let files: Vec<serde_json::Value> = (1..=3)
            .map(|n| serde_json::json!({"filename": format!("@@m\\Radiohead\\OK Computer\\0{n}.flac"), "size": 10, "bitDepth": 16}))
            .collect();
        let (slskd, seen) = crate::slskd::testing::start(
            serde_json::json!([{"username": "alice", "hasFreeUploadSlot": true, "files": files}]),
            1,
        )
        .await;
        let sources = Sources {
            slskd: Some(Arc::new(slskd)),
            ..Sources::default()
        };
        let fetched = run(&pool, &sources, request).await.unwrap();
        assert!(
            fetched
                .notes
                .contains(&"not found on its own; fetching its album".to_owned()),
            "{:?}",
            fetched.notes
        );
        assert_eq!(
            seen.lock().unwrap().texts,
            ["Radiohead Creep", "Radiohead OK Computer"]
        );
        let album: i64 =
            sqlx::query_scalar("SELECT wanted_album_id FROM wanted_songs WHERE id = ?")
                .bind(song)
                .fetch_one(&pool)
                .await
                .unwrap();
        let download = downloads::get(&pool, fetched.grabbed.last().unwrap().download_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(download.wanted_album_id, Some(album));
        // The song goes as its album's download goes.
        let progress = progress::of(&pool, "song", song).await.unwrap().unwrap();
        assert_eq!(progress.download_id, Some(download.id));
        assert_ne!(progress.step, Step::Wanted);
        // Asked again: it's on its way, through its album.
        assert_eq!(
            run(&pool, &sources, request).await.unwrap().notes,
            ["it's on disk or downloading"]
        );
    }

    #[tokio::test]
    async fn a_songs_album_follows_it_once_its_in() {
        let pool = db::open_in_memory().await;
        let song = want_creep(&pool).await;
        let now = crate::db::unix_now();
        sqlx::query(
            "INSERT INTO downloads (id, source, soulseek, title, wanted_song_id, decision, state,
                 added_at, updated_at, completed_at)
             VALUES (9, 'soulseek', '{}', 'Radiohead - Creep', ?, '{}', 'downloading', ?, ?, NULL)",
        )
        .bind(song)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let sources = Sources::default();
        assert!(
            albums_after(&pool, &sources).await.unwrap().is_empty(),
            "not while it downloads"
        );

        sqlx::query("UPDATE downloads SET state = 'imported' WHERE id = 9")
            .execute(&pool)
            .await
            .unwrap();
        let albums = albums_after(&pool, &sources).await.unwrap();
        assert_eq!(albums.len(), 1);
        assert!(
            albums_after(&pool, &sources).await.unwrap().is_empty(),
            "once"
        );

        // The album downloading after it doesn't make the song look unfinished.
        sqlx::query(
            "INSERT INTO downloads (source, soulseek, title, wanted_album_id, decision, state, added_at, updated_at)
             VALUES ('soulseek', '{}', 'Radiohead - OK Computer', ?, '{}', 'downloading', 0, 0)",
        )
        .bind(albums[0])
        .execute(&pool)
        .await
        .unwrap();
        let progress = progress::of(&pool, "song", song).await.unwrap().unwrap();
        assert_eq!(
            (progress.step, progress.download_id),
            (Step::InLibrary, Some(9))
        );

        // A song in long before isn't followed by its album.
        for undo in [
            "UPDATE wanted_songs SET wanted_album_id = NULL",
            "UPDATE downloads SET updated_at = 0 WHERE id = 9",
        ] {
            sqlx::query(undo).execute(&pool).await.unwrap();
        }
        assert!(albums_after(&pool, &sources).await.unwrap().is_empty());
    }
}
