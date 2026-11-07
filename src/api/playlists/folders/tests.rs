use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

/// A folder row as an Aoide device decodes it (the apps' `Folder` record).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
struct DeviceFolder {
    id: String,
    name: String,
    parent_id: Option<String>,
    sort_index: String,
    updated_at: i64,
    deleted: bool,
    origin_device: String,
    field_updated_at: Option<BTreeMap<String, i64>>,
}

struct Fixture {
    app: Router,
    pool: SqlitePool,
    _dir: tempfile::TempDir,
}

/// One track (7, MusicBrainz recording "rec-7"), the playlist routes and Aoide's sync routes.
async fn fixture() -> Fixture {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Post', 'Björk', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, genres,
             duration_seconds, mb_recording_id)
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Army of Me', '[]', 234.0, 'rec-7');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app =
        routes(pool.clone()).merge(crate::api::aoide::routes(pool.clone(), dir.path().into()));
    Fixture {
        app,
        pool,
        _dir: dir,
    }
}

/// A session for `name`, made on first use, and their user id.
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

async fn call(
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

async fn ops(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM aoide_ops")
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

/// Every folder op a device of `token`'s pulls, each one it decodes.
async fn pulled_folders(f: &Fixture, token: &str) -> Vec<Value> {
    let (status, body) = call(f, token, "GET", "/aoide/sync/pull?since=0", None).await;
    assert_eq!(status, StatusCode::OK);
    let folders: Vec<Value> = body["ops"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| op["entity"] == "folders")
        .cloned()
        .collect();
    for op in &folders {
        let row: DeviceFolder = serde_json::from_value(op["payload"].clone())
            .unwrap_or_else(|e| panic!("a device can't decode {op}: {e}"));
        assert_eq!(row.deleted, op["operation"] == "delete");
        assert_eq!(op["entityId"], row.id.as_str());
    }
    folders
}

#[tokio::test]
async fn folders_made_filed_and_deleted_on_the_web_reach_the_devices() {
    let f = fixture().await;
    let (alice, _) = session(&f, "alice").await;
    let post = |uri: &'static str, body: Value| call(&f, &alice, "POST", uri, Some(body));
    let (status, moods) = post("/api/v1/playlists/folders", json!({ "name": " Moods " })).await;
    assert_eq!(status, StatusCode::CREATED);
    let moods_id = moods["id"].as_str().unwrap().to_owned();
    assert_eq!(
        moods,
        json!({ "id": moods_id, "name": "Moods", "parent_id": null, "playlists": 0 })
    );
    let (_, calm) = post(
        "/api/v1/playlists/folders",
        json!({ "name": "Calm", "parent_id": moods_id }),
    )
    .await;
    let calm_id = calm["id"].as_str().unwrap().to_owned();
    assert_eq!(calm["parent_id"], moods_id.as_str());
    let (_, mix) = post("/api/v1/playlists", json!({ "name": "Mix" })).await;
    let mix_uri = format!("/api/v1/playlists/{}", mix["id"].as_str().unwrap());

    let (status, filed) = call(
        &f,
        &alice,
        "PUT",
        &mix_uri,
        Some(json!({ "folder_id": moods_id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&filed["folder"], &filed["folder_id"], &filed["name"]),
        (&json!("Moods"), &json!(moods_id), &json!("Mix"))
    );
    let (_, listed) = call(&f, &alice, "GET", "/api/v1/playlists/folders", None).await;
    assert_eq!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (
                f["name"].as_str().unwrap(),
                f["playlists"].as_i64().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("Moods", 1), ("Calm", 0)],
        "in the order made"
    );
    let (status, renamed) = call(
        &f,
        &alice,
        "PUT",
        &format!("/api/v1/playlists/folders/{moods_id}"),
        Some(json!({ "name": "Feelings" })),
    )
    .await;
    assert_eq!(
        (status, &renamed["name"]),
        (StatusCode::OK, &json!("Feelings"))
    );

    // Deleting it deletes no playlist: what was in it moves up to the top, where it was.
    let (status, _) = call(
        &f,
        &alice,
        "DELETE",
        &format!("/api/v1/playlists/folders/{moods_id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, listed) = call(&f, &alice, "GET", "/api/v1/playlists/folders", None).await;
    assert_eq!(
        listed,
        json!([{ "id": calm_id, "name": "Calm", "parent_id": null, "playlists": 0 }])
    );
    let (_, mix_now) = call(&f, &alice, "GET", &mix_uri, None).await;
    assert_eq!(
        (&mix_now["folder"], &mix_now["folder_id"]),
        (&Value::Null, &Value::Null)
    );

    // A device of hers sees the same folders, by the apps' merge.
    let pulled = pulled_folders(&f, &alice).await;
    let folded = playlists::fold(
        true,
        pulled
            .iter()
            .map(|op| (op["operation"].as_str().unwrap(), op["payload"].to_string()))
            .collect::<Vec<_>>()
            .iter()
            .map(|(o, p)| (*o, p.as_str())),
    );
    assert!(playlists::deleted(&folded[&moods_id]));
    assert_eq!(folded[&moods_id]["name"], "Feelings");
    assert!(!playlists::deleted(&folded[&calm_id]));
    assert!(!folded[&calm_id].contains_key("parentId"));

    // Filed in Calm, then out to the top again.
    for (folder, name) in [(json!(calm_id), json!("Calm")), (Value::Null, Value::Null)] {
        let (status, body) = call(
            &f,
            &alice,
            "PUT",
            &mix_uri,
            Some(json!({ "folder_id": folder })),
        )
        .await;
        assert_eq!((status, &body["folder"]), (StatusCode::OK, &name));
    }
}

#[tokio::test]
async fn folders_are_each_users_own_and_only_the_owner_files_a_playlist() {
    let f = fixture().await;
    let (alice, _) = session(&f, "alice").await;
    let (bob, bob_id) = session(&f, "bob").await;
    let (carol, carol_id) = session(&f, "carol").await;
    let (_, folder) = call(
        &f,
        &alice,
        "POST",
        "/api/v1/playlists/folders",
        Some(json!({ "name": "Moods" })),
    )
    .await;
    let folder_id = folder["id"].as_str().unwrap().to_owned();
    let (_, mix) = call(
        &f,
        &alice,
        "POST",
        "/api/v1/playlists",
        Some(json!({ "name": "Mix" })),
    )
    .await;
    let mix_id = mix["id"].as_str().unwrap().to_owned();
    let mix_uri = format!("/api/v1/playlists/{mix_id}");
    // Bob may edit it on his devices; Carol may listen.
    for (user, can_edit) in [(bob_id, true), (carol_id, false)] {
        crate::aoide::library::share(&f.pool, &mix_id, user, can_edit, 1)
            .await
            .unwrap();
    }
    let before = ops(&f).await;
    let folder_uri = format!("/api/v1/playlists/folders/{folder_id}");
    for token in [&bob, &carol] {
        // Another's folder is as good as none.
        let (status, _) = call(&f, token, "PUT", &folder_uri, Some(json!({ "name": "x" }))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(&f, token, "DELETE", &folder_uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, theirs) = call(&f, token, "GET", "/api/v1/playlists/folders", None).await;
        assert_eq!(theirs, json!([]));
        // Filing is the owner's, editor or not.
        let (status, body) = call(
            &f,
            token,
            "PUT",
            &mix_uri,
            Some(json!({ "folder_id": null })),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        // The shared playlist shows in no folder of theirs.
        let (_, seen) = call(&f, token, "GET", &mix_uri, None).await;
        assert_eq!(seen["folder_id"], Value::Null);
    }
    let (dave, _) = session(&f, "dave").await;
    let (status, _) = call(
        &f,
        &dave,
        "PUT",
        &mix_uri,
        Some(json!({ "folder_id": null })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a stranger");
    // Bad folders and names, refused before anything is written.
    for (uri, method, body, want) in [
        (
            "/api/v1/playlists/folders",
            "POST",
            json!({ "name": "  " }),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/playlists/folders",
            "POST",
            json!({ "name": "In nothing", "parent_id": "NOPE" }),
            StatusCode::NOT_FOUND,
        ),
        (
            mix_uri.as_str(),
            "PUT",
            json!({ "folder_id": "NOPE" }),
            StatusCode::NOT_FOUND,
        ),
        (
            folder_uri.as_str(),
            "PUT",
            json!({ "name": "" }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, body) = call(&f, &alice, method, uri, Some(body)).await;
        assert_eq!(status, want, "{method} {uri}: {body}");
    }
    assert_eq!(ops(&f).await, before, "nothing written");

    // A smart playlist is filed like any other, though not renamed here.
    let (_, smart) = call(
        &f,
        &alice,
        "POST",
        "/aoide/sync/push",
        Some(
            json!({ "deviceId": "phone", "ops": [{ "opId": "s", "entity": "playlists",
            "entityId": "SMART", "operation": "upsert", "createdAt": 5, "payload": { "id": "SMART",
            "name": "Rules", "isSmart": true, "smartRules": "{}", "sortIndex": "W",
            "updatedAt": 5, "deleted": false, "originDevice": "phone" } }] }),
        ),
    )
    .await;
    assert_eq!(smart["rejected"], json!([]));
    let (status, body) = call(
        &f,
        &alice,
        "PUT",
        "/api/v1/playlists/SMART",
        Some(json!({ "folder_id": folder_id })),
    )
    .await;
    assert_eq!((status, &body["folder"]), (StatusCode::OK, &json!("Moods")));
    let (status, _) = call(
        &f,
        &alice,
        "PUT",
        "/api/v1/playlists/SMART",
        Some(json!({ "name": "New", "folder_id": null })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

/// A song request of `user`'s for recording `recording`; its id.
async fn ask(f: &Fixture, user: i64, kind: &str, recording: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO requests (user_id, kind, mb_recording_id, mb_release_group_id, title, artist,
             state, created_at)
         VALUES (?, ?, ?, ?, ?, 'Björk', 'requested', 1) RETURNING id",
    )
    .bind(user)
    .bind(kind)
    .bind(recording)
    .bind((kind == "album").then_some("rg"))
    .bind(title)
    .fetch_one(&f.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_song_asked_for_waits_in_the_playlist_and_only_the_owner_adds_one() {
    let f = fixture().await;
    let (alice, alice_id) = session(&f, "alice").await;
    let (bob, bob_id) = session(&f, "bob").await;
    let (carol, carol_id) = session(&f, "carol").await;
    let (dave, dave_id) = session(&f, "dave").await;
    let (_, mix) = call(
        &f,
        &alice,
        "POST",
        "/api/v1/playlists",
        Some(json!({ "name": "Mix" })),
    )
    .await;
    let mix_id = mix["id"].as_str().unwrap().to_owned();
    let waiting = format!("/api/v1/playlists/{mix_id}/waiting");

    let song = ask(&f, alice_id, "song", "rec-new", "Hunter").await;
    let (status, body) = call(
        &f,
        &alice,
        "POST",
        &waiting,
        Some(json!({ "request_id": song })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let item = &body["items"][0];
    assert_eq!(item["track"], Value::Null);
    assert_eq!(
        (
            &item["pending"]["title"],
            &item["pending"]["artist"],
            &item["pending"]["state"]
        ),
        (&json!("Hunter"), &json!("Björk"), &json!("requested"))
    );
    // A song the library has by now goes in as its track.
    let here = ask(&f, alice_id, "song", "rec-7", "Army of Me").await;
    let (_, body) = call(
        &f,
        &alice,
        "POST",
        &waiting,
        Some(json!({ "request_id": here, "index": 0 })),
    )
    .await;
    assert_eq!(body["items"][0]["track"]["id"], 7);
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    // Refused: someone else's request, a request that isn't a song, a playlist that isn't
    // theirs (shared to edit or to listen) or that they can't see.
    for (user, can_edit) in [(bob_id, true), (carol_id, false)] {
        crate::aoide::library::share(&f.pool, &mix_id, user, can_edit, 1)
            .await
            .unwrap();
    }
    let before = ops(&f).await;
    let album = ask(&f, alice_id, "album", "x", "Post").await;
    for (who, id, want) in [
        (
            &alice,
            ask(&f, bob_id, "song", "rec-b", "Bob's").await,
            StatusCode::NOT_FOUND,
        ),
        (&alice, album, StatusCode::NOT_FOUND),
        (
            &bob,
            ask(&f, bob_id, "song", "rec-b2", "Bob's").await,
            StatusCode::FORBIDDEN,
        ),
        (
            &carol,
            ask(&f, carol_id, "song", "rec-c", "Carol's").await,
            StatusCode::FORBIDDEN,
        ),
        (
            &dave,
            ask(&f, dave_id, "song", "rec-d", "Dave's").await,
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, body) =
            call(&f, who, "POST", &waiting, Some(json!({ "request_id": id }))).await;
        assert_eq!(status, want, "{body}");
    }
    assert_eq!(ops(&f).await, before, "nothing written");
}
