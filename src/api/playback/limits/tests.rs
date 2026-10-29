use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::http::{Request as HttpRequest, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::TokenKind;
use crate::library::{LibraryConfig, LibraryKind, sync_config};
use crate::ways_in::{WaysIn, guard, parse_nets, parse_ways};

#[test]
fn rates_are_said_in_words() {
    assert_eq!(rate_words(8_000_000), "8 Mbit/s");
    assert_eq!(rate_words(2_500_000), "2.5 Mbit/s");
    assert_eq!(rate_words(800_000), "800 kbit/s");
}

#[test]
fn a_files_bitrate_is_its_containers_else_its_size_over_its_length() {
    let mut media: probe::MediaInfo = serde_json::from_value(json!({
        "container": "matroska,webm", "duration_seconds": 100.0, "bit_rate": 5_000_000,
        "video": [], "audio": [], "subtitles": []
    }))
    .unwrap();
    assert_eq!(overall_bitrate(1, Some(&media)), Some(5_000_000));
    media.bit_rate = None;
    assert_eq!(overall_bitrate(50_000_000, Some(&media)), Some(4_000_000));
    media.duration_seconds = None;
    assert_eq!(overall_bitrate(50_000_000, Some(&media)), None);
    assert_eq!(overall_bitrate(50_000_000, None), None);
}

struct Fixture {
    app: Router,
    pool: SqlitePool,
    token: String,
    /// The clip's overall bitrate.
    bitrate: i64,
    _dir: tempfile::TempDir,
}

/// File 1, a 20-second clip, behind three ways in: home with no limit, "slow" held to
/// 100 kbit/s (less than the clip takes) and "fast" to 8 Mbit/s (more).
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let clip = probe::testing::make_long_clip(dir.path(), 20);
    let info = probe::probe(std::path::Path::new("ffprobe"), &clip)
        .await
        .unwrap();
    let bitrate = info.bit_rate.unwrap();
    assert!(
        bitrate > 100_000,
        "the clip takes more than the slow way's limit"
    );
    std::fs::rename(&clip, library.join("clip.mkv")).unwrap();
    let size = std::fs::metadata(library.join("clip.mkv")).unwrap().len() as i64;
    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: library,
    };
    sync_config(&pool, &[config]).await.unwrap();
    sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'clip.mkv', 'Clip', 1)")
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
    let user = auth::create_user(&pool, "alice", "alice-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap();
    let transcoder = crate::transcode::Transcoder::new(
        "ffmpeg".into(),
        crate::transcode::Hardware::Software,
        dir.path().join("transcode"),
        4,
    );
    let ways = WaysIn {
        ways: Some(
            parse_ways(
                "home 192.168.0.0/24 all; slow 10.20.0.1 all max=100k; fast 10.10.0.1 all max=8M",
            )
            .unwrap(),
        ),
        proxies: parse_nets("").unwrap(),
    };
    let app = routes(pool.clone(), "ffprobe".into())
        .merge(crate::api::transcode::routes(
            pool.clone(),
            "ffprobe".into(),
            transcoder,
            dir.path().join("subtitles"),
        ))
        .layer(axum::middleware::from_fn_with_state(Arc::new(ways), guard));
    Fixture {
        app,
        pool,
        token,
        bitrate,
        _dir: dir,
    }
}

