use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::config::Secret;
use crate::rss::tests::{indexer, releases, setup, want_dune, want_frieren};

struct Fixture {
    pool: SqlitePool,
    library: tempfile::TempDir,
    admin: String,
    viewer: String,
    dune: i64,
}

const FAKE: &str = "Dune.Part.Two.2024.1080p.WEBRip.x265-FAKE";
const GOOD: &str =
    "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR";

/// A movies library with two films: Dune: Part Two, which a download of `FAKE` put there, and
/// Arrival, adopted from the library as it was.
async fn fixture() -> Fixture {
    let pool = setup().await;
    let library = tempfile::tempdir().unwrap();
    let mut tokens = Vec::new();
    for (name, admin) in [("admin", true), ("viewer", false)] {
        let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        tokens.push(token);
    }
    let root = library.path();
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)")
        .bind(root.to_string_lossy())
        .execute(&pool)
        .await
        .unwrap();
    let dune = want_dune(&pool).await;
    for (id, folder, file) in [
        (1, "Dune Part Two (2024)", format!("{FAKE}.mkv")),
        (
            2,
            "Arrival (2016)",
            "Arrival.2016.1080p.BluRay.mkv".to_owned(),
        ),
    ] {
        let rel = format!("{folder}/{file}");
        std::fs::create_dir_all(root.join(folder)).unwrap();
        std::fs::write(root.join(&rel), b"a film, or not").unwrap();
        let meta = std::fs::metadata(root.join(&rel)).unwrap();
        let modified = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (?1, 1, ?2, ?2, 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
                 VALUES (?1, 1, ?3, ?4, ?5, ?1, 1, 1)",
        )
        .bind(id)
        .bind(folder)
        .bind(&rel)
        .bind(meta.len() as i64)
        .bind(modified)
        .execute(&pool)
        .await
        .unwrap();
    }
    let placed = root.join(format!("Dune Part Two (2024)/{FAKE}.mkv"));
    let download: i64 = sqlx::query_scalar(
        "INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, placed,
             added_at, updated_at)
         VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', ?, ?, '{}', 'imported',
             json_array(?), 0, 0) RETURNING id",
    )
    .bind(FAKE)
    .bind(dune)
    .bind(placed.to_string_lossy())
    .fetch_one(&pool)
    .await
    .unwrap();
    // As placing it records it.
    crate::transfer::record_placed(&pool, download, &placed)
        .await
        .unwrap();
    let viewer = tokens.pop().unwrap();
    Fixture {
        pool,
        library,
        admin: tokens.pop().unwrap(),
        viewer,
        dune,
    }
}

