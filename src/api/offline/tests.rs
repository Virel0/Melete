use axum::http::Request as HttpRequest;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::TokenKind;
use crate::offline::tests::{library, tools, turn_on};

struct Fixture {
    app: Router,
    pool: SqlitePool,
    tokens: [String; 2],
    tools: offline::Tools,
    dir: tempfile::TempDir,
}

/// The library of the offline tests, with alice (user 1) and bob signed in.
async fn fixture(on: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = library(dir.path()).await;
    if on {
        turn_on(&pool).await;
    }
    let bob = crate::auth::create_user(&pool, "bob", "bob-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let mut tokens = Vec::new();
    for user in [1, bob.id] {
        let (_, token) = crate::auth::issue_token(&pool, user, TokenKind::Session, "t")
            .await
            .unwrap();
        tokens.push(token);
    }
    let tools = tools(dir.path(), "ffmpeg".into(), Hardware::Software);
    Fixture {
        app: routes(pool.clone(), "ffprobe".into(), tools.root.clone()),
        pool,
        tokens: [tokens[0].clone(), tokens[1].clone()],
        tools,
        dir,
    }
}

/// A request as `who` (0 for alice, 1 for bob, None for nobody); status, headers' range and
/// body.
async fn send(
    f: &Fixture,
    method: &str,
    uri: &str,
    who: Option<usize>,
    body: Option<Value>,
    range: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut request = HttpRequest::builder().method(method).uri(uri);
    if let Some(who) = who {
        request = request.header(header::AUTHORIZATION, format!("Bearer {}", f.tokens[who]));
    }
    if let Some(range) = range {
        request = request.header(header::RANGE, range);
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (
        status,
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
}

async fn json_of(
    f: &Fixture,
    method: &str,
    uri: &str,
    who: usize,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, bytes) = send(f, method, uri, Some(who), body, None).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn nothing_is_made_while_its_off() {
    let f = fixture(false).await;
    let (status, choices) = json_of(&f, "GET", "/api/v1/files/1/offline", 0, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(choices["on"], false);
    let (status, _) = json_of(&f, "POST", "/api/v1/files/1/offline", 0, Some(json!({}))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(offline::held(&f.pool, 1).await.unwrap(), 0);
}

#[tokio::test]
async fn a_copy_is_asked_for_followed_downloaded_and_deleted() {
    let f = fixture(true).await;
    let (_, choices) = json_of(&f, "GET", "/api/v1/files/1/offline", 0, None).await;
    assert_eq!(choices["choices"][0]["max_height"], 480, "{choices}");
    assert!(choices["choices"][0]["estimated_bytes"].as_i64().unwrap() > 0);

    let track = format!(
        "x{}",
        crate::subtitles::sidecars(&f.dir.path().join("library/clip.mkv")).unwrap()[0].id
    );
    let asked = json!({ "max_height": 180, "subtitles": [track] });
    let (status, copy) = json_of(&f, "POST", "/api/v1/files/1/offline", 0, Some(asked)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{copy}");
    assert_eq!(copy["status"], "queued");
    let id = copy["id"].as_i64().unwrap();
    let one = format!("/api/v1/offline/{id}");
    assert_eq!(
        json_of(&f, "GET", &one, 1, None).await.0,
        StatusCode::NOT_FOUND,
        "bob's not"
    );
    let (status, _) = json_of(&f, "POST", &format!("{one}/link"), 0, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "not ready yet");

    offline::make_next(&f.pool, &f.tools).await.unwrap();
    let (_, mine) = json_of(&f, "GET", "/api/v1/offline", 0, None).await;
    assert_eq!(mine[0]["status"], "ready", "{mine}");
    let (_, link) = json_of(&f, "POST", &format!("{one}/link"), 0, None).await;
    let url = link["download_url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/api/v1/offline/download/mlo_"), "{url}");
    let (status, part) = send(&f, "GET", &url, None, None, Some("bytes=0-99")).await;
    assert_eq!(
        (status, part.len()),
        (StatusCode::PARTIAL_CONTENT, 100),
        "resumable"
    );
    let vtt = link["subtitles"][0]["url"].as_str().unwrap().to_owned();
    let (status, body) = send(&f, "GET", &vtt, None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().contains("Kept beside it"));
    let (_, copy) = json_of(&f, "GET", &one, 0, None).await;
    assert!(copy["delete_at"].as_i64().unwrap() >= crate::db::unix_now() + offline::KEPT - 5);

    assert_eq!(
        json_of(&f, "DELETE", &one, 1, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        json_of(&f, "DELETE", &one, 0, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(!offline::video_path(&f.tools.root, id).exists());
    assert_eq!(
        send(&f, "GET", &url, None, None, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn asking_is_checked() {
    let f = fixture(true).await;
    let uri = "/api/v1/files/1/offline";
    for (body, status) in [
        (json!({ "burn_subtitle": "s2" }), StatusCode::BAD_REQUEST),
        (json!({ "subtitles": ["nope"] }), StatusCode::BAD_REQUEST),
        (json!({ "max_height": 50 }), StatusCode::BAD_REQUEST),
        (
            json!({ "audio_stream": 9 }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        assert_eq!(
            json_of(&f, "POST", uri, 0, Some(body.clone())).await.0,
            status,
            "{body}"
        );
    }
    for _ in 0..offline::PER_USER {
        offline::create(&f.pool, 1, 1, "{}", &[], 1).await.unwrap();
    }
    let (status, said) = json_of(&f, "POST", uri, 0, Some(json!({}))).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{said}");
    assert_eq!(
        json_of(&f, "POST", uri, 1, Some(json!({}))).await.0,
        StatusCode::ACCEPTED,
        "bob has none"
    );
    assert_eq!(
        send(&f, "GET", "/api/v1/offline", None, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &f,
            "GET",
            "/api/v1/offline/download/mls_x",
            None,
            None,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "a stream link isn't one"
    );
}