/// Sends a request from `from`, signed in, and returns the status and JSON body.
async fn send(
    f: &Fixture,
    method: &str,
    uri: &str,
    from: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", f.token));
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(from.parse().unwrap(), 50_000)));
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_limited_way_says_its_limit_and_refuses_a_file_over_it() {
    let f = fixture().await;
    let (_, info) = send(&f, "GET", "/api/v1/files/1", "192.168.0.5", None).await;
    assert_eq!(info["max_bitrate"], Value::Null, "home has no limit");
    let (_, info) = send(&f, "GET", "/api/v1/files/1", "10.20.0.1", None).await;
    assert_eq!(info["max_bitrate"], 100_000);

    let (status, play) = send(&f, "POST", "/api/v1/files/1/play", "10.20.0.1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(play["max_bitrate"], 100_000);
    let link = play["stream_url"].as_str().unwrap().to_owned();
    let (status, refused) = send(&f, "GET", &link, "10.20.0.1", None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let error = refused["error"].as_str().unwrap();
    assert!(
        error.contains("over the 100 kbit/s limit of the way you came in; ask for a conversion"),
        "{error}"
    );
    let (status, _) = send(&f, "GET", &link, "10.10.0.1", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the clip is within the fast way's limit"
    );
    let (status, _) = send(&f, "GET", &link, "192.168.0.5", None).await;
    assert_eq!(status, StatusCode::OK, "the same link at home");
}

/// The settings a transcode's link was made with, as stored.
async fn stored_settings(f: &Fixture) -> Value {
    let settings: String = sqlx::query_scalar(
        "SELECT transcode FROM play_sessions WHERE transcode IS NOT NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    serde_json::from_str(&settings).unwrap()
}

#[tokio::test]
async fn a_limited_way_holds_conversions_to_its_limit() {
    let f = fixture().await;
    let transcode = "/api/v1/files/1/transcode";
    let (status, answer) = send(&f, "POST", transcode, "10.10.0.1", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["max_bitrate"], 8_000_000);
    assert_eq!(
        stored_settings(&f).await["max_bitrate"],
        8_000_000 - 384_000,
        "the limit less the audio's share"
    );
    let asked = json!({ "max_bitrate": 2_000_000 });
    send(&f, "POST", transcode, "10.10.0.1", Some(asked.clone())).await;
    assert_eq!(
        stored_settings(&f).await["max_bitrate"],
        2_000_000,
        "less was asked"
    );
    let (_, answer) = send(&f, "POST", transcode, "192.168.0.5", Some(asked)).await;
    assert_eq!(answer["max_bitrate"], Value::Null);
    send(&f, "POST", transcode, "192.168.0.5", Some(json!({}))).await;
    assert_eq!(
        stored_settings(&f).await["max_bitrate"],
        Value::Null,
        "home: as asked"
    );

    send(&f, "POST", transcode, "10.20.0.1", Some(json!({}))).await;
    assert_eq!(
        stored_settings(&f).await["max_bitrate"],
        100_000,
        "never below the least"
    );
    // Copied video keeps the file's bitrate: only through a way it fits.
    let copy = json!({ "video_codec": "copy" });
    let (status, refused) = send(&f, "POST", transcode, "10.20.0.1", Some(copy.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, _) = send(&f, "POST", transcode, "10.10.0.1", Some(copy)).await;
    assert_eq!(status, StatusCode::OK, "{} bit/s is within 8M", f.bitrate);
}

#[tokio::test]
async fn an_extras_stream_link_is_held_to_the_limit_too() {
    let f = fixture().await;
    let extra = 1_099_511_627_776_i64;
    let info = serde_json::to_string(&json!({
        "container": "matroska,webm", "duration_seconds": 20.0, "bit_rate": f.bitrate,
        "video": [], "audio": [], "subtitles": []
    }))
    .unwrap();
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation, probed_size, probed_modified_at,
             probe_info) VALUES (?, 1, 'clip.mkv', 1, 1, 1, 'trailer', 'Trailer', 1, 1, 1, 1, ?)",
    )
    .bind(extra)
    .bind(info)
    .execute(&f.pool)
    .await
    .unwrap();
    let (status, play) = send(
        &f,
        "POST",
        &format!("/api/v1/files/{extra}/play"),
        "10.20.0.1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{play}");
    assert_eq!(play["max_bitrate"], 100_000);
    let link = play["stream_url"].as_str().unwrap().to_owned();
    assert_eq!(
        send(&f, "GET", &link, "10.20.0.1", None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&f, "GET", &link, "10.10.0.1", None).await.0,
        StatusCode::OK
    );
}
