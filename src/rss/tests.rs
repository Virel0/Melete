use std::sync::Mutex;

use axum::extract::{RawQuery, State};

use super::*;
use crate::config::Secret;
use crate::db;
use crate::deluge::testing;
use crate::profiles::import::tests::converted;
use crate::profiles::{self, App};

pub(crate) type Feed = Arc<Mutex<(String, Vec<String>)>>;

/// An indexer whose newest releases are what `feed` holds; its requests are logged there.
pub(crate) async fn indexer(pool: &SqlitePool, categories: &str) -> Feed {
    let feed: Feed = Arc::default();
    let router = axum::Router::new()
        .route(
            "/api",
            axum::routing::get(
                |RawQuery(q): RawQuery, State(feed): State<Feed>| async move {
                    let mut feed = feed.lock().unwrap();
                    feed.1.push(q.unwrap_or_default());
                    feed.0.clone()
                },
            ),
        )
        .with_state(feed.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    sqlx::query(
        "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                 tv_search_params, source, source_id, added_at)
             VALUES ('Tracker', ?, 'secret-key', 1, ?, '[\"q\"]', '[\"q\"]', 'prowlarr',
                 (SELECT count(*) + 1 FROM indexers), 0)",
    )
    .bind(url)
    .bind(categories)
    .execute(pool)
    .await
    .unwrap();
    feed
}

/// Releases with magnet links, each with its own hash, and any extra Torznab attributes.
pub(crate) fn releases(items: &[(&str, &str)]) -> String {
    let items: String = items
            .iter()
            .map(|(title, attrs)| {
                let hash: String = Sha256::digest(title.as_bytes())[..20]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                format!(
                    r#"<item><title>{title}</title><guid>https://tracker.example/{hash}?passkey=pk</guid>
                    <link>magnet:?xt=urn:btih:{hash}</link><size>5000000000</size>
                    <torznab:attr name="seeders" value="50"/>{attrs}</item>"#
                )
            })
            .collect();
    format!(
        r#"<rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>{items}</channel></rss>"#
    )
}

pub(crate) async fn setup() -> SqlitePool {
    let pool = db::open_in_memory().await;
    for app in [App::Radarr, App::Sonarr] {
        profiles::save(&pool, app, &converted(app)).await.unwrap();
    }
    pool
}

pub(crate) async fn want_dune(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
            "INSERT INTO wanted_movies (tmdb_id, imdb_id, title, year, monitored, quality_profile,
                 source, added_at, original_language)
             VALUES (693134, 'tt15239678', 'Dune: Part Two', 2024, 1, '4K Preferred', 'radarr', 0, 'en')
             RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Frieren, with season 1 monitored but its episode 7, and season 2 not.
pub(crate) async fn want_frieren(pool: &SqlitePool, series_id: Option<i64>) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO wanted_series (tvdb_id, title, year, monitored, series_type, quality_profile,
                 source, added_at, series_id, original_language)
             VALUES (424536, 'Frieren: Beyond Journey''s End', 2023, 1, 'anime', '4K Preferred',
                 'sonarr', 0, ?, 'en')
             RETURNING id",
    )
    .bind(series_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (?, 1, 1), (?, 2, 0);
             INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (?, 1, 7, 0)",
        )
        .bind(id)
        .bind(id)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Each release of a sync's searches: its title, whether accepted, and its first reason.
async fn verdicts(pool: &SqlitePool, view: &SyncView) -> Vec<(String, bool, Option<String>)> {
    let mut all = Vec::new();
    for s in &view.searches {
        let search = search::get(pool, s.search_id).await.unwrap().unwrap();
        assert_eq!(search.rss_sync_id, Some(view.id));
        for r in search.releases {
            let first = r.decision["rejections"][0].as_str().map(str::to_owned);
            all.push((r.title, r.accepted, first));
        }
    }
    all.sort();
    all
}

/// Wanted movie, wanted series, season and episode.
type Place = (Option<i64>, Option<i64>, Option<i64>, Option<i64>);