async fn send(app: &Router, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn only_a_file_melete_imported_goes_to_the_trash_and_the_title_is_searched_again() {
    let f = fixture().await;
    let root = f.library.path();
    let feed = indexer(&f.pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(FAKE, ""), (GOOD, "")]);
    let (url, deluge) = crate::deluge::testing::start().await;
    let app = routes(
        f.pool.clone(),
        Sources {
            pipeline: Some(Arc::new(crate::downloads::Pipeline {
                deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
                fetcher: crate::downloads::Fetcher::new().unwrap(),
                seedbox: None,
            })),
            ..Sources::default()
        },
    );
    let adopted = root.join("Arrival (2016)/Arrival.2016.1080p.BluRay.mkv");
    let imported = root.join(format!("Dune Part Two (2024)/{FAKE}.mkv"));
    assert_eq!(
        upgrades::imported_by(&f.pool, &adopted).await.unwrap(),
        None
    );
    assert!(
        upgrades::imported_by(&f.pool, &imported)
            .await
            .unwrap()
            .is_some()
    );

    // Admins only.
    let (status, _) = send(&app, "/api/v1/files/1/wrong", &f.viewer).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(imported.exists());

    // A file adopted from the library is never moved.
    let (status, body) = send(&app, "/api/v1/files/2/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("never moves"), "{body}");
    assert!(adopted.exists());
    let (status, _) = send(&app, "/api/v1/files/99/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["release"], FAKE);
    assert_eq!(body["search"], format!("searched again and grabbed {GOOD}"));
    assert!(!imported.exists(), "gone from the library");
    let trashed = root.join(format!(
        ".melete-trash/{}/Dune Part Two (2024)/{FAKE}.mkv",
        body["download_id"]
    ));
    assert_eq!(std::fs::read(&trashed).unwrap(), b"a film, or not");
    assert!(adopted.exists());
    let listed = upgrades::list(&f.pool).await.unwrap();
    assert_eq!(
        (listed.len(), listed[0].reason.as_str(), listed[0].id),
        (1, "wrong", body["trash_id"].as_i64().unwrap())
    );
    let missing: Option<i64> =
        sqlx::query_scalar("SELECT missing_since FROM media_files WHERE id = 1")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(missing.is_some());
    let blocked: Vec<String> = sqlx::query_scalar("SELECT title FROM blocklist")
        .fetch_all(&f.pool)
        .await
        .unwrap();
    assert_eq!(blocked, [FAKE]);
    let downloads: Vec<(String, String)> =
        sqlx::query_as("SELECT title, state FROM downloads WHERE wanted_movie_id = ? ORDER BY id")
            .bind(f.dune)
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        downloads,
        [
            (FAKE.to_owned(), "failed".to_owned()),
            (GOOD.to_owned(), "downloading".to_owned())
        ]
    );
    assert_eq!(deluge.lock().unwrap().torrents.len(), 1);

    // It's restorable, as an upgrade's old file is.
    upgrades::restore(&f.pool, body["trash_id"].as_i64().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(imported.exists() && !trashed.exists());
}

#[tokio::test]
async fn an_unmonitored_title_isnt_searched_again() {
    let f = fixture().await;
    sqlx::query("UPDATE wanted_movies SET monitored = 0")
        .execute(&f.pool)
        .await
        .unwrap();
    let app = routes(f.pool.clone(), Sources::default());
    let (status, body) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["search"],
        "it isn't monitored, so it wasn't searched for again"
    );
}

/// Dune's imported file.
fn imported(f: &Fixture) -> std::path::PathBuf {
    f.library
        .path()
        .join(format!("Dune Part Two (2024)/{FAKE}.mkv"))
}

#[tokio::test]
async fn another_file_put_at_a_placed_name_isnt_taken_for_melete_s() {
    let f = fixture().await;
    let app = routes(f.pool.clone(), Sources::default());
    let path = imported(&f);
    // Another file put back at the same name, with the same size and last change (an adopted
    // file restored from the trash, say): only its inode tells it apart.
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let other = path.with_extension("other");
    std::fs::write(&other, b"a film, or not").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&other, &path).unwrap();
    assert_eq!(upgrades::imported_by(&f.pool, &path).await.unwrap(), None);
    let (status, body) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(path.exists());
}

#[tokio::test]
async fn a_placed_file_changed_by_hand_isnt_taken_for_melete_s() {
    let f = fixture().await;
    let app = routes(f.pool.clone(), Sources::default());
    let path = imported(&f);
    std::fs::write(&path, b"someone's own copy, written over it").unwrap();
    assert_eq!(upgrades::imported_by(&f.pool, &path).await.unwrap(), None);
    let (status, _) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(path.exists());
}

#[tokio::test]
async fn a_file_whose_download_is_still_coming_in_is_refused() {
    let f = fixture().await;
    sqlx::query("UPDATE downloads SET state = 'transferring'")
        .execute(&f.pool)
        .await
        .unwrap();
    let app = routes(f.pool.clone(), Sources::default());
    let (status, body) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body.to_string().contains("still being brought in"),
        "{body}"
    );
    assert!(imported(&f).exists());
}

