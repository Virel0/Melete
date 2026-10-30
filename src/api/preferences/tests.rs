use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;

async fn token(pool: &SqlitePool, name: &str) -> String {
    let password = auth::generate_password().unwrap();
    let user = auth::create_user(pool, name, password, false)
        .await
        .unwrap()
        .unwrap();
    auth::issue_token(pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

async fn send(app: &Router, token: &str, method: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri("/api/v1/me/playback")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
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
async fn the_next_episode_starts_by_itself_after_ten_seconds_until_a_user_says_otherwise() {
    let pool = db::open_in_memory().await;
    let app = routes(pool.clone());
    let viewer = token(&pool, "viewer").await;
    let other = token(&pool, "other").await;
    let defaults = json!({ "autoplay_next": true, "countdown_seconds": 10 });
    assert_eq!(
        send(&app, &viewer, "GET", None).await,
        (StatusCode::OK, defaults.clone())
    );

    let off = json!({ "autoplay_next": false });
    let (status, body) = send(&app, &viewer, "PUT", Some(off)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "autoplay_next": false, "countdown_seconds": 10 })
    );
    // A field left out keeps its value.
    let longer = json!({ "countdown_seconds": 25 });
    send(&app, &viewer, "PUT", Some(longer)).await;
    assert_eq!(
        send(&app, &viewer, "GET", None).await.1,
        json!({ "autoplay_next": false, "countdown_seconds": 25 })
    );
    // Each user's own.
    assert_eq!(send(&app, &other, "GET", None).await.1, defaults);
}

#[test]
fn the_answer_carries_exactly_the_documented_fields() {
    let spec: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
    let schema = &spec["components"]["schemas"]["PlaybackPreferences"]["properties"];
    let documented: Vec<&String> = schema.as_object().unwrap().keys().collect();
    let answer = serde_json::to_value(Playback::default()).unwrap();
    let given: Vec<&String> = answer.as_object().unwrap().keys().collect();
    assert_eq!(given, documented);
}

#[tokio::test]
async fn a_countdown_out_of_range_or_an_unknown_field_is_refused_and_nothing_changes() {
    let pool = db::open_in_memory().await;
    let app = routes(pool.clone());
    let viewer = token(&pool, "viewer").await;
    for wrong in [
        json!({ "countdown_seconds": 4 }),
        json!({ "countdown_seconds": 31 }),
        json!({ "autoplay_next": false, "countdown_seconds": 0 }),
    ] {
        let (status, body) = send(&app, &viewer, "PUT", Some(wrong)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("5 to 30"),
            "{body}"
        );
    }
    let (status, _) = send(&app, &viewer, "PUT", Some(json!({ "autoplay": false }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        send(&app, &viewer, "GET", None).await.1,
        json!({ "autoplay_next": true, "countdown_seconds": 10 })
    );
    // The bounds themselves are kept.
    for seconds in [5, 30] {
        let body = json!({ "countdown_seconds": seconds });
        let (status, body) = send(&app, &viewer, "PUT", Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["countdown_seconds"], json!(seconds));
    }
}