#[tokio::test]
async fn decides_on_new_releases_for_wanted_titles_once() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    let frieren = want_frieren(&pool, None).await;
    let feed = indexer(&pool, "[2000,2045,5000,5040,8000]").await;
    let no_video = indexer(&pool, "[3000]").await;
    feed.lock().unwrap().0 = releases(&[
        ("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", ""),
        ("Dune.Part.Two.2024.720p.WEB-DL.H.264-GRP", ""),
        ("Frieren.Beyond.Journeys.End.S01E05.1080p.WEB.H264-GRP", ""),
        (
            "Frieren.Beyond.Journeys.End.S01E06.1080p.WEB.H264-GRP",
            r#"<torznab:attr name="tvdbid" value="1"/>"#,
        ),
        ("Frieren.Beyond.Journeys.End.S01E07.1080p.WEB.H264-GRP", ""),
        ("Frieren.Beyond.Journeys.End.S02E01.1080p.WEB.H264-GRP", ""),
        ("Some.Other.Movie.2024.1080p.WEB-DL.H.264-GRP", ""),
    ]);
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, None).await.unwrap().unwrap();
    assert_eq!(view.new_releases, 7);
    assert!(view.grabbed.is_empty(), "downloads are off");
    let s = |t: &str| Some(t.to_owned());
    assert_eq!(
        verdicts(&pool, &view).await,
        [
            (
                "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX".into(),
                true,
                None
            ),
            (
                "Dune.Part.Two.2024.720p.WEB-DL.H.264-GRP".into(),
                false,
                s("WEBDL-720p isn't allowed by 4K Preferred")
            ),
            (
                "Frieren.Beyond.Journeys.End.S01E05.1080p.WEB.H264-GRP".into(),
                true,
                None
            ),
            (
                "Frieren.Beyond.Journeys.End.S01E06.1080p.WEB.H264-GRP".into(),
                false,
                s("the indexer says it's TVDB 1, not 424536")
            ),
            (
                "Frieren.Beyond.Journeys.End.S01E07.1080p.WEB.H264-GRP".into(),
                false,
                s("S01E07 isn't monitored")
            ),
            (
                "Frieren.Beyond.Journeys.End.S02E01.1080p.WEB.H264-GRP".into(),
                false,
                s("season 2 isn't monitored")
            ),
        ]
    );
    let targets: Vec<Place> = view
        .searches
        .iter()
        .map(|s| (s.wanted_movie_id, s.wanted_series_id, s.season, s.episode))
        .collect();
    assert!(targets.contains(&(Some(dune), None, None, None)));
    assert!(targets.contains(&(None, Some(frieren), Some(1), Some(5))));
    assert_eq!(view.indexers[0].results, 7);
    assert_eq!(
        view.indexers[1].error.as_deref(),
        Some("it has no movie or TV categories")
    );
    assert!(no_video.lock().unwrap().1.is_empty());
    assert_eq!(
        feed.lock().unwrap().1[0],
        "t=search&cat=2000%2C2045%2C5000%2C5040&apikey=secret-key"
    );

    // The same releases again: nothing new, nothing stored.
    let again = sync(&pool, &client, None, None).await.unwrap().unwrap();
    assert_eq!((again.new_releases, again.searches.len()), (0, 0));
    assert_eq!(recent(&pool, 10).await.unwrap().len(), 2);
    // No guid or passkey is kept, only hashes.
    let keys: Vec<String> = sqlx::query_scalar("SELECT release_key FROM rss_seen")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(keys.len() == 7 && keys.iter().all(|k| k.len() == 64 && !k.contains("passkey")));
}

#[tokio::test]
async fn a_release_fitting_two_wanted_titles_is_refused() {
    let pool = setup().await;
    for (tvdb, year) in [(76107, 1963), (78804, 2005)] {
        sqlx::query(
                "INSERT INTO wanted_series (tvdb_id, title, year, monitored, series_type, quality_profile,
                     source, added_at, original_language)
                 VALUES (?, 'Doctor Who', ?, 1, 'standard', '4K Preferred', 'sonarr', 0, 'en');
                 INSERT INTO wanted_seasons (wanted_series_id, season, monitored)
                 VALUES (last_insert_rowid(), 1, 1)",
            )
            .bind(tvdb)
            .bind(year)
            .execute(&pool)
            .await
            .unwrap();
    }
    let feed = indexer(&pool, "[5000]").await;
    feed.lock().unwrap().0 = releases(&[
        ("Doctor.Who.S01E01.1080p.WEB.H264-GRP", ""),
        ("Doctor.Who.2005.S01E02.1080p.WEB.H264-GRP", ""),
    ]);
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, None).await.unwrap().unwrap();
    let verdicts = verdicts(&pool, &view).await;
    let undated: Vec<_> = verdicts
        .iter()
        .filter(|(t, ..)| t.starts_with("Doctor.Who.S01E01"))
        .collect();
    assert_eq!(undated.len(), 2, "kept against both");
    assert!(undated.iter().all(|(_, accepted, why)| !accepted
        && why.as_deref() == Some("it could be any of 2 wanted titles named \"Doctor Who\"")));
    // With its year it's one of them.
    let dated: Vec<_> = verdicts
        .iter()
        .filter(|(t, ..)| t.starts_with("Doctor.Who.2005"))
        .collect();
    assert_eq!(dated.iter().filter(|(_, accepted, _)| *accepted).count(), 1);
}

