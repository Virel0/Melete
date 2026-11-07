use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use serde_json::value::RawValue;
use tower::ServiceExt;

use super::*;
use crate::aoide::sync;
use crate::auth::{self, TokenKind};

/// Two tracks, Jazz (1) and Rock (2), and the playlist routes.
async fn fixture() -> (SqlitePool, Router) {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Mixed', 'Various', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, genres, duration_seconds)
             VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'So What', '["Jazz"]', 545.0),
                    (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Paranoid Android', '["Rock"]', 383.0);"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = routes(pool.clone());
    (pool, app)
}

async fn user(pool: &SqlitePool, name: &str) -> (i64, String) {
    let user = crate::auth::testing::user(pool, name, false).await;
    let token = auth::issue_token(pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1;
    (user.id, token)
}

/// Pushes one op as a device of `user` would.
async fn push(pool: &SqlitePool, user: i64, entity: &str, payload: Value) {
    let op = json!({"opId": format!("{entity}-{}", payload["id"].as_str().unwrap()),
        "entity": entity, "entityId": payload["id"], "operation": "upsert",
        "payload": payload, "createdAt": 1});
    let op = sync::validate(&RawValue::from_string(op.to_string()).unwrap()).unwrap();
    let pushed = sync::push(pool, user, "phone", vec![op], 1).await.unwrap();
    assert!(pushed.denied.is_empty());
}

/// A smart playlist of `user`'s with rules `rules` (stored as text, as the apps store them).
async fn smart(pool: &SqlitePool, user: i64, id: &str, rules: Option<&str>) {
    push(
        pool,
        user,
        "playlists",
        json!({"id": id, "name": id, "isSmart": rules.is_some(), "smartRules": rules,
            "sortIndex": "a0", "updatedAt": 1, "deleted": false, "originDevice": "phone"}),
    )
    .await;
}

async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn titles(body: &Value) -> Vec<&str> {
    body["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn a_smart_playlists_songs_are_worked_out_for_whoever_listens() {
    let (pool, app) = fixture().await;
    let (alice, alice_token) = user(&pool, "alice").await;
    let (bob, bob_token) = user(&pool, "bob").await;
    let (_, stranger) = user(&pool, "carol").await;
    smart(
        &pool,
        alice,
        "JAZZ",
        Some(r#"{"match":"all","rules":[{"field":"genre","op":"is","value":"jazz"}]}"#),
    )
    .await;
    smart(
        &pool,
        alice,
        "LOVED",
        Some(r#"{"match":"all","rules":[{"field":"liked","op":"=","value":true}],"sort":{"field":"title","dir":"desc"}}"#),
    )
    .await;
    for id in ["JAZZ", "LOVED"] {
        crate::aoide::library::share(&pool, id, bob, false, 1)
            .await
            .unwrap();
    }
    push(
        &pool,
        alice,
        "likes",
        json!({"id": "l1", "jellyfinId": "1", "contentKey": "", "liked": true,
            "updatedAt": 1, "deleted": false, "originDevice": "phone"}),
    )
    .await;

    let (status, body) = get(&app, &alice_token, "/api/v1/playlists/JAZZ/songs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(titles(&body), ["So What"]);
    assert_eq!(body["id"], "JAZZ");
    assert_eq!(
        body["rules"],
        json!({"match": "all", "rules": [{"field": "genre", "op": "is", "value": "jazz"}]})
    );
    // Shared to listen, the same rules, worked out from the listener's own likes.
    assert_eq!(
        titles(
            &get(&app, &bob_token, "/api/v1/playlists/JAZZ/songs")
                .await
                .1
        ),
        ["So What"]
    );
    let (_, loved) = get(&app, &alice_token, "/api/v1/playlists/LOVED/songs").await;
    assert_eq!(titles(&loved), ["So What"]);
    assert_eq!(
        loved["rules"]["rules"][0]["op"], "is",
        "a symbol comes back by name"
    );
    let (_, theirs) = get(&app, &bob_token, "/api/v1/playlists/LOVED/songs").await;
    assert!(titles(&theirs).is_empty());
    // A song the listener marked Not interested is never one.
    push(
        &pool,
        bob,
        "track_flags",
        json!({"id": "f1", "jellyfinId": "1", "contentKey": "", "notInterested": true,
            "dontCount": false, "updatedAt": 1, "deleted": false, "originDevice": "phone"}),
    )
    .await;
    assert!(
        titles(
            &get(&app, &bob_token, "/api/v1/playlists/JAZZ/songs")
                .await
                .1
        )
        .is_empty()
    );
    assert_eq!(
        titles(
            &get(&app, &alice_token, "/api/v1/playlists/JAZZ/songs")
                .await
                .1
        ),
        ["So What"]
    );
    // Not shared with her: no such playlist.
    assert_eq!(
        get(&app, &stranger, "/api/v1/playlists/JAZZ/songs").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn what_cant_be_worked_out_is_refused_saying_why() {
    let (pool, app) = fixture().await;
    let (alice, token) = user(&pool, "alice").await;
    smart(&pool, alice, "HAND", None).await;
    smart(
        &pool,
        alice,
        "BAD",
        Some(r#"{"match":"all","rules":[{"field":"titel","op":"is","value":"x"}]}"#),
    )
    .await;
    smart(
        &pool,
        alice,
        "GENRES",
        Some(r#"{"match":"all","rules":[],"sort":{"field":"genre","dir":"asc"}}"#),
    )
    .await;
    smart(&pool, alice, "EMPTY", Some("")).await;
    for (uri, want, says) in [
        (
            "/api/v1/playlists/HAND/songs",
            StatusCode::CONFLICT,
            "isn't a smart playlist",
        ),
        (
            "/api/v1/playlists/BAD/songs",
            StatusCode::CONFLICT,
            "no such rule field: titel",
        ),
        (
            "/api/v1/playlists/GENRES/songs",
            StatusCode::CONFLICT,
            "genre has no single value",
        ),
        (
            "/api/v1/playlists/EMPTY/songs",
            StatusCode::CONFLICT,
            "has no rules",
        ),
        (
            "/api/v1/playlists/NONE/songs",
            StatusCode::NOT_FOUND,
            "no such playlist",
        ),
        (
            "/api/v1/playlists/GENRES/songs?offset_minutes=900",
            StatusCode::BAD_REQUEST,
            "offset_minutes",
        ),
    ] {
        let (status, body) = get(&app, &token, uri).await;
        assert_eq!(status, want, "{uri}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains(says),
            "{uri}: {body}"
        );
    }
}
