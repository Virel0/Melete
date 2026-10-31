//! Downloads: sending an accepted search result to Deluge on the seedbox, and following it there
//! until Deluge has all of it. Copying it home and into the library is `transfer`'s part.
//!
//! A torrent hash is grabbed at most once: the table says what was already fetched, so nothing
//! left on the seedbox is downloaded again.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::LOCATION;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::deluge;

mod clearing;
mod copying;
mod fetching;
mod following;
mod grabbing;
mod records;
mod removing;

pub use clearing::*;
pub use copying::*;
pub use fetching::*;
pub use following::*;
pub use grabbing::*;
pub use records::*;
pub use removing::*;

/// The largest .torrent fetched.
const MAX_TORRENT_BYTES: usize = 10 * 1024 * 1024;

/// What downloading needs: Deluge, and a way to fetch torrents from indexers.
pub struct Pipeline {
    /// The seedbox's Deluge.
    pub deluge: deluge::Client,
    /// Fetches .torrent files from indexers' links.
    pub fetcher: Fetcher,
    /// None until the seedbox's SFTP is set up: downloads are followed, not fetched.
    pub seedbox: Option<crate::seedbox::Seedbox>,
}

/// How often Deluge is asked about downloads in progress.
const FOLLOW_EVERY: Duration = Duration::from_secs(60);

/// Follows downloads on the seedbox for as long as the server runs, and removes imported
/// torrents from Deluge when that's turned on.
pub async fn follow(pool: SqlitePool, pipeline: std::sync::Arc<Pipeline>) {
    loop {
        if let Err(err) = refresh(&pool, &pipeline.deluge).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't check downloads in Deluge"
            );
        }
        if let Err(err) = clear(&pool, &pipeline.deluge).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't remove imported torrents from Deluge"
            );
        }
        tokio::time::sleep(FOLLOW_EVERY).await;
    }
}

/// Removes imported torrents from Deluge as the settings say, read afresh each time.
async fn clear(pool: &SqlitePool, deluge: &deluge::Client) -> anyhow::Result<()> {
    if let Some(clearing) = Clearing::from_settings(pool).await? {
        clear_imported(pool, deluge, &clearing).await?;
    }
    Ok(())
}

/// Why a release wasn't grabbed.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// No release with that id was found in a search.
    NoRelease,
    /// It was turned down; the first reason.
    NotAccepted(String),
    NoLink,
    /// Already downloaded or downloading: that download's id.
    Already(i64),
    /// The indexer's link didn't give a torrent.
    Fetch(String),
    /// Deluge refused it or couldn't be reached: why.
    Deluge(String),
    /// A Soulseek result, and slskd isn't set up.
    Soulseek,
    /// slskd couldn't take it: why.
    Slskd(String),
    /// It's on the blocklist.
    Blocklisted,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoRelease => f.write_str("no such release"),
            Refused::NotAccepted(why) => write!(f, "it was turned down: {why}"),
            Refused::NoLink => f.write_str("the indexer gave no download link"),
            Refused::Already(id) => write!(f, "it's already download {id}"),
            Refused::Fetch(why) => write!(f, "couldn't get the torrent: {why}"),
            Refused::Deluge(why) => f.write_str(why),
            Refused::Soulseek => {
                f.write_str("it's a Soulseek result, and slskd isn't set up (MELETE_SLSKD_URL)")
            }
            Refused::Slskd(why) => f.write_str(why),
            Refused::Blocklisted => {
                f.write_str("it's on the blocklist; take it off the blocklist to grab it")
            }
        }
    }
}

