use axum::http::header;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::TokenKind;
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};

struct Fixture {
    pool: SqlitePool,
    app: Router,
    token: String,
    token_id: i64,
    _library: tempfile::TempDir,
    _outside: tempfile::TempDir,
}

const CONTENT: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// File 1: a movie with real bytes. File 2: a missing file. File 3: a symlink inside the
/// library pointing outside it. File 4: an episode.
async fn fixture() -> Fixture {
    let library = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(library.path().join("Movie (2001)")).unwrap();
    std::fs::write(library.path().join("Movie (2001)/movie.mkv"), CONTENT).unwrap();
    std::fs::write(outside.path().join("secret.mkv"), b"secret").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.mkv"),
        library.path().join("Movie (2001)/link.mkv"),
    )
    .unwrap();

    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: library.path().into(),
    };
    sync_config(&pool, &[config]).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation)
                 VALUES (1, 1, 'Movie (2001)', 'Movie', 2001, 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation, missing_since) VALUES
                 (1, 1, 'Movie (2001)/movie.mkv', 36, 1, 1, 1, 1, NULL),
                 (2, 1, 'Movie (2001)/gone.mkv', 1, 1, 1, 1, 1, 50),
                 (3, 1, 'Movie (2001)/link.mkv', 6, 1, 1, 1, 1, NULL);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let user = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap();
    Fixture {
        app: routes(pool.clone(), "ffprobe".into()),
        pool,
        token,
        token_id,
        _library: library,
        _outside: outside,
    }
}

async fn send(f: &Fixture, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", f.token));
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn fetch(f: &Fixture, uri: &str, range: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(range) = range {
        request = request.header(header::RANGE, range);
    }
    f.app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn stream_url(f: &Fixture, file: i64) -> String {
    let (status, body) = send(f, "POST", &format!("/api/v1/files/{file}/play"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["stream_url"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn streams_whole_files_and_ranges() {
    let f = fixture().await;
    let url = stream_url(&f, 1).await;
    assert!(url.starts_with("/api/v1/stream/mls_"));

    let whole = fetch(&f, &url, None).await;
    assert_eq!(whole.status(), StatusCode::OK);
    assert_eq!(whole.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(
        &whole.into_body().collect().await.unwrap().to_bytes()[..],
        CONTENT
    );

    let part = fetch(&f, &url, Some("bytes=10-19")).await;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 10-19/36");
    assert_eq!(
        &part.into_body().collect().await.unwrap().to_bytes()[..],
        b"abcdefghij"
    );
}

#[tokio::test]
async fn stream_links_expire_and_die_with_their_login() {
    let f = fixture().await;
    assert_eq!(
        fetch(&f, "/api/v1/stream/mls_made-up", None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fetch(&f, "/api/v1/stream/mlt_wrong-kind", None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    let expired = stream_url(&f, 1).await;
    sqlx::query("UPDATE play_sessions SET expires_at = 0")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        fetch(&f, &expired, None).await.status(),
        StatusCode::NOT_FOUND
    );

    let revoked = stream_url(&f, 1).await;
    assert_eq!(fetch(&f, &revoked, None).await.status(), StatusCode::OK);
    sqlx::query("DELETE FROM tokens WHERE id = ?")
        .bind(f.token_id)
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        fetch(&f, &revoked, None).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn never_streams_outside_the_library() {
    let f = fixture().await;
    let url = stream_url(&f, 3).await;
    let response = fetch(&f, &url, None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(!body.windows(6).any(|w| w == b"secret"));
}

#[tokio::test]
async fn play_needs_a_present_file_and_a_user() {
    let f = fixture().await;
    assert_eq!(
        send(&f, "POST", "/api/v1/files/2/play", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&f, "POST", "/api/v1/files/99/play", None).await.0,
        StatusCode::NOT_FOUND
    );
    let anonymous = Request::post("/api/v1/files/1/play")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        f.app.clone().oneshot(anonymous).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn progress_resumes_then_finishes_once_per_viewing() {
    let f = fixture().await;
    let report = |position: f64, duration: Option<f64>| json!({ "position_seconds": position, "duration_seconds": duration });
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(100.0, Some(1000.0))),
    )
    .await;
    assert_eq!(
        p,
        json!({ "position_seconds": 100.0, "duration_seconds": 1000.0, "watched": false, "play_count": 0 })
    );
    let (_, play) = send(&f, "POST", "/api/v1/files/1/play", None).await;
    assert_eq!(
        (play["resume_seconds"].as_f64(), play["watched"].as_bool()),
        (Some(100.0), Some(false))
    );

    // The duration is remembered; 90% finishes it.
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(905.0, None)),
    )
    .await;
    assert_eq!(
        p,
        json!({ "position_seconds": 0.0, "duration_seconds": 1000.0, "watched": true, "play_count": 1 })
    );
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(960.0, None)),
    )
    .await;
    assert_eq!(
        p["play_count"], 1,
        "reports near the end of the same viewing don't count again"
    );

    // Rewinding past 90% and crossing it again is the same viewing.
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(850.0, None)),
    )
    .await;
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(950.0, None)),
    )
    .await;
    assert_eq!(p["play_count"], 1, "rewinding doesn't count a second play");

    // Watching again starts with a new stream link.
    stream_url(&f, 1).await;
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(30.0, None)),
    )
    .await;
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(report(990.0, None)),
    )
    .await;
    assert_eq!(
        (p["watched"].as_bool(), p["play_count"].as_i64()),
        (Some(true), Some(2))
    );
}

