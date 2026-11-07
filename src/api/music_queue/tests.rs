use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

/// The queue route and `GET /aoide/queue`, which the apps read it back from.
fn app(pool: &SqlitePool) -> Router {
    routes(pool.clone()).merge(super::super::aoide_library::routes(pool.clone()))
}

/// A user and a session of theirs named `device`: (user id, token id, token).
async fn session(pool: &SqlitePool, name: &str, device: &str) -> (i64, i64, String) {
    let user = match sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
    {
        Some(id) => id,
        None => crate::auth::testing::user(pool, name, false).await.id,
    };
    let (token_id, token) = auth::issue_token(pool, user, TokenKind::Session, device)
        .await
        .unwrap();
    (user, token_id, token)
}

async fn call(
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

#[tokio::test]
async fn a_sessions_queue_is_its_devices_row_for_the_users_other_devices() {
    let pool = crate::db::open_in_memory().await;
    let app = app(&pool);
    let (_, laptop_id, laptop) = session(&pool, "alice", "Firefox on Linux").await;
    let (_, _, phone) = session(&pool, "alice", "Phone").await;

    let body = json!({ "track_ids": [7, 8, 9], "position": 1, "elapsed_ms": 12_000 });
    let (status, _) = call(&app, &laptop, "PUT", "/api/v1/music/queue", Some(body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Another of the user's devices sees it, named and shaped as an app's row.
    let (status, queues) = call(&app, &phone, "GET", "/aoide/queue", None).await;
    assert_eq!(status, StatusCode::OK);
    let queues = queues.as_array().unwrap();
    assert_eq!(queues.len(), 1);
    let device = format!("token-{laptop_id}");
    assert_eq!(queues[0]["deviceId"], device.as_str());
    assert_eq!(queues[0]["isCurrentDevice"], false);
    let payload = &queues[0]["payload"];
    assert_eq!(payload["deviceId"], device.as_str());
    assert_eq!(payload["originDevice"], device.as_str());
    assert_eq!(payload["deviceName"], "Firefox on Linux");
    assert_eq!(
        payload["trackIds"], r#"["7","8","9"]"#,
        "JSON text, as the apps write it"
    );
    assert_eq!(payload["position"], 1);
    assert_eq!(payload["elapsedMs"], 12_000);
    assert!(payload["updatedAt"].as_i64().unwrap() > 0);
    // The session that wrote it is told it's its own.
    let (_, mine) = call(&app, &laptop, "GET", "/aoide/queue", None).await;
    assert_eq!(mine[0]["isCurrentDevice"], true);

    // Publishing again replaces the row: one per device.
    let body = json!({ "track_ids": [9], "position": 0, "elapsed_ms": 0 });
    call(&app, &laptop, "PUT", "/api/v1/music/queue", Some(body)).await;
    let (_, queues) = call(&app, &phone, "GET", "/aoide/queue", None).await;
    assert_eq!(queues.as_array().unwrap().len(), 1);
    assert_eq!(queues[0]["payload"]["trackIds"], r#"["9"]"#);

    // An empty queue is a row too, which the apps don't offer.
    let body = json!({ "track_ids": [] });
    let (status, _) = call(&app, &laptop, "PUT", "/api/v1/music/queue", Some(body)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, queues) = call(&app, &phone, "GET", "/aoide/queue", None).await;
    assert_eq!(queues[0]["payload"]["trackIds"], "[]");
}

#[tokio::test]
async fn one_user_can_neither_read_nor_write_anothers_queue() {
    let pool = crate::db::open_in_memory().await;
    let app = app(&pool);
    let (_, _, alice) = session(&pool, "alice", "Laptop").await;
    let (_, _, alices_phone) = session(&pool, "alice", "Phone").await;
    let (_, bob_id, bob) = session(&pool, "bob", "Laptop").await;

    let body = json!({ "track_ids": [7], "position": 0, "elapsed_ms": 1_000 });
    call(&app, &alice, "PUT", "/api/v1/music/queue", Some(body)).await;
    // Bob doesn't see Alice's queue.
    let (_, bobs) = call(&app, &bob, "GET", "/aoide/queue", None).await;
    assert_eq!(bobs, json!([]));
    // Bob's publishing writes his own row, in his own log: Alice's is untouched.
    let body = json!({ "track_ids": [8, 9], "position": 1, "elapsed_ms": 5_000 });
    call(&app, &bob, "PUT", "/api/v1/music/queue", Some(body)).await;
    let (_, alices) = call(&app, &alices_phone, "GET", "/aoide/queue", None).await;
    let alices = alices.as_array().unwrap();
    assert_eq!(alices.len(), 1);
    assert_eq!(alices[0]["payload"]["trackIds"], r#"["7"]"#);
    let (_, bobs) = call(&app, &bob, "GET", "/aoide/queue", None).await;
    assert_eq!(bobs.as_array().unwrap().len(), 1);
    assert_eq!(bobs[0]["deviceId"], format!("token-{bob_id}").as_str());
    assert_eq!(bobs[0]["payload"]["trackIds"], r#"["8","9"]"#);
}

#[tokio::test]
async fn a_queue_out_of_bounds_or_from_a_limited_user_is_refused() {
    let pool = crate::db::open_in_memory().await;
    let app = app(&pool);
    let (_, _, token) = session(&pool, "alice", "Laptop").await;
    let too_many: Vec<i64> = (1..=501).collect();
    for (body, want) in [
        (json!({ "track_ids": too_many }), StatusCode::BAD_REQUEST),
        (
            json!({ "track_ids": [1, 2], "position": 2 }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "track_ids": [1], "position": -1 }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "track_ids": [1], "elapsed_ms": -5 }),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({ "track_ids": [1], "device_id": "phone" }),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, _) = call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/queue",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, want, "{body}");
    }
    let (_, queues) = call(&app, &token, "GET", "/aoide/queue", None).await;
    assert_eq!(queues, json!([]), "nothing refused was kept");

    // A user who may see only some music libraries can't publish one, as they can't read them.
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             SELECT id, 0, NULL, 0 FROM users WHERE username = 'alice';",
    )
    .execute(&pool)
    .await
    .unwrap();
    let body = json!({ "track_ids": [1] });
    let (status, _) = call(&app, &token, "PUT", "/api/v1/music/queue", Some(body)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