/// Every download, newest first.
pub async fn list(pool: &SqlitePool) -> anyhow::Result<Vec<Download>> {
    let rows: Vec<DownloadRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{DOWNLOAD_COLUMNS} ORDER BY id DESC"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Download::from).collect())
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Redirect};

    use super::*;
    use crate::config::Secret;
    use crate::db;
    use crate::deluge::testing;

    #[tokio::test]
    async fn part_files_go_with_failed_downloads_but_never_mid_copy() {
        let pool = db::open_in_memory().await;
        let library = tempfile::tempdir().unwrap();
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)",
        )
        .bind(library.path().to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
        let part = |id: i64| {
            library
                .path()
                .join(format!("A (2020)/.A.mkv.{id}.melete-part"))
        };
        std::fs::create_dir_all(library.path().join("A (2020)")).unwrap();
        std::fs::write(library.path().join("A (2020)/A.mkv"), b"in the library").unwrap();
        for (id, state) in [(1, "transferring"), (2, "failed"), (3, "completed")] {
            std::fs::write(part(id), b"half").unwrap();
            sqlx::query(
                "INSERT INTO downloads (id, infohash, title, decision, state, partials, added_at, updated_at)
                 VALUES (?, lower(hex(randomblob(20))), 'A', '{}', ?, json_array(?), 0, 0)",
            )
            .bind(id)
            .bind(state)
            .bind(part(id).to_string_lossy().into_owned())
            .execute(&pool)
            .await
            .unwrap();
        }
        // Being copied home: stays, with its part file.
        assert!(!remove(&pool, 1).await.unwrap());
        assert!(part(1).exists());
        // Clearing failed ones takes theirs; removing a waiting one takes its own.
        assert_eq!(clear_failed(&pool, None).await.unwrap(), 1);
        assert!(!part(2).exists());
        assert!(remove(&pool, 3).await.unwrap());
        assert!(!part(3).exists());
        assert!(part(1).exists());
        assert_eq!(
            std::fs::read(library.path().join("A (2020)/A.mkv")).unwrap(),
            b"in the library"
        );
    }

    const MOVIE_HASH: &str = "a11bae70b2fdd9eb95b63e746f8594dce2650b0c";
    const MAGNET_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    /// An indexer serving the test torrent, redirects to it and to a magnet, and a login page.
    async fn indexer() -> String {
        let router = axum::Router::new()
            .route(
                "/t",
                axum::routing::get(|| async {
                    include_bytes!("../tests/fixtures/deluge/movie.torrent").to_vec()
                }),
            )
            .route(
                "/redirect",
                axum::routing::get(|| async { Redirect::to("/t") }),
            )
            .route(
                "/magnet",
                axum::routing::get(|| async {
                    Redirect::to(&format!("magnet:?xt=urn:btih:{MAGNET_HASH}&dn=x"))
                }),
            )
            .route(
                "/html",
                axum::routing::get(|| async { "<html>sign in</html>" }),
            )
            .route(
                "/inside",
                axum::routing::get(|| async { Redirect::to("http://192.168.1.1/admin") }),
            )
            .route(
                "/gone",
                axum::routing::get(|| async { StatusCode::NOT_FOUND.into_response() }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        url
    }

    /// A search with releases linking to `links`; returns their ids. The last is turned down.
    async fn releases(pool: &SqlitePool, links: &[String]) -> Vec<i64> {
        let wanted: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at)
             VALUES (1, 'Some Movie', 2024, 1, 'radarr', 0) RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let search: i64 = sqlx::query_scalar(
            "INSERT INTO searches (wanted_movie_id, searched_at, indexers) VALUES (?, ?, '[]') RETURNING id",
        )
        .bind(wanted)
        .bind(unix_now())
        .fetch_one(pool)
        .await
        .unwrap();
        let mut ids = Vec::new();
        for (i, link) in links.iter().enumerate() {
            let accepted = i + 1 < links.len();
            let decision = if accepted {
                r#"{"accepted":true,"rejections":[]}"#
            } else {
                r#"{"accepted":false,"rejections":["WEBDL-720p isn't allowed by 4K Preferred"]}"#
            };
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO release_decisions (search_id, title, download_url, accepted, decision)
                 VALUES (?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(search)
            .bind(format!("Some.Movie.2024.{i}"))
            .bind(link)
            .bind(accepted)
            .bind(decision)
            .fetch_one(pool)
            .await
            .unwrap();
            ids.push(id);
        }
        ids
    }

    async fn pipeline() -> (Pipeline, testing::Shared) {
        let (url, fake) = testing::start().await;
        let pipeline = Pipeline {
            deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
            fetcher: Fetcher::new().unwrap(),
            seedbox: None,
        };
        (pipeline, fake)
    }

    #[tokio::test]
    async fn grabs_accepted_releases_once() {
        let pool = db::open_in_memory().await;
        let base = indexer().await;
        let links = ["/redirect", "/magnet", "/html", "/gone", "/t"].map(|p| format!("{base}{p}"));
        let ids = releases(&pool, &links).await;
        let (p, fake) = pipeline().await;
        let grab = |id| grab(&pool, &p.deluge, &p.fetcher, id);

        let download = grab(ids[0]).await.unwrap().unwrap();
        assert_eq!(
            (download.infohash.as_deref(), download.state.as_str()),
            (Some(MOVIE_HASH), "downloading")
        );
        assert_eq!(download.title, "Some.Movie.2024.0");
        assert!(download.wanted_movie_id.is_some());
        // The same torrent again, whichever release links to it.
        assert_eq!(
            grab(ids[0]).await.unwrap(),
            Err(Refused::Already(download.id))
        );

        let magnet = grab(ids[1]).await.unwrap().unwrap();
        assert_eq!(magnet.infohash.as_deref(), Some(MAGNET_HASH));
        assert!(fake.lock().unwrap().torrents.contains_key(MAGNET_HASH));

        assert_eq!(
            grab(ids[2]).await.unwrap(),
            Err(Refused::Fetch(
                "the link gave something that isn't a torrent".into()
            ))
        );
        assert_eq!(
            grab(ids[3]).await.unwrap(),
            Err(Refused::Fetch("the indexer answered 404 Not Found".into()))
        );
        assert_eq!(
            grab(ids[4]).await.unwrap(),
            Err(Refused::NotAccepted(
                "WEBDL-720p isn't allowed by 4K Preferred".into()
            ))
        );
        assert_eq!(grab(999).await.unwrap(), Err(Refused::NoRelease));
        // The download keeps its decision after the search is gone.
        sqlx::query("DELETE FROM searches")
            .execute(&pool)
            .await
            .unwrap();
        let kept: String = sqlx::query_scalar("SELECT decision FROM downloads WHERE id = ?")
            .bind(download.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(kept.contains("\"accepted\":true"));
    }

    #[tokio::test]
    async fn never_grabs_a_blocklisted_release_under_any_name() {
        let pool = db::open_in_memory().await;
        let base = indexer().await;
        let links = ["/redirect", "/redirect", "/t"].map(|p| format!("{base}{p}"));
        let ids = releases(&pool, &links).await;
        let (p, fake) = pipeline().await;
        sqlx::query(
            "INSERT INTO blocklist (title, infohash, source, added_at)
             VALUES ('SOME.MOVIE.2024.0', ?, 'torrent', 0)",
        )
        .bind(MOVIE_HASH)
        .execute(&pool)
        .await
        .unwrap();
        // By its name, before the indexer is asked; then the same torrent under another name.
        for id in [ids[0], ids[1]] {
            assert_eq!(
                grab(&pool, &p.deluge, &p.fetcher, id).await.unwrap(),
                Err(Refused::Blocklisted)
            );
        }
        assert!(fake.lock().unwrap().torrents.is_empty());
        let downloads: i64 = sqlx::query_scalar("SELECT count(*) FROM downloads")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(downloads, 0);
    }

    #[tokio::test]
    async fn grabs_an_album_and_says_why_one_was_refused() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);
             INSERT INTO searches (id, wanted_album_id, searched_at, indexers) VALUES (1, 7, 0, '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let base = indexer().await;
        let mut ids = Vec::new();
        for (title, link, accepted, decision) in [
            (
                "Radiohead - OK Computer (1997) [FLAC]",
                format!("{base}/t"),
                true,
                r#"{"accepted":true,"quality":"FLAC","rank":1,"reasons":[]}"#,
            ),
            (
                "Radiohead - OK Computer (1997) [MP3 192]",
                format!("{base}/magnet"),
                false,
                r#"{"accepted":false,"quality":"MP3-192","rank":null,"reasons":["MP3-192 isn't in the profile"]}"#,
            ),
        ] {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO release_decisions (search_id, title, download_url, accepted, decision)
                 VALUES (1, ?, ?, ?, ?) RETURNING id",
            )
            .bind(title)
            .bind(link)
            .bind(accepted)
            .bind(decision)
            .fetch_one(&pool)
            .await
            .unwrap();
            ids.push(id);
        }
        let (p, _fake) = pipeline().await;
        let download = grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                download.wanted_album_id,
                download.wanted_movie_id,
                download.replaces.len()
            ),
            (Some(7), None, 0)
        );
        assert_eq!(
            grab(&pool, &p.deluge, &p.fetcher, ids[1]).await.unwrap(),
            Err(Refused::NotAccepted("MP3-192 isn't in the profile".into()))
        );
        // One wanted title per download, held by the database.
        let both = sqlx::query("UPDATE downloads SET wanted_movie_id = 1 WHERE id = ?")
            .bind(download.id)
            .execute(&pool)
            .await;
        assert!(both.unwrap_err().to_string().contains("one wanted title"));
    }

    #[tokio::test]
    async fn a_soulseek_result_isnt_sent_to_deluge() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);
             INSERT INTO searches (id, wanted_album_id, searched_at, indexers) VALUES (1, 7, 0, '[]');
             INSERT INTO release_decisions (id, search_id, title, accepted, decision, soulseek)
                 VALUES (5, 1, 'Radiohead - OK Computer', 1, '{\"accepted\":true,\"reasons\":[]}',
                         '{\"username\":\"alice\",\"folder\":\"a\",\"files\":[]}');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (p, fake) = pipeline().await;
        assert_eq!(
            grab(&pool, &p.deluge, &p.fetcher, 5).await.unwrap(),
            Err(Refused::Soulseek)
        );
        assert!(fake.lock().unwrap().torrents.is_empty());
    }

    #[tokio::test]
    async fn deluge_following_leaves_soulseek_downloads_alone() {
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO downloads (source, soulseek, title, decision, state, message, added_at, updated_at)
                 VALUES ('soulseek', '{\"username\":\"a\",\"folder\":\"f\",\"parts\":[]}', 'A - One', '{}',
                         'downloading', 'queued with slskd', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (p, _fake) = pipeline().await;
        assert_eq!(refresh(&pool, &p.deluge).await.unwrap(), 0);
        let (state, message, missing): (String, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT state, message, missing_since FROM downloads")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (state.as_str(), message.as_deref(), missing),
            ("downloading", Some("queued with slskd"), None)
        );
    }

    #[tokio::test]
    async fn a_torrent_counts_as_added_here_only_when_melete_added_it() {
        let added_here = |pool: SqlitePool| async move {
            sqlx::query_scalar::<_, bool>("SELECT added_here FROM downloads")
                .fetch_one(&pool)
                .await
                .unwrap()
        };
        let base = indexer().await;
        let pool = db::open_in_memory().await;
        let ids = releases(&pool, &[format!("{base}/t"), String::new()]).await;
        let (p, _fake) = pipeline().await;
        grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        assert!(added_here(pool).await);

        // In Deluge already (another app's, or added by hand): someone else's.
        let pool = db::open_in_memory().await;
        let ids = releases(&pool, &[format!("{base}/t"), String::new()]).await;
        let (p, fake) = pipeline().await;
        fake.lock().unwrap().torrents.insert(
            MOVIE_HASH.into(),
            testing::status("Some.Movie", "Downloading", 40.0, false),
        );
        grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        assert!(!added_here(pool).await);
    }

    #[tokio::test]
    async fn follows_a_torrent_already_in_deluge() {
        let pool = db::open_in_memory().await;
        let ids = releases(&pool, &[format!("{}/t", indexer().await), String::new()]).await;
        let (p, fake) = pipeline().await;
        fake.lock().unwrap().torrents.insert(
            MOVIE_HASH.into(),
            testing::status("Some.Movie", "Downloading", 40.0, false),
        );
        let download = grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            download.message.as_deref(),
            Some("it was already in Deluge")
        );
    }

    #[tokio::test]
    async fn marks_downloads_complete_or_failed_as_deluge_says() {
        let pool = db::open_in_memory().await;
        let base = indexer().await;
        let ids = releases(
            &pool,
            &[format!("{base}/t"), format!("{base}/magnet"), String::new()],
        )
        .await;
        let (p, fake) = pipeline().await;
        let movie = grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        let magnet = grab(&pool, &p.deluge, &p.fetcher, ids[1])
            .await
            .unwrap()
            .unwrap();

        fake.lock().unwrap().torrents.insert(
            MOVIE_HASH.into(),
            testing::status("Some.Movie", "Downloading", 42.5, false),
        );
        assert_eq!(refresh(&pool, &p.deluge).await.unwrap(), 2);
        let now = get(&pool, movie.id).await.unwrap().unwrap();
        assert_eq!(
            (now.state.as_str(), now.progress, now.message.as_deref()),
            ("downloading", 42.5, Some("Downloading"))
        );

        // The movie finishes; the magnet's torrent is removed from Deluge.
        let finished = testing::status("Some.Movie", "Seeding", 100.0, true);
        {
            let mut fake = fake.lock().unwrap();
            fake.torrents.insert(MOVIE_HASH.into(), finished);
            fake.torrents.remove(MAGNET_HASH);
        }
        refresh(&pool, &p.deluge).await.unwrap();
        let done = get(&pool, movie.id).await.unwrap().unwrap();
        assert_eq!((done.state.as_str(), done.progress), ("completed", 100.0));
        assert!(done.completed_at.is_some());
        let (save_path, files): (String, String) =
            sqlx::query_as("SELECT save_path, files FROM downloads WHERE id = ?")
                .bind(movie.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(save_path, "/home/user/Completed");
        assert_eq!(
            files,
            r#"[{"path":"Some.Movie/Some.Movie.mkv","size":300000}]"#
        );
        // Missing once isn't gone: Deluge may be starting up.
        let missing = get(&pool, magnet.id).await.unwrap().unwrap();
        assert_eq!(
            (missing.state.as_str(), missing.message.as_deref()),
            ("downloading", Some("Deluge doesn't list it at the moment"))
        );
        // Missing for half an hour is.
        sqlx::query("UPDATE downloads SET missing_since = missing_since - 1800 WHERE id = ?")
            .bind(magnet.id)
            .execute(&pool)
            .await
            .unwrap();
        refresh(&pool, &p.deluge).await.unwrap();
        let gone = get(&pool, magnet.id).await.unwrap().unwrap();
        assert_eq!(
            (gone.state.as_str(), gone.message.as_deref()),
            (
                "failed",
                Some("it was removed from Deluge before it finished")
            )
        );
        // Finished and failed downloads aren't asked about again.
        assert_eq!(refresh(&pool, &p.deluge).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn follows_the_hash_deluge_gives_and_waits_out_failed_moves() {
        let pool = db::open_in_memory().await;
        let base = indexer().await;
        let ids = releases(
            &pool,
            &[format!("{base}/t"), format!("{base}/inside"), String::new()],
        )
        .await;
        let (p, fake) = pipeline().await;
        // A hybrid torrent: Deluge names it by its v2 hash, not the SHA-1 Melete computes.
        let v2 = "00864f0123456789abcdef0123456789abcdef01";
        fake.lock().unwrap().name_next = Some(v2.into());
        let download = grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(download.infohash.as_deref(), Some(v2));
        // Finished, but Deluge couldn't move it: not complete.
        let mut failed_move = testing::status("Some.Movie", "Error", 100.0, true);
        failed_move["message"] = "Move storage failed".into();
        fake.lock().unwrap().torrents.insert(v2.into(), failed_move);
        refresh(&pool, &p.deluge).await.unwrap();
        let now = get(&pool, download.id).await.unwrap().unwrap();
        assert_eq!(
            (now.state.as_str(), now.message.as_deref()),
            ("downloading", Some("Deluge: Move storage failed"))
        );
        // Grabbing it again is refused, by Deluge's hash.
        let again = grab(&pool, &p.deluge, &p.fetcher, ids[0]).await.unwrap();
        assert_eq!(again, Err(Refused::Already(download.id)));
        // A link can't redirect into the local network.
        assert_eq!(
            grab(&pool, &p.deluge, &p.fetcher, ids[1]).await.unwrap(),
            Err(Refused::Fetch("a redirect to a local address".into()))
        );
    }

    #[tokio::test]
    async fn an_upgrade_records_the_file_it_replaces() {
        let pool = db::open_in_memory().await;
        let ids = releases(
            &pool,
            &[
                format!("magnet:?xt=urn:btih:{MAGNET_HASH}&dn=x"),
                "unused".into(),
            ],
        )
        .await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Some Movie (2024)', 'Some Movie', 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Some Movie (2024)/Some.Movie.2024.720p.mkv', 1, 0, 1, 1, 1);
             UPDATE wanted_movies SET movie_id = 1",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (p, _fake) = pipeline().await;
        let download = grab(&pool, &p.deluge, &p.fetcher, ids[0])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            download.replaces,
            ["Some Movie (2024)/Some.Movie.2024.720p.mkv"]
        );
        assert_eq!(list(&pool).await.unwrap()[0].replaces, download.replaces);
    }
}