#[tokio::test]
async fn playing_without_a_stream_link_counts_if_not_yet_watched() {
    let f = fixture().await;
    let report = json!({ "position_seconds": 95.0, "duration_seconds": 100.0 });
    let (_, p) = send(&f, "PUT", "/api/v1/files/1/progress", Some(report.clone())).await;
    assert_eq!(p["play_count"], 1);
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(json!({ "position_seconds": 10.0 })),
    )
    .await;
    let (_, p) = send(&f, "PUT", "/api/v1/files/1/progress", Some(report)).await;
    assert_eq!(p["play_count"], 1, "already watched");
}

#[tokio::test]
async fn a_progress_report_does_not_undo_unwatched_from_another_device() {
    let f = fixture().await;
    send(
        &f,
        "PUT",
        "/api/v1/files/1/watched",
        Some(json!({ "watched": true })),
    )
    .await;
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(json!({ "position_seconds": 10.0 })),
    )
    .await;
    // Another device marks it unwatched; this one keeps reporting.
    send(
        &f,
        "PUT",
        "/api/v1/files/1/watched",
        Some(json!({ "watched": false })),
    )
    .await;
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(json!({ "position_seconds": 20.0 })),
    )
    .await;
    assert_eq!(
        (p["watched"].as_bool(), p["position_seconds"].as_f64()),
        (Some(false), Some(20.0))
    );
}

