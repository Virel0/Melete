use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};

struct Fixture {
    pool: SqlitePool,
    app: Router,
    _dir: tempfile::TempDir,
}

/// An episode's probe: English audio (the default) and Japanese, and English signs (forced).
const EPISODE: &str = r#"{"container": "matroska,webm", "duration_seconds": 1400.0, "bit_rate": null,
    "video": [],
    "audio": [
        {"index": 1, "codec": "aac", "profile": null, "channels": 2, "channel_layout": null,
         "language": "eng", "title": null, "default": true},
        {"index": 2, "codec": "aac", "profile": null, "channels": 2, "channel_layout": null,
         "language": "jpn", "title": null, "default": false}],
    "subtitles": [
        {"index": 3, "codec": "subrip", "language": "eng", "title": "Signs", "default": true,
         "forced": true, "text": true}]}"#;

/// Files 1 and 2: episodes of series 1, the first with full English subtitles beside it.
/// File 3: a movie.
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let shows = dir.path().join("shows");
    let movies = dir.path().join("movies");
    std::fs::create_dir_all(shows.join("Show")).unwrap();
    std::fs::create_dir_all(movies.join("Movie")).unwrap();
    for name in ["Show/S01E01.mkv", "Show/S01E02.mkv", "Show/S01E01.en.srt"] {
        std::fs::write(shows.join(name), b"x").unwrap();
    }
    std::fs::write(movies.join("Movie/movie.mkv"), b"x").unwrap();
    let pool = db::open_in_memory().await;
    let libraries = [
        LibraryConfig {
            name: "shows".into(),
            kind: LibraryKind::Shows,
            path: shows,
        },
        LibraryConfig {
            name: "movies".into(),
            kind: LibraryKind::Movies,
            path: movies,
        },
    ];
    sync_config(&pool, &libraries).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Show', 'Show', 1);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 2, 'Movie', 'Movie', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id, season,
             episode, movie_id, first_seen_generation, seen_generation) VALUES
             (1, 1, 'Show/S01E01.mkv', 1, 1, 1, 1, 1, NULL, 1, 1),
             (2, 1, 'Show/S01E02.mkv', 1, 1, 1, 1, 2, NULL, 1, 1),
             (3, 2, 'Movie/movie.mkv', 1, 1, NULL, NULL, NULL, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    for file in 1..=3 {
        sqlx::query(
            "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
             VALUES (?, 1, 1, 1, ?)",
        )
        .bind(file)
        .bind(EPISODE)
        .execute(&pool)
        .await
        .unwrap();
    }
    let app = routes(pool.clone()).merge(super::super::playback::routes(
        pool.clone(),
        "ffprobe".into(),
    ));
    Fixture {
        pool,
        app,
        _dir: dir,
    }
}