#[tokio::test]
async fn grabs_the_best_release_for_what_is_missing() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Frieren', 'Frieren', 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season, episode,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Frieren/Frieren.S01E05.720p.HDTV.x264-GRP.mkv', 1, 0, 1, 1, 5, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let frieren = want_frieren(&pool, Some(1)).await;
    let feed = indexer(&pool, "[2000,5000]").await;
    feed.lock().unwrap().0 = releases(&[
        ("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", ""),
        (
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
            "",
        ),
        // An upgrade of the 720p file on disk.
        ("Frieren.Beyond.Journeys.End.S01E05.1080p.WEB.H264-GRP", ""),
        ("Frieren.Beyond.Journeys.End.S01E06.1080p.WEB.H264-GRP", ""),
    ]);
    let (url, fake) = testing::start().await;
    let pipeline = Pipeline {
        deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: downloads::Fetcher::new().unwrap(),
        seedbox: None,
    };
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    let titles: Vec<&str> = view.grabbed.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
            "Frieren.Beyond.Journeys.End.S01E05.1080p.WEB.H264-GRP",
            "Frieren.Beyond.Journeys.End.S01E06.1080p.WEB.H264-GRP",
        ],
        "the best for each, the upgrade included"
    );
    let upgrades: Vec<bool> = view.grabbed.iter().map(|g| g.upgrade).collect();
    assert_eq!(upgrades, [false, true, false]);
    assert!(
        view.grabbed
            .iter()
            .all(|g| g.download_id.is_some() && g.refused.is_none())
    );
    let downloads: Vec<Place> = sqlx::query_as(
        "SELECT wanted_movie_id, wanted_series_id, season, episode FROM downloads ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        downloads,
        [
            (Some(dune), None, None, None),
            (None, Some(frieren), Some(1), Some(5)),
            (None, Some(frieren), Some(1), Some(6))
        ]
    );
    assert_eq!(fake.lock().unwrap().torrents.len(), 3);

    // A better release later, and the season pack: both wait while downloads are on the way.
    feed.lock().unwrap().0 = releases(&[
        (
            "Dune.Part.Two.2024.2160p.WEB-DL.DDP5.1.DV.HDR.H.265-FLUX",
            "",
        ),
        ("Frieren.Beyond.Journeys.End.S01.1080p.BluRay.x264-GRP", ""),
    ]);
    let later = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(later.searches.len(), 2);
    assert!(later.grabbed.is_empty(), "{:?}", later.grabbed);
}

#[tokio::test]
async fn a_title_on_disk_in_a_folder_not_linked_to_it_isnt_grabbed_again() {
    let pool = setup().await;
    want_dune(&pool).await;
    // Matched through TMDB, but not linked to the wanted entry.
    sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             VALUES (1, 1, 'Dune Part Two (2024)', 'Dune Part Two', 1, NULL);
             UPDATE movies SET arr_tmdb_id = 693134 WHERE id = 1;
             INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Dune Part Two (2024)/Dune.Part.Two.2024.1080p.mkv', 1, 0, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(
        "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
        "",
    )]);
    let (url, fake) = testing::start().await;
    let pipeline = Pipeline {
        deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: downloads::Fetcher::new().unwrap(),
        seedbox: None,
    };
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.searches[0].accepted, 1);
    assert!(view.grabbed.is_empty());
    assert!(fake.lock().unwrap().torrents.is_empty());
}

