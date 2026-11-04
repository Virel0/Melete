use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

async fn send(
    app: &Router,
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
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
    let user = auth::testing::user(pool, name, admin).await;
    auth::issue_token(pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

#[tokio::test]
async fn admins_list_and_switch_the_films_high_frame_rate_is_wanted_for() {
    let pool = crate::db::open_in_memory().await;
    let (admin, viewer) = (
        token(&pool, "admin", true).await,
        token(&pool, "viewer", false).await,
    );
    let app = routes(pool.clone());
    const LIST: &str = "/api/v1/high-frame-rate-films";

    // It starts with the films really made at a high frame rate.
    let (status, films) = send(&app, &admin, "GET", LIST, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(films, json!([49051, 57158, 122917, 293767, 453405]));

    let on = json!({ "allowed": true });
    let (status, _) = send(
        &app,
        &admin,
        "PUT",
        &format!("{LIST}/76600"),
        Some(on.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Twice is the same as once.
    send(
        &app,
        &admin,
        "PUT",
        &format!("{LIST}/76600"),
        Some(on.clone()),
    )
    .await;
    let off = json!({ "allowed": false });
    let (status, _) = send(&app, &admin, "PUT", &format!("{LIST}/49051"), Some(off)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, films) = send(&app, &admin, "GET", LIST, None).await;
    assert_eq!(films, json!([57158, 76600, 122917, 293767, 453405]));
    assert!(frame_rate::allowed(&pool, 76600).await.unwrap());
    assert!(!frame_rate::allowed(&pool, 49051).await.unwrap());

    // Admins only, and only a TMDB id.
    assert_eq!(
        send(&app, &viewer, "GET", LIST, None).await.0,
        StatusCode::FORBIDDEN
    );
    let refused = send(&app, &viewer, "PUT", &format!("{LIST}/1"), Some(on.clone())).await;
    assert_eq!(refused.0, StatusCode::FORBIDDEN);
    let bad = send(&app, &admin, "PUT", &format!("{LIST}/0"), Some(on)).await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    let typo = send(
        &app,
        &admin,
        "PUT",
        &format!("{LIST}/1"),
        Some(json!({ "allow": true })),
    )
    .await;
    assert_eq!(typo.0, StatusCode::UNPROCESSABLE_ENTITY);
}