async fn token(f: &Fixture, name: &str) -> String {
    let user = auth::create_user(&f.pool, name, "user-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    auth::issue_token(&f.pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

async fn send(
    f: &Fixture,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
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

async fn tracks(f: &Fixture, token: &str, file: i64) -> Value {
    let (status, body) = send(
        f,
        token,
        "POST",
        &format!("/api/v1/files/{file}/play"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["tracks"].clone()
}

fn picked(audio: i64, audio_reason: &str, subtitle: Value, subtitle_reason: &str) -> Value {
    json!({ "audio": audio, "audio_reason": audio_reason, "subtitle": subtitle,
            "subtitle_reason": subtitle_reason })
}

#[tokio::test]
async fn playing_picks_tracks_from_the_users_languages_and_their_series_choice() {
    let f = fixture().await;
    let viewer = token(&f, "viewer").await;
    assert_eq!(
        tracks(&f, &viewer, 1).await,
        picked(1, "file_default", json!("s3"), "forced")
    );

    let languages = json!({ "audio_languages": ["jpn"], "subtitle_languages": ["en"],
                            "subtitle_mode": "foreign" });
    let (status, body) = send(&f, &viewer, "PUT", "/api/v1/me/languages", Some(languages)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["audio_languages"], json!(["ja"]));
    // Japanese isn't read: the full English subtitles beside the file, not the signs.
    let picks = tracks(&f, &viewer, 1).await;
    assert_eq!(picks["audio"], 2);
    assert_eq!(picks["audio_reason"], "preferred_language");
    assert!(
        picks["subtitle"].as_str().unwrap().starts_with('x'),
        "{picks}"
    );
    assert_eq!(picks["subtitle_reason"], "foreign_audio");

    // The dub with subtitles off, chosen in episode 1, carries to episode 2.
    let choice = json!({ "audio_language": "eng", "subtitle": null });
    let (status, _) = send(
        &f,
        &viewer,
        "PUT",
        "/api/v1/files/1/track-choice",
        Some(choice),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        tracks(&f, &viewer, 2).await,
        picked(1, "series_choice", Value::Null, "series_choice")
    );
    // Signs chosen by hand.
    let signs = json!({ "audio_language": "jpn",
                        "subtitle": { "language": "eng", "forced": true, "hearing_impaired": false } });
    send(
        &f,
        &viewer,
        "PUT",
        "/api/v1/files/2/track-choice",
        Some(signs),
    )
    .await;
    assert_eq!(
        tracks(&f, &viewer, 2).await,
        picked(2, "series_choice", json!("s3"), "series_choice")
    );
    // A movie doesn't take a series' choice, and nobody else's choices are anyone else's.
    assert_eq!(
        tracks(&f, &viewer, 3).await["audio_reason"],
        "preferred_language"
    );
    let other = token(&f, "other").await;
    assert_eq!(
        tracks(&f, &other, 2).await,
        picked(1, "file_default", json!("s3"), "forced")
    );
}

#[tokio::test]
async fn languages_are_checked_and_kept_in_order_once_each() {
    let f = fixture().await;
    let viewer = token(&f, "viewer").await;
    let (status, body) = send(&f, &viewer, "GET", "/api/v1/me/languages", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "audio_languages": [], "subtitle_languages": [], "subtitle_mode": "foreign" })
    );
    let put = |audio: Value, mode: &str| {
        json!({ "audio_languages": audio, "subtitle_languages": ["pt_br", "por"],
                "subtitle_mode": mode })
    };
    let (status, body) = send(
        &f,
        &viewer,
        "PUT",
        "/api/v1/me/languages",
        Some(put(json!(["jpn", "ja", "fre"]), "always")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let kept = json!({ "audio_languages": ["ja", "fr"], "subtitle_languages": ["pt-BR", "pt"],
                       "subtitle_mode": "always" });
    assert_eq!(body, kept);
    assert_eq!(
        send(&f, &viewer, "GET", "/api/v1/me/languages", None)
            .await
            .1,
        kept
    );

    let many: Vec<String> = [
        "en", "fr", "de", "es", "it", "ja", "ko", "zh", "ru", "pl", "sv",
    ]
    .map(String::from)
    .to_vec();
    for wrong in [
        put(json!(["english"]), "always"),
        put(json!(["und"]), "always"),
        put(json!(["en"]), "sometimes"),
        put(json!(many), "always"),
    ] {
        let (status, _) = send(&f, &viewer, "PUT", "/api/v1/me/languages", Some(wrong)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        send(&f, &viewer, "GET", "/api/v1/me/languages", None)
            .await
            .1,
        kept
    );
}

#[tokio::test]
async fn choices_are_kept_for_episodes_only() {
    let f = fixture().await;
    let viewer = token(&f, "viewer").await;
    let choice = json!({ "audio_language": null, "subtitle": null });
    let at = |file: i64| format!("/api/v1/files/{file}/track-choice");
    let (status, _) = send(&f, &viewer, "PUT", &at(3), Some(choice.clone())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(&f, &viewer, "PUT", &at(99), Some(choice)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let wrong = json!({ "audio_language": "klingon", "subtitle": null });
    let (status, _) = send(&f, &viewer, "PUT", &at(1), Some(wrong)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // An untagged track chosen: nothing is remembered about it, and the user's languages decide.
    let untagged = json!({ "audio_language": null,
                           "subtitle": { "language": null, "forced": false } });
    let (status, _) = send(&f, &viewer, "PUT", &at(1), Some(untagged)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        tracks(&f, &viewer, 2).await,
        picked(1, "file_default", json!("s3"), "forced")
    );
}