#[tokio::test]
async fn an_upgrade_waits_for_the_last_one_and_needs_every_file_replaceable() {
    let pool = setup().await;
    want_dune(&pool).await;
    sqlx::query(
            "INSERT INTO libraries (id, name, kind, path, last_scan_at) VALUES (1, 'movies', 'movies', '/m', 0);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Dune Part Two (2024)', 'Dune Part Two', 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Dune Part Two (2024)/Dune.Part.Two.2024.720p.HDTV.x264-GRP.mkv', 1, 0, 1, 1, 1);
             UPDATE wanted_movies SET movie_id = 1;
             -- An earlier upgrade in the library, its old file not dealt with yet.
             INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, added_at,
                 updated_at, replaces)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Dune.Part.Two.2024.1080p', 1, '{}',
                 'imported', 0, 0, '[{\"file_id\": 1}]')",
        )
        .execute(&pool)
        .await
        .unwrap();
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", "")]);
    let (url, _fake) = testing::start().await;
    let pipeline = Pipeline {
        deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: downloads::Fetcher::new().unwrap(),
        seedbox: None,
    };
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.searches[0].accepted, 1);
    assert!(view.grabbed.is_empty(), "the last upgrade isn't settled");

    // Settled, but a second copy sits in another folder known by the title's id: it
    // wouldn't be replaced, so there's no upgrade.
    sqlx::query(
            "UPDATE downloads SET retired_at = 1;
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, arr_tmdb_id)
             VALUES (2, 1, 'Dune 2', 'Dune', 1, 693134);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Dune 2/Dune.mkv', 1, 0, 2, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    feed.lock().unwrap().0 = releases(&[("Dune.Part.Two.2024.1080p.BluRay.x264-GRP", "")]);
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert!(view.grabbed.is_empty());

    // With only the one file, it's grabbed as an upgrade replacing it.
    sqlx::query("UPDATE media_files SET missing_since = 1 WHERE movie_id = 2")
        .execute(&pool)
        .await
        .unwrap();
    feed.lock().unwrap().0 =
        releases(&[("Dune.Part.Two.2024.1080p.AMZN.WEB-DL.DDP5.1.H.264-NTb", "")]);
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.grabbed.len(), 1, "{:?}", view.grabbed);
    assert!(view.grabbed[0].upgrade && view.grabbed[0].download_id.is_some());
    let download = downloads::get(&pool, view.grabbed[0].download_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        download.replaces,
        ["Dune Part Two (2024)/Dune.Part.Two.2024.720p.HDTV.x264-GRP.mkv"]
    );
}

#[tokio::test]
async fn releases_numbered_from_the_first_episode_find_their_episode() {
    let pool = setup().await;
    sqlx::query(
            "INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, fetched_at) VALUES (209867, 424536, 'Frieren', 0);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id)
             WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 38)
             SELECT 209867, iif(value <= 28, 1, 2), iif(value <= 28, value, value - 28), value FROM n",
        )
        .execute(&pool)
        .await
        .unwrap();
    let frieren = want_frieren(&pool, None).await;
    sqlx::query(
        "INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (?, 3, 1)",
    )
    .bind(frieren)
    .execute(&pool)
    .await
    .unwrap();
    let feed = indexer(&pool, "[5000]").await;
    feed.lock().unwrap().0 = releases(&[
        (
            "[SubsPlease] Frieren - Beyond Journey's End - 05 (1080p) [ABCD1234].mkv",
            "",
        ),
        // S02E02: season 2 isn't monitored.
        (
            "[SubsPlease] Frieren - Beyond Journey's End - 30 (1080p) [ABCD1234].mkv",
            "",
        ),
        // Past TMDB's list: nothing to map it to.
        (
            "[SubsPlease] Frieren - Beyond Journey's End - 50 (1080p) [ABCD1234].mkv",
            "",
        ),
    ]);
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, None).await.unwrap().unwrap();
    let places: Vec<Place> = view
        .searches
        .iter()
        .map(|s| (s.wanted_movie_id, s.wanted_series_id, s.season, s.episode))
        .collect();
    assert_eq!(
        places,
        [
            (None, Some(frieren), Some(1), Some(5)),
            (None, Some(frieren), Some(2), Some(2))
        ]
    );
    let verdicts = verdicts(&pool, &view).await;
    assert!(verdicts[0].1, "{verdicts:?}");
    assert_eq!(verdicts[1].2.as_deref(), Some("season 2 isn't monitored"));
}

