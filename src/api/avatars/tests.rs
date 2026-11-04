use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::avatars::tests::{PNG, gallery};

struct Fixture {
    app: Router,
    pool: SqlitePool,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let pool = crate::db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    let app = routes_with(pool.clone(), gallery(dir.path()));
    Fixture {
        app,
        pool,
        _dir: dir,
    }
}

/// A session for `name`, made the first time, and their id.
async fn session(f: &Fixture, name: &str) -> (String, i64) {
    let user = match auth::find_user(&f.pool, name).await.unwrap() {
        Some(user) => user,
        None => crate::auth::testing::user(&f.pool, name, false).await,
    };
    let token = auth::issue_token(&f.pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1;
    (token, user.id)
}

/// A request's status, content type and body.
async fn send(
    f: &Fixture,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<(&str, Vec<u8>)>,
) -> (StatusCode, String, Vec<u8>) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some((mime, bytes)) => {
            request = request.header(header::CONTENT_TYPE, mime);
            Body::from(bytes)
        }
        None => Body::empty(),
    };
    let response = f
        .app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, mime, bytes)
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

#[tokio::test]
async fn an_upload_is_your_picture_and_everyone_sees_it() {
    let f = fixture().await;
    let (alice, alice_id) = session(&f, "alice").await;
    let (bob, _) = session(&f, "bob").await;
    let (status, _, body) = send(
        &f,
        &alice,
        "PUT",
        "/api/v1/me/avatar",
        Some(("image/png", PNG.to_vec())),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let url = json_of(&body)["avatar_url"].as_str().unwrap().to_owned();
    let hash = crate::aoide::library::sha256_hex(PNG);
    assert_eq!(
        url,
        format!("/api/v1/users/{alice_id}/avatar?v={}", &hash[..12])
    );
    // Bob sees Alice's picture.
    let (status, mime, bytes) = send(&f, &bob, "GET", &url, None).await;
    assert_eq!(
        (status, mime.as_str(), bytes.as_slice()),
        (StatusCode::OK, "image/png", PNG)
    );
    // She takes it away: no picture for anyone.
    let (status, _, body) = send(&f, &alice, "DELETE", "/api/v1/me/avatar", None).await;
    assert_eq!(
        (status, &json_of(&body)["avatar_url"]),
        (StatusCode::OK, &Value::Null)
    );
    let (status, _, _) = send(&f, &bob, "GET", &url, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_upload_is_refused_before_anything_is_kept() {
    let f = fixture().await;
    let (alice, alice_id) = session(&f, "alice").await;
    for (mime, bytes, expected) in [
        ("image/gif", PNG.to_vec(), StatusCode::BAD_REQUEST),
        ("image/png", Vec::new(), StatusCode::BAD_REQUEST),
        ("image/jpeg", PNG.to_vec(), StatusCode::BAD_REQUEST),
        ("image/webp", b"<html>".to_vec(), StatusCode::BAD_REQUEST),
        (
            "image/png",
            [PNG, &vec![0; MAX_IMAGE_BYTES]].concat(),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let (status, _, body) =
            send(&f, &alice, "PUT", "/api/v1/me/avatar", Some((mime, bytes))).await;
        assert_eq!(
            status,
            expected,
            "{mime}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    assert!(avatars::get(&f.pool, alice_id).await.unwrap().is_none());
}

#[tokio::test]
async fn a_gallery_picture_is_kept_as_your_own() {
    let f = fixture().await;
    let (alice, alice_id) = session(&f, "alice").await;
    let (status, _, body) = send(&f, &alice, "GET", "/api/v1/avatars", None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<String> = json_of(&body)["categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["official", "gallery/Netflix"]);
    let (status, _, body) = send(
        &f,
        &alice,
        "GET",
        "/api/v1/avatars/pictures?category=gallery%2FNetflix&limit=1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page = json_of(&body);
    assert_eq!(page["total"], 2);
    let first = &page["pictures"][0];
    assert_eq!(first["id"], "gallery/Netflix/a.png");
    let (status, mime, bytes) = send(&f, &alice, "GET", first["url"].as_str().unwrap(), None).await;
    assert_eq!(
        (status, mime.as_str(), bytes.as_slice()),
        (StatusCode::OK, "image/png", PNG)
    );
    let pick = serde_json::to_vec(&json!({ "id": "gallery/Netflix/a.png" })).unwrap();
    let (status, _, body) = send(
        &f,
        &alice,
        "PUT",
        "/api/v1/me/avatar/gallery",
        Some(("application/json", pick)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert!(json_of(&body)["avatar_url"].is_string());
    let kept = avatars::get(&f.pool, alice_id).await.unwrap().unwrap();
    assert_eq!(
        (kept.bytes.as_slice(), kept.mime.as_str()),
        (PNG, "image/png")
    );
}

#[tokio::test]
async fn nothing_outside_the_gallery_is_served_or_picked() {
    let f = fixture().await;
    let (alice, alice_id) = session(&f, "alice").await;
    for id in [
        "gallery/../secret.png",
        "gallery/Netflix/../../secret.png",
        "official/README.md",
    ] {
        let query = reqwest::Url::parse_with_params("http://x/", [("id", id)]).unwrap();
        let uri = format!("/api/v1/avatars/picture?{}", query.query().unwrap());
        let (status, _, _) = send(&f, &alice, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
        let pick = serde_json::to_vec(&json!({ "id": id })).unwrap();
        let (status, _, _) = send(
            &f,
            &alice,
            "PUT",
            "/api/v1/me/avatar/gallery",
            Some(("application/json", pick)),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
    }
    let (status, _, _) = send(
        &f,
        &alice,
        "GET",
        "/api/v1/avatars/pictures?category=gallery%2F..",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(avatars::get(&f.pool, alice_id).await.unwrap().is_none());
}
