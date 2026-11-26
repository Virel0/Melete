use axum::body::Body;
use axum::extract::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::access::{Restriction, set_restriction};

async fn token(pool: &SqlitePool, name: &str, user: i64) -> String {
    crate::auth::issue_token(pool, user, crate::auth::TokenKind::Session, name)
        .await
        .unwrap()
        .1
}

async fn send(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
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
async fn friends_join_by_code_and_a_party_is_no_way_round_an_accounts_limits() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '/f'),
             (2, 'kids', 'movies', '/k');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Film (2020)', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation)
             VALUES (1, 1, 'Film (2020)/film.mkv', 1, 0, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let alice = crate::auth::testing::user(&pool, "alice", false).await;
    let bob = crate::auth::testing::user(&pool, "bob", false).await;
    let kid = crate::auth::testing::user(&pool, "kid", false).await;
    let restriction = Restriction {
        libraries: Some(vec![2]),
        max_age: None,
    };
    set_restriction(&pool, kid.id, Some(&restriction))
        .await
        .unwrap();
    let (a, b, k) = (
        token(&pool, "alice", alice.id).await,
        token(&pool, "bob", bob.id).await,
        token(&pool, "kid", kid.id).await,
    );
    let app = routes(pool.clone());

    let (status, started) = send(
        &app,
        &a,
        "POST",
        "/api/v1/watch-parties",
        json!({"file_id": 1, "position_ms": 0, "playing": true, "guest_max_bitrate": 8000000}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    assert!(started["server_time"].is_i64());
    let (id, code) = (
        started["party"]["id"].as_i64().unwrap(),
        started["party"]["code"].as_str().unwrap().to_owned(),
    );
    assert_eq!(started["party"]["movie_id"], 1);

    let (status, joined) = send(
        &app,
        &b,
        "POST",
        "/api/v1/watch-parties/join",
        json!({"code": code}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    let (status, refused) = send(
        &app,
        &k,
        "POST",
        "/api/v1/watch-parties/join",
        json!({"code": code}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, _) = send(
        &app,
        &b,
        "POST",
        "/api/v1/watch-parties/join",
        json!({"code": "ZZZZZZ"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Bob pauses; a change against an old version is a conflict; Bob can't end it.
    let uri = format!("/api/v1/watch-parties/{id}");
    let pause = json!({"version": 1, "file_id": 1, "position_ms": 5000, "playing": false});
    let (status, paused) = send(&app, &b, "PUT", &uri, pause.clone()).await;
    assert_eq!(
        (status, paused["party"]["version"].clone()),
        (StatusCode::OK, json!(2))
    );
    assert_eq!(
        send(&app, &a, "PUT", &uri, pause).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        send(&app, &b, "DELETE", &uri, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    // The kid, not in it, doesn't see it.
    assert_eq!(
        send(&app, &k, "GET", &uri, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, &a, "DELETE", &uri, Value::Null).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, none) = send(&app, &b, "GET", "/api/v1/watch-parties", Value::Null).await;
    assert_eq!(none, json!([]));
}
