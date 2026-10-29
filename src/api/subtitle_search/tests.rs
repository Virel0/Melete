use std::sync::atomic::Ordering;

use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::library::{LibraryConfig, LibraryKind, sync_config};
use crate::opensubtitles::testing::{self, Fake};

struct Fixture {
    app: Router,
    pool: SqlitePool,
    token: String,
    token_id: i64,
    fake: Arc<Fake>,
    _dir: tempfile::TempDir,
}

/// File 1, a clip of the movie TMDB knows as 603, with OpenSubtitles' stand-in set up when
/// `on`.
async fn fixture(on: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 5);
    std::fs::rename(&clip, library.join("clip.mkv")).unwrap();
    let size = std::fs::metadata(library.join("clip.mkv")).unwrap().len() as i64;
    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: library,
    };
    sync_config(&pool, &[config]).await.unwrap();
    sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, tmdb_id, seen_generation) VALUES (1, 1, 'clip.mkv', 'Clip', 603, 1)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, 'clip.mkv', ?, 1, 1, 1, 1)",
    )
    .bind(size)
    .execute(&pool)
    .await
    .unwrap();
    if on {
        sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_OPENSUBTITLES_KEY', ?, 0)")
            .bind(testing::KEY)
            .execute(&pool)
            .await
            .unwrap();
    }
    let user = auth::create_user(&pool, "alice", "alice-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap();
    let (base, fake) = testing::start().await;
    let subtitles = dir.path().join("subtitles");
    let app = routes_at(pool.clone(), "ffprobe".into(), subtitles.clone(), &base).merge(
        super::super::subtitles::routes(pool.clone(), "ffprobe".into(), "ffmpeg".into(), subtitles),
    );
    Fixture {
        app,
        pool,
        token,
        token_id,
        fake,
        _dir: dir,
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
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

fn calls(f: &Fixture, path: &str) -> usize {
    f.fake
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.path == path)
        .count()
}

#[tokio::test]
async fn nothing_is_asked_while_its_off() {
    let f = fixture(false).await;
    let (status, body) = send(
        &f,
        "POST",
        "/api/v1/files/1/subtitles/search?language=fr",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(f.fake.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_search_is_shown_then_one_result_fetched_once() {
    let f = fixture(true).await;
    let search = "/api/v1/files/1/subtitles/search?language=FR";
    let (status, found) = send(&f, "POST", search, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["language"], "fr");
    assert_eq!(found["remembered"], false);
    assert_eq!(found["results"][0]["id"], 102);
    assert_eq!(found["results"][0]["matches_file"], true);
    assert_eq!(found["fetched"], Value::Null);
    let query = f.fake.seen.lock().unwrap()[0].query.clone();
    assert!(query.starts_with("languages=fr&moviehash="), "{query}");
    assert!(query.ends_with("&tmdb_id=603"), "{query}");

    let (_, again) = send(&f, "POST", search, None).await;
    assert_eq!(again["remembered"], true);
    assert_eq!(calls(&f, "/subtitles"), 1, "remembered, not sent again");

    let fetch = "/api/v1/files/1/subtitles/fetch";
    let (status, _) = send(
        &f,
        "POST",
        fetch,
        Some(json!({ "language": "fr", "result": 999 })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "only a result of the search");
    let (status, track) = send(
        &f,
        "POST",
        fetch,
        Some(json!({ "language": "fr", "result": 102 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{track}");
    assert_eq!(track["source"], "opensubtitles");
    assert_eq!(track["title"], "This.Very.File");
    let id = track["track"].as_str().unwrap().to_owned();
    assert!(id.starts_with('o') && id.len() == 9, "{id}");
    let (_, same) = send(
        &f,
        "POST",
        fetch,
        Some(json!({ "language": "fr", "result": 101 })),
    )
    .await;
    assert_eq!(same, track, "one per file and language");
    assert_eq!(calls(&f, "/download"), 1);

    let (_, list) = send(&f, "GET", "/api/v1/files/1/subtitles", None).await;
    assert_eq!(list.as_array().unwrap().last().unwrap(), &track);
    let Ok((stream, _)) =
        crate::api::playback::create_play_session(&f.pool, f.token_id, 1, None).await
    else {
        panic!("creating a stream link failed");
    };
    let (status, vtt) = send(
        &f,
        "GET",
        &format!("/api/v1/stream/{stream}/subtitles/{id}.vtt"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let vtt = vtt.as_str().unwrap();
    assert!(
        vtt.starts_with("WEBVTT") && vtt.contains("Fetched 102.srt"),
        "{vtt}"
    );
}

#[tokio::test]
async fn used_up_downloads_stop_fetching_until_they_come_back() {
    let f = fixture(true).await;
    f.fake.quota_reached.store(true, Ordering::Relaxed);
    send(
        &f,
        "POST",
        "/api/v1/files/1/subtitles/search?language=de",
        None,
    )
    .await;
    let fetch = json!({ "language": "de", "result": 102 });
    let uri = "/api/v1/files/1/subtitles/fetch";
    let (status, body) = send(&f, "POST", uri, Some(fetch.clone())).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(
        crate::opensubtitles::used_up_until(&f.pool).await.unwrap(),
        Some(1_893_553_445)
    );
    let (status, _) = send(&f, "POST", uri, Some(fetch)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        calls(&f, "/download"),
        1,
        "not asked again until they're back"
    );
    let health = crate::admin_health::check(&f.pool).await.unwrap();
    let system = health.areas.iter().find(|a| a.area == "system").unwrap();
    let finding = system
        .findings
        .iter()
        .find(|x| x.code == "opensubtitles_used_up")
        .unwrap();
    assert!(
        finding.message.ends_with("resumes at 03:04 UTC."),
        "{}",
        finding.message
    );
}

#[tokio::test]
async fn finding_nothing_is_remembered_and_languages_are_checked() {
    let f = fixture(true).await;
    f.fake.nothing.store(true, Ordering::Relaxed);
    let search = "/api/v1/files/1/subtitles/search?language=pt-br";
    let (_, found) = send(&f, "POST", search, None).await;
    assert_eq!(found["results"], json!([]));
    let (_, again) = send(&f, "POST", search, None).await;
    assert_eq!(
        (again["results"].clone(), again["remembered"].clone()),
        (json!([]), json!(true))
    );
    assert_eq!(calls(&f, "/subtitles"), 1);
    for wrong in ["english", "e", "en-", "e1", "en-gb-x"] {
        let (status, _) = send(
            &f,
            "POST",
            &format!("/api/v1/files/1/subtitles/search?language={wrong}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{wrong}");
    }
}