#[tokio::test]
async fn history_says_set_aside_not_upgraded() {
    let f = fixture().await;
    let app = routes(f.pool.clone(), Sources::default());
    let (status, _) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::OK);
    let events: Vec<(String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT kind, release, message FROM title_events ORDER BY id")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.0.as_str()).collect();
    assert!(!kinds.contains(&"upgraded"), "{events:?}");
    let set_aside = events
        .iter()
        .find(|e| e.0 == "set_aside")
        .expect("recorded");
    assert_eq!(
        set_aside.1.as_deref(),
        Some(format!("Dune Part Two (2024)/{FAKE}.mkv").as_str())
    );
    assert_eq!(
        set_aside.2.as_deref(),
        Some(format!("set aside as the wrong file, from {FAKE}").as_str())
    );
}

#[tokio::test]
async fn a_search_that_fails_after_the_move_is_said_not_an_error() {
    let f = fixture().await;
    let (url, _deluge) = crate::deluge::testing::start().await;
    let app = routes(
        f.pool.clone(),
        Sources {
            pipeline: Some(Arc::new(crate::downloads::Pipeline {
                deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
                fetcher: crate::downloads::Fetcher::new().unwrap(),
                seedbox: None,
            })),
            ..Sources::default()
        },
    );
    // Searching can't work: its indexers' table is gone.
    sqlx::query("DROP TABLE indexers")
        .execute(&f.pool)
        .await
        .unwrap();
    let (status, body) = send(&app, "/api/v1/files/1/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["search"]
            .as_str()
            .unwrap()
            .starts_with("searching again failed: "),
        "{body}"
    );
    assert!(!imported(&f).exists(), "it's in the trash");
}

#[tokio::test]
async fn one_wrong_episode_of_a_pack_is_searched_for_alone() {
    let f = fixture().await;
    let root = f.library.path();
    sqlx::query(
        "INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
         VALUES (7, 1, 'Frieren', 'Frieren', 1)",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let frieren = want_frieren(&f.pool, Some(7)).await;
    let download: i64 = sqlx::query_scalar(
        "INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
             added_at, updated_at)
         VALUES ('b11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Frieren.S01.1080p.WEB-GRP', ?, 1,
             '{}', 'imported', 0, 0) RETURNING id",
    )
    .bind(frieren)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    std::fs::create_dir_all(root.join("Frieren/Season 01")).unwrap();
    for (id, episode) in [(10, 5), (11, 6)] {
        let rel = format!("Frieren/Season 01/Frieren.S01E{episode:02}.mkv");
        std::fs::write(root.join(&rel), b"an episode").unwrap();
        let modified = std::fs::metadata(root.join(&rel))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        sqlx::query(
            "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                 season, episode, first_seen_generation, seen_generation)
             VALUES (?, 1, ?, 10, ?, 7, 1, ?, 1, 1)",
        )
        .bind(id)
        .bind(&rel)
        .bind(modified)
        .bind(episode)
        .execute(&f.pool)
        .await
        .unwrap();
        crate::transfer::record_placed(&f.pool, download, &root.join(&rel))
            .await
            .unwrap();
    }
    let feed = indexer(&f.pool, "[5000]").await;
    feed.lock().unwrap().0 = releases(&[]);
    let (url, _deluge) = crate::deluge::testing::start().await;
    let app = routes(
        f.pool.clone(),
        Sources {
            pipeline: Some(Arc::new(crate::downloads::Pipeline {
                deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
                fetcher: crate::downloads::Fetcher::new().unwrap(),
                seedbox: None,
            })),
            ..Sources::default()
        },
    );

    let (status, body) = send(&app, "/api/v1/files/10/wrong", &f.admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["release"], "Frieren.S01.1080p.WEB-GRP");
    let searched: Vec<(i64, i64, Option<i64>)> =
        sqlx::query_as("SELECT wanted_series_id, season, episode FROM searches ORDER BY id")
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert_eq!(searched, [(frieren, 1, Some(5))], "episode 5 alone");
    assert!(root.join("Frieren/Season 01/Frieren.S01E06.mkv").exists());
}