#[tokio::test]
async fn a_report_back_at_the_start_keeps_the_saved_position_unless_restarting() {
    let f = fixture().await;
    let report = |body: Value| send(&f, "PUT", "/api/v1/files/1/progress", Some(body));
    report(json!({ "position_seconds": 1200.0, "duration_seconds": 3000.0 })).await;
    for position in [0.0, 4.9] {
        let (status, p) = report(json!({ "position_seconds": position })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(p["position_seconds"], 1200.0, "{position} is ignored");
    }
    let (_, p) = report(json!({ "position_seconds": 5.0 })).await;
    assert_eq!(p["position_seconds"], 5.0, "a real rewind is kept");
    let (_, p) = report(json!({ "position_seconds": 0.0 })).await;
    assert_eq!(
        p["position_seconds"], 0.0,
        "nothing much is lost under a minute in"
    );

    report(json!({ "position_seconds": 600.0 })).await;
    let (_, p) = report(json!({ "position_seconds": 0.0, "restart": true })).await;
    assert_eq!(p["position_seconds"], 0.0, "starting over is deliberate");
    let (_, p) = send(&f, "POST", "/api/v1/files/1/play", None).await;
    assert_eq!(p["resume_seconds"], 0.0);
}

#[tokio::test]
async fn stream_links_per_session_are_capped() {
    let f = fixture().await;
    for _ in 0..MAX_STREAMS_PER_TOKEN + 5 {
        stream_url(&f, 1).await;
    }
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM play_sessions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(live, MAX_STREAMS_PER_TOKEN);
}

#[tokio::test]
async fn progress_rejects_impossible_values() {
    let f = fixture().await;
    for body in [
        json!({ "position_seconds": -1.0 }),
        json!({ "position_seconds": 1e9 }),
        json!({ "position_seconds": 10.0, "duration_seconds": 0.0 }),
    ] {
        let (status, _) = send(&f, "PUT", "/api/v1/files/1/progress", Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    let (status, _) = send(
        &f,
        "PUT",
        "/api/v1/files/2/progress",
        Some(json!({ "position_seconds": 1.0 })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn marking_watched_by_hand() {
    let f = fixture().await;
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(json!({ "position_seconds": 50.0 })),
    )
    .await;
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/watched",
        Some(json!({ "watched": true })),
    )
    .await;
    assert_eq!(
        p,
        json!({ "position_seconds": 0.0, "duration_seconds": null, "watched": true, "play_count": 1 })
    );
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/watched",
        Some(json!({ "watched": true })),
    )
    .await;
    assert_eq!(p["play_count"], 1, "already watched");
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/1/watched",
        Some(json!({ "watched": false })),
    )
    .await;
    assert_eq!(
        (p["watched"].as_bool(), p["play_count"].as_i64()),
        (Some(false), Some(1))
    );
}

/// Series 1 (library 2): S01E01 (10), S01E02 (11), a special (12), S02E01 (13), a missing
/// S01E03 (14) and a file named as no episode (15). Series 2 (library 3): S01E01 (20).
async fn series_fixture() -> Fixture {
    let f = fixture().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (2, 'shows', 'shows', '/shows'),
             (3, 'kids', 'shows', '/kids');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES
             (1, 2, 'Show', 'Show', 1), (2, 3, 'Kids', 'Kids', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id, season,
             episode, first_seen_generation, seen_generation, missing_since) VALUES
             (10, 2, 'Show/S01E01.mkv', 1, 1, 1, 1, 1, 1, 1, NULL),
             (11, 2, 'Show/S01E02.mkv', 1, 1, 1, 1, 2, 1, 1, NULL),
             (12, 2, 'Show/S00E01.mkv', 1, 1, 1, 0, 1, 1, 1, NULL),
             (13, 2, 'Show/S02E01.mkv', 1, 1, 1, 2, 1, 1, 1, NULL),
             (14, 2, 'Show/S01E03.mkv', 1, 1, 1, 1, 3, 1, 1, 50),
             (15, 2, 'Show/Extra bit.mkv', 1, 1, 1, NULL, NULL, 1, 1, NULL),
             (20, 3, 'Kids/S01E01.mkv', 1, 1, 2, 1, 1, 1, 1, NULL);",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    f
}

/// The viewer's files marked watched, by id.
async fn watched_files(f: &Fixture) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT p.file_id FROM playback_progress p JOIN users u ON u.id = p.user_id
         WHERE u.username = 'viewer' AND p.watched ORDER BY p.file_id",
    )
    .fetch_all(&f.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn marking_a_season_or_a_series_watched_in_one_request() {
    let f = series_fixture().await;
    let mark = |body: Value| send(&f, "PUT", "/api/v1/series/1/watched", Some(body));
    // Partway through S01E01 first: marking the season finishes it.
    send(
        &f,
        "PUT",
        "/api/v1/files/10/progress",
        Some(json!({ "position_seconds": 50.0 })),
    )
    .await;

    let (status, body) = mark(json!({ "watched": true, "season": 1 })).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "changed": 2 })));
    let spec: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
    let documented: Vec<&String> = spec["components"]["schemas"]["SeriesWatched"]["properties"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(
        documented,
        body.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    assert_eq!(
        watched_files(&f).await,
        [10, 11],
        "season 1's present files only"
    );
    let (_, p) = send(&f, "GET", "/api/v1/me/progress?series_id=1", None).await;
    assert_eq!(
        (
            p[0]["position_seconds"].as_f64(),
            p[0]["play_count"].as_i64()
        ),
        (Some(0.0), Some(1))
    );
    assert_eq!(
        mark(json!({ "watched": true, "season": 1 })).await.1["changed"],
        0
    );

    // The whole series takes in the specials, never the missing file, the file named as no
    // episode, or another series' file.
    assert_eq!(mark(json!({ "watched": true })).await.1["changed"], 2);
    assert_eq!(watched_files(&f).await, [10, 11, 12, 13]);

    assert_eq!(
        mark(json!({ "watched": false, "season": 1 })).await.1["changed"],
        2
    );
    assert_eq!(watched_files(&f).await, [12, 13]);
    assert_eq!(
        mark(json!({ "watched": true, "season": 0 })).await.1["changed"],
        0
    );
    assert_eq!(mark(json!({ "watched": false })).await.1["changed"], 2);
    assert!(watched_files(&f).await.is_empty());
    let (_, p) = send(&f, "GET", "/api/v1/me/progress?series_id=1", None).await;
    assert_eq!(p[0]["play_count"], 1, "unmarking keeps the count");

    for (body, status) in [
        (
            json!({ "watched": true, "seasons": 1 }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "watched": true, "season": -1 }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(mark(body.clone()).await.0, status, "{body}");
    }
    let (status, _) = send(
        &f,
        "PUT",
        "/api/v1/series/999/watched",
        Some(json!({ "watched": true })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(watched_files(&f).await.is_empty());
}

#[tokio::test]
async fn a_series_hidden_from_a_user_cannot_be_marked() {
    let f = series_fixture().await;
    let restriction = crate::access::Restriction {
        libraries: Some(vec![3]),
        max_age: None,
    };
    let viewer: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'viewer'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    crate::access::set_restriction(&f.pool, viewer, Some(&restriction))
        .await
        .unwrap();
    let body = Some(json!({ "watched": true }));
    let (status, _) = send(&f, "PUT", "/api/v1/series/1/watched", body.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(watched_files(&f).await.is_empty());
    let (status, changed) = send(&f, "PUT", "/api/v1/series/2/watched", body).await;
    assert_eq!((status, changed), (StatusCode::OK, json!({ "changed": 1 })));
    assert_eq!(watched_files(&f).await, [20]);
}

#[tokio::test]
async fn continue_watching_and_per_title_progress() {
    let f = fixture().await;
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(json!({ "position_seconds": 42.0, "duration_seconds": 100.0 })),
    )
    .await;
    send(
        &f,
        "PUT",
        "/api/v1/files/3/watched",
        Some(json!({ "watched": true })),
    )
    .await;

    let (status, items) = send(&f, "GET", "/api/v1/me/continue", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        items.as_array().unwrap().len(),
        1,
        "watched files aren't partway through"
    );
    assert_eq!(items[0]["file_id"], 1);
    assert_eq!(items[0]["title"], "Movie");
    assert_eq!(items[0]["movie_id"], 1);
    assert_eq!(items[0]["position_seconds"], 42.0);

    let (_, entries) = send(&f, "GET", "/api/v1/me/progress?movie_id=1", None).await;
    let files: Vec<i64> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["file_id"].as_i64().unwrap())
        .collect();
    assert_eq!(files, [1, 3]);
    assert_eq!(entries[1]["watched"], true);
    sqlx::query("UPDATE media_files SET missing_since = 1 WHERE id = 3")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, entries) = send(&f, "GET", "/api/v1/me/progress?movie_id=1", None).await;
    assert_eq!(
        entries.as_array().unwrap().len(),
        1,
        "missing files are hidden"
    );
    for bad in ["", "?movie_id=1&series_id=1"] {
        let (status, _) = send(&f, "GET", &format!("/api/v1/me/progress{bad}"), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

async fn add_file(f: &Fixture, id: i64, name: &str, bytes: &[u8]) {
    let path = f._library.path().join("Movie (2001)").join(name);
    std::fs::write(&path, bytes).unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation) VALUES (?, 1, ?, ?, 1, 1, 1, 1)",
    )
    .bind(id)
    .bind(format!("Movie (2001)/{name}"))
    .bind(bytes.len() as i64)
    .execute(&f.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn file_info_probes_on_request_and_remembers() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let clip = std::fs::read(crate::probe::testing::make_clip(dir.path())).unwrap();
    add_file(&f, 4, "clip.mkv", &clip).await;

    let (status, body) = send(&f, "GET", "/api/v1/files/4", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["path"], "Movie (2001)/clip.mkv");
    assert_eq!(body["media"]["video"][0]["codec"], "h264");
    assert_eq!(body["media"]["audio"][0]["language"], "eng");

    // Stored: answered from the database even with the file gone from disk.
    std::fs::remove_file(f._library.path().join("Movie (2001)/clip.mkv")).unwrap();
    let (_, again) = send(&f, "GET", "/api/v1/files/4", None).await;
    assert_eq!(again["media"], body["media"]);
}

#[tokio::test]
async fn file_info_for_a_broken_file_has_no_media() {
    let f = fixture().await;
    let (status, body) = send(&f, "GET", "/api/v1/files/1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["media"], Value::Null);
    let error: Option<String> =
        sqlx::query_scalar("SELECT error FROM media_probes WHERE file_id = 1")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(error.is_some(), "the failure is remembered");
    assert_eq!(
        send(&f, "GET", "/api/v1/files/2", None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn progress_uses_the_probed_duration() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let clip = std::fs::read(crate::probe::testing::make_clip(dir.path())).unwrap();
    add_file(&f, 4, "clip.mkv", &clip).await;
    send(&f, "GET", "/api/v1/files/4", None).await;

    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/4/progress",
        Some(json!({ "position_seconds": 1.95 })),
    )
    .await;
    assert_eq!(p["watched"], true, "{p}");
    assert_eq!(
        p["duration_seconds"],
        Value::Null,
        "the probed duration isn't saved as the user's"
    );

    // A probe of an older version of the file doesn't count.
    sqlx::query("UPDATE media_files SET size = size + 1 WHERE id = 4")
        .execute(&f.pool)
        .await
        .unwrap();
    stream_url(&f, 4).await;
    let (_, p) = send(
        &f,
        "PUT",
        "/api/v1/files/4/progress",
        Some(json!({ "position_seconds": 1.95 })),
    )
    .await;
    assert_eq!(
        p["position_seconds"], 1.95,
        "not finished without a current duration: {p}"
    );
}

#[tokio::test]
async fn progress_is_per_user() {
    let f = fixture().await;
    send(
        &f,
        "PUT",
        "/api/v1/files/1/progress",
        Some(json!({ "position_seconds": 42.0 })),
    )
    .await;
    let other = auth::create_user(&f.pool, "other", "other-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = auth::issue_token(&f.pool, other.id, TokenKind::Session, "t")
        .await
        .unwrap();
    let request = Request::get("/api/v1/me/continue")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let bytes = f
        .app
        .clone()
        .oneshot(request)
        .await
        .unwrap()
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(&bytes[..], b"[]");
}

#[tokio::test]
async fn extras_play_like_files_but_keep_no_progress() {
    let f = fixture().await;
    let trailers = f._library.path().join("Movie (2001)/Trailers");
    std::fs::create_dir_all(&trailers).unwrap();
    let clip = trailers.join("Teaser.mkv");
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=d=1:s=64x48",
        ])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());
    let size = std::fs::metadata(&clip).unwrap().len() as i64;
    let id = crate::library::extras::ID_BASE;
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation)
         VALUES (?, 1, 'Movie (2001)/Trailers/Teaser.mkv', ?, 1, 1, 'trailer', 'Teaser', 1, 1)",
    )
    .bind(id)
    .bind(size)
    .execute(&f.pool)
    .await
    .unwrap();

    let (status, info) = send(&f, "GET", &format!("/api/v1/files/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert!(
        info["media"]["duration_seconds"].as_f64().unwrap() > 0.5,
        "probed"
    );
    let probed: Option<i64> =
        sqlx::query_scalar("SELECT probed_size FROM media_extras WHERE id = ?")
            .bind(id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(probed, Some(size), "and its probe kept");

    let url = stream_url(&f, id).await;
    let whole = fetch(&f, &url, None).await;
    assert_eq!(whole.status(), StatusCode::OK);
    let bytes = whole.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.len() as i64, size);
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM play_sessions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(files, 0, "an extra's link is kept apart from files' links");

    let progress = json!({ "position_seconds": 0.5 });
    let uri = format!("/api/v1/files/{id}/progress");
    assert_eq!(
        send(&f, "PUT", &uri, Some(progress)).await.0,
        StatusCode::NOT_FOUND,
        "no progress is kept for extras"
    );

    sqlx::query("UPDATE media_extras SET missing_since = 5")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(fetch(&f, &url, None).await.status(), StatusCode::NOT_FOUND);
    let uri = format!("/api/v1/files/{id}/play");
    assert_eq!(send(&f, "POST", &uri, None).await.0, StatusCode::NOT_FOUND);
}