#[tokio::test]
async fn looks_out_for_wanted_albums_and_grabs_the_best_once() {
    let pool = setup().await;
    let preset = music_profiles::lossless(&pool).await.unwrap();
    sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (9, 'music', 'music', '/m');
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at, profile_id)
                 VALUES (1, 'mb-r', 'Radiohead', 1, 'all', 'lidarr', 0, ?1), (2, 'mb-q', 'Queen', 1, 'all', 'lidarr', 0, ?1);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at) VALUES
                 (10, 1, 'rg-okc', 'OK Computer', 1, 'lidarr', 0), (11, 1, 'rg-kida', 'Kid A', 1, 'lidarr', 0),
                 (12, 1, 'rg-amn', 'Amnesiac', 0, 'lidarr', 0),
                 (13, 2, 'rg-gh1', 'Greatest Hits', 1, 'lidarr', 0), (14, 2, 'rg-gh2', 'Greatest Hits', 1, 'lidarr', 0);
             INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation, mb_release_group_id)
                 VALUES (9, 'k', 'Kid A', 'Radiohead', 'r', 1, 'rg-kida');",
        )
        .bind(preset)
        .execute(&pool)
        .await
        .unwrap();
    let feed = indexer(&pool, "[2000,3000,3040,5000]").await;
    let music = |c: u32| format!(r#"<torznab:attr name="category" value="{c}"/>"#);
    let (flac, mp3) = (music(3040), music(3010));
    feed.lock().unwrap().0 = releases(&[
        ("Radiohead - OK Computer (1997) [MP3 320]", mp3.as_str()),
        ("Radiohead - OK Computer (1997) [FLAC]", flac.as_str()),
        ("Radiohead - Kid A (2000) [FLAC]", flac.as_str()),
        ("Radiohead - Amnesiac (2001) [FLAC]", flac.as_str()),
        ("Queen - Greatest Hits (1981) [FLAC]", flac.as_str()),
    ]);
    let (url, _fake) = testing::start().await;
    let pipeline = Pipeline {
        deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: downloads::Fetcher::new().unwrap(),
        seedbox: None,
    };
    let client = torznab::Client::new().unwrap();
    let view = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();

    let asked = feed.lock().unwrap().1.clone();
    assert_eq!(asked.len(), 2, "video and music asked apart: {asked:?}");
    assert!(asked[1].contains("cat=3000%2C3040"), "{asked:?}");
    // A search for each album named: the unmonitored one isn't looked out for.
    let albums: Vec<Option<i64>> = view.searches.iter().map(|s| s.wanted_album_id).collect();
    assert_eq!(albums, [Some(10), Some(11), Some(13), Some(14)]);
    let mut verdicts = Vec::new();
    for s in &view.searches {
        for r in search::get(&pool, s.search_id)
            .await
            .unwrap()
            .unwrap()
            .releases
        {
            verdicts.push((
                s.wanted_album_id,
                r.title,
                r.accepted,
                r.decision["reasons"][0].as_str().map(str::to_owned),
            ));
        }
    }
    let ambiguous = Some("it could be any of 2 wanted albums".to_owned());
    assert_eq!(
        verdicts,
        [
            (
                Some(10),
                "Radiohead - OK Computer (1997) [FLAC]".into(),
                true,
                None
            ),
            (
                Some(10),
                "Radiohead - OK Computer (1997) [MP3 320]".into(),
                true,
                None
            ),
            (
                Some(11),
                "Radiohead - Kid A (2000) [FLAC]".into(),
                true,
                None
            ),
            (
                Some(13),
                "Queen - Greatest Hits (1981) [FLAC]".into(),
                false,
                ambiguous.clone()
            ),
            (
                Some(14),
                "Queen - Greatest Hits (1981) [FLAC]".into(),
                false,
                ambiguous
            ),
        ]
    );
    // The best for the album with nothing on disk; Kid A is there already.
    let grabbed: Vec<(&str, bool)> = view
        .grabbed
        .iter()
        .map(|g| (g.title.as_str(), g.download_id.is_some()))
        .collect();
    assert_eq!(grabbed, [("Radiohead - OK Computer (1997) [FLAC]", true)]);
    let album: Option<i64> = sqlx::query_scalar("SELECT wanted_album_id FROM downloads")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(album, Some(10));

    // Each release is looked at once.
    let again = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((again.new_releases, again.grabbed.len()), (0, 0));
    // A better one arrives while the first is downloading: decided on, not grabbed.
    feed.lock().unwrap().0 =
        releases(&[("Radiohead - OK Computer (1997) [FLAC 24-96]", flac.as_str())]);
    let later = sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            later.new_releases,
            later.searches.len(),
            later.grabbed.len()
        ),
        (1, 1, 0)
    );
}

#[tokio::test]
async fn season_and_episode_switches_take_effect_for_rss_at_once() {
    use crate::wanted_edit::{set_episode_monitored, set_season_monitored};

    let pool = setup().await;
    let frieren = want_frieren(&pool, None).await;
    let release = release::parse("Frieren.S01E05.1080p.WEB-DL.mkv");
    let target = Target::Series {
        id: frieren,
        season: 1,
        episode: Some(5),
    };
    let why = || async { unmonitored(&pool, target, &release).await.unwrap() };
    assert_eq!(why().await, None);

    set_episode_monitored(&pool, frieren, 1, 5, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(why().await.as_deref(), Some("S01E05 isn't monitored"));
    set_episode_monitored(&pool, frieren, 1, 5, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(why().await, None);

    assert!(
        set_season_monitored(&pool, frieren, 1, false)
            .await
            .unwrap()
    );
    assert_eq!(why().await.as_deref(), Some("season 1 isn't monitored"));
}
