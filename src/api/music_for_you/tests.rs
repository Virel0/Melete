use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::for_you::testing::{NOW, World, id, on, world};

async fn token(pool: &SqlitePool, user: i64) -> String {
    let (_, token) = crate::auth::issue_token(pool, user, crate::auth::TokenKind::Session, "t")
        .await
        .unwrap();
    token
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

/// The world with Alice's list made, seeing everything.
async fn made() -> (World, Router, String) {
    let w = world().await;
    let access = Access::everything();
    for_you::make_for(&w.pool, &w.client, w.alice, &access, NOW)
        .await
        .unwrap();
    let alice = token(&w.pool, w.alice).await;
    let app = routes(w.pool.clone());
    (w, app, alice)
}

/// The fields `schema` documents.
fn documented(schema: &str) -> BTreeSet<String> {
    let spec: Value = serde_json::from_str(crate::api::OPENAPI).unwrap();
    spec["components"]["schemas"][schema]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("no schema {schema}"))
        .keys()
        .cloned()
        .collect()
}

fn fields(value: &Value) -> BTreeSet<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

#[tokio::test]
async fn serves_the_list_in_the_documented_shape() {
    let (_w, app, alice) = made().await;
    let (status, body) = get(&app, &alice, "/api/v1/music/for-you").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fields(&body), documented("MusicForYou"));
    assert_eq!(
        (&body["state"], &body["made_at"]),
        (&json!("ready"), &json!(NOW))
    );
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 7);
    for item in items {
        assert_eq!(fields(item), documented("MusicForYouItem"));
        assert_eq!(fields(&item["because"]), documented("MusicForYouBecause"));
    }
    let playable = &items[0];
    assert_eq!(playable["state"], "playable");
    assert_eq!(fields(&playable["track"]), documented("MusicTrack"));
    assert_eq!(playable["track"]["id"], 2);
    let song = &items[5];
    assert_eq!(fields(&song["song"]), documented("MusicForYouSong"));
    assert_eq!(
        (&song["state"], &song["request_id"], &song["track"]),
        (&json!("requestable"), &Value::Null, &Value::Null)
    );
    assert_eq!(
        song["song"],
        json!({
            "kind": "song", "mb_recording_id": id(20), "title": "Far Away", "artist": "Dune",
            "mb_artist_ids": [id(120)], "release": "Some Album", "poster": null,
        }),
        "what asking for it takes (kind and recording), and no cover with those covers off"
    );
    assert_eq!(
        items[0]["because"],
        json!({"kind": "song", "title": "Seed Song", "artist": "Alder"})
    );
    assert_eq!(
        items[1]["because"],
        json!({"kind": "artist", "title": null, "artist": "Alder"})
    );
}

#[tokio::test]
async fn says_which_songs_are_asked_for_already() {
    let (w, app, alice) = made().await;
    sqlx::query(
        "INSERT INTO requests (user_id, kind, mb_recording_id, title, artist, state, created_at)
         VALUES (?, 'song', ?, 'Far Away', 'Dune', 'requested', 0)",
    )
    .bind(w.alice)
    .bind(id(20))
    .execute(&w.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wanted_songs (mb_recording_id, title, artist, source, added_at)
         VALUES (?, 'Wanted Song', 'Echo', 'manual', 0)",
    )
    .bind(id(22))
    .execute(&w.pool)
    .await
    .unwrap();
    let (_, body) = get(&app, &alice, "/api/v1/music/for-you").await;
    let request: i64 = sqlx::query_scalar("SELECT id FROM requests")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let marks: Vec<(Value, Value)> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["track"].is_null())
        .map(|i| (i["state"].clone(), i["request_id"].clone()))
        .collect();
    assert_eq!(
        marks,
        [
            (json!("requested"), json!(request)),
            (json!("requested"), Value::Null)
        ],
        "her own request, then one wanted already"
    );
    // Someone else's request isn't hers.
    sqlx::query("UPDATE requests SET user_id = ?")
        .bind(w.bob)
        .execute(&w.pool)
        .await
        .unwrap();
    let (_, body) = get(&app, &alice, "/api/v1/music/for-you").await;
    assert_eq!(body["items"][5]["state"], "requestable");
}

#[tokio::test]
async fn covers_are_listed_only_for_the_listener_whose_list_it_is() {
    let (w, app, alice) = made().await;
    on(&w.pool, "MELETE_SEARCH_COVERS").await;
    let (_, body) = get(&app, &alice, "/api/v1/music/for-you").await;
    let group = id(220);
    assert_eq!(
        body["items"][5]["song"]["poster"],
        json!(format!("/api/v1/music/release-groups/{group}/cover"))
    );
    let now = crate::db::unix_now();
    assert!(
        crate::search_covers::servable(&w.pool, &group, w.alice, now)
            .await
            .unwrap()
    );
    assert!(
        !crate::search_covers::servable(&w.pool, &group, w.bob, now)
            .await
            .unwrap(),
        "listed for Alice, so not served to Bob"
    );
}

#[tokio::test]
async fn each_listener_is_served_their_own_list_alone() {
    let (w, app, _) = made().await;
    let bob = token(&w.pool, w.bob).await;
    let (status, body) = get(&app, &bob, "/api/v1/music/for-you").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"state": "waiting", "made_at": null, "items": []})
    );
    let (status, _) = get(&app, "not-a-token", "/api/v1/music/for-you").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn takes_a_limit_and_says_when_its_off() {
    let (w, app, alice) = made().await;
    let (_, body) = get(&app, &alice, "/api/v1/music/for-you?limit=2").await;
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    for bad in ["0", "101", "x"] {
        let uri = format!("/api/v1/music/for-you?limit={bad}");
        assert_eq!(get(&app, &alice, &uri).await.0, StatusCode::BAD_REQUEST);
    }
    sqlx::query("UPDATE settings SET value = 'off' WHERE name = 'MELETE_MUSICBRAINZ'")
        .execute(&w.pool)
        .await
        .unwrap();
    let (_, body) = get(&app, &alice, "/api/v1/music/for-you").await;
    assert_eq!(body, json!({"state": "off", "made_at": null, "items": []}));
}
