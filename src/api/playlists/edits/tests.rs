use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

/// A playlist row as an Aoide device decodes it (the apps' `Playlist` record): the fields it
/// requires, the optional ones it knows, flags as booleans or integers.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct DevicePlaylist {
    id: String,
    name: String,
    notes: Option<String>,
    folder_id: Option<String>,
    #[serde(deserialize_with = "flag")]
    is_smart: bool,
    smart_rules: Option<String>,
    sort_index: String,
    updated_at: i64,
    #[serde(deserialize_with = "flag")]
    deleted: bool,
    origin_device: String,
    field_updated_at: Option<BTreeMap<String, i64>>,
    image_hash: Option<String>,
}

/// A playlist item as a device decodes it (the apps' `PlaylistItem` record).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct DeviceItem {
    id: String,
    playlist_id: String,
    jellyfin_id: String,
    content_key: String,
    position: String,
    updated_at: i64,
    #[serde(deserialize_with = "flag")]
    deleted: bool,
    origin_device: String,
}

fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    match Value::deserialize(d)? {
        Value::Bool(b) => Ok(b),
        Value::Number(n) if n.is_i64() => Ok(n.as_i64() != Some(0)),
        other => Err(serde::de::Error::custom(format!("not a flag: {other}"))),
    }
}

struct Fixture {
    app: Router,
    pool: SqlitePool,
    _dir: tempfile::TempDir,
}

/// Three tracks, the playlist routes and Aoide's sync routes.
async fn fixture() -> Fixture {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Post', 'Björk', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, genres,
             duration_seconds) VALUES
             (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Army of Me', '[]', 234.0),
             (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Hyperballad', '[]', 321.0),
             (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'Isobel', '[]', 347.0);",
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

async fn session(f: &Fixture, name: &str) -> String {
    let user = match auth::find_user(&f.pool, name).await.unwrap() {
        Some(user) => user,
        None => crate::auth::testing::user(&f.pool, name, false).await,
    };
    auth::issue_token(&f.pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
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

/// A device's sync: what it pulls past `since`, and its new cursor.
async fn pull(f: &Fixture, token: &str, since: i64) -> (Vec<Value>, i64) {
    let (status, body) = call(
        f,
        token,
        "GET",
        &format!("/aoide/sync/pull?since={since}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    (
        body["ops"].as_array().unwrap().clone(),
        body["cursor"].as_i64().unwrap(),
    )
}

/// Pushes one op as a device would.
async fn push(f: &Fixture, token: &str, device: &str, entity: &str, payload: Value) {
    let op = json!({ "opId": format!("{device}-{}", payload["updatedAt"]), "entity": entity,
        "entityId": payload["id"], "operation": "upsert", "payload": payload,
        "createdAt": payload["updatedAt"] });
    let (status, body) = call(
        f,
        token,
        "POST",
        "/aoide/sync/push",
        Some(json!({ "deviceId": device, "ops": [op] })),
    )
    .await;
    assert_eq!(
        (status, &body["rejected"]),
        (StatusCode::OK, &json!([])),
        "{body}"
    );
}

/// Checks a pulled op is one a device applies, as its sync engine checks it.
fn applies(op: &Value) {
    let payload = &op["payload"];
    match op["entity"].as_str() {
        Some("playlists") => {
            let row: DevicePlaylist = serde_json::from_value(payload.clone())
                .unwrap_or_else(|e| panic!("a device can't decode {op}: {e}"));
            assert_eq!(row.deleted, op["operation"] == "delete");
            assert_eq!(op["deviceId"], row.origin_device.as_str());
        }
        Some("playlist_items") => {
            let row: DeviceItem = serde_json::from_value(payload.clone())
                .unwrap_or_else(|e| panic!("a device can't decode {op}: {e}"));
            assert_eq!(row.deleted, op["operation"] == "delete");
            assert_eq!(op["deviceId"], row.origin_device.as_str());
        }
        other => panic!("unexpected entity {other:?}"),
    }
    assert_eq!(op["entityId"], payload["id"], "the op names its row");
}

/// The device's playlists and items after applying `ops`, by the apps' merge.
fn on_device(ops: &[Value]) -> (BTreeMap<String, Value>, Vec<(String, String)>) {
    let of = |entity: &str| -> Vec<(String, String)> {
        ops.iter()
            .filter(|op| op["entity"] == entity)
            .map(|op| {
                (
                    op["operation"].as_str().unwrap().to_owned(),
                    op["payload"].to_string(),
                )
            })
            .collect()
    };
    let lists = of("playlists");
    let lists = playlists::fold(true, lists.iter().map(|(o, p)| (o.as_str(), p.as_str())));
    let items = of("playlist_items");
    let items = playlists::fold(false, items.iter().map(|(o, p)| (o.as_str(), p.as_str())));
    let mut live: Vec<(String, String)> = items
        .values()
        .filter(|i| !playlists::deleted(i))
        .map(|i| {
            (
                i["position"].as_str().unwrap().to_owned(),
                i["jellyfinId"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    live.sort();
    let lists = lists
        .into_iter()
        .map(|(id, row)| (id, Value::Object(row)))
        .collect();
    (lists, live)
}

fn tracks_of(body: &Value) -> Vec<i64> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["track"]["id"].as_i64().unwrap())
        .collect()
}

/// A phone and the web, both signed in as alice, and what the phone has pulled.
struct Devices {
    f: Fixture,
    phone: String,
    web: String,
    /// The playlist made on the web.
    id: String,
    seen: Vec<Value>,
    cursor: i64,
}

impl Devices {
    /// The phone syncs: what's new, each op checked as its sync engine checks it.
    async fn sync(&mut self) -> Vec<Value> {
        let (new, cursor) = pull(&self.f, &self.phone, self.cursor).await;
        new.iter().for_each(applies);
        self.cursor = cursor;
        self.seen.extend(new.iter().cloned());
        new
    }

    async fn web(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        call(&self.f, &self.web, method, uri, body).await
    }

    async fn phone_pushes(&self, entity: &str, payload: Value) {
        push(&self.f, &self.phone, "phone", entity, payload).await;
    }
}

/// "Late Night", made on the web with tracks 1 and 2, as the phone has it after syncing.
async fn made_on_the_web() -> Devices {
    let f = fixture().await;
    let phone = session(&f, "alice").await;
    let web = session(&f, "alice").await;
    let mut d = Devices {
        f,
        phone,
        web,
        id: String::new(),
        seen: Vec::new(),
        cursor: 0,
    };
    let body = json!({ "name": " Late Night ", "notes": "For the drive" });
    let (status, made) = d.web("POST", "/api/v1/playlists", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(
        (&made["name"], &made["mine"], &made["tracks"]),
        (&json!("Late Night"), &json!(true), &json!(0))
    );
    d.id = made["id"].as_str().unwrap().to_owned();
    let items = format!("/api/v1/playlists/{}/items", d.id);
    let (status, body) = d
        .web("POST", &items, Some(json!({ "track_ids": [1, 2] })))
        .await;
    assert_eq!((status, tracks_of(&body)), (StatusCode::OK, vec![1, 2]));
    d
}

#[tokio::test]
async fn a_playlist_made_on_the_web_reaches_the_device() {
    let mut d = made_on_the_web().await;
    let new = d.sync().await;
    let kinds: Vec<&str> = new
        .iter()
        .map(|op| op["entity"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["playlists", "playlist_items", "playlist_items"]);
    // Every field of a new row is stamped, as a device stamps one.
    let row = &new[0]["payload"];
    let stamps = row["fieldUpdatedAt"].as_object().unwrap();
    for field in [
        "id",
        "name",
        "notes",
        "isSmart",
        "sortIndex",
        "updatedAt",
        "deleted",
        "originDevice",
    ] {
        assert_eq!(
            stamps.get(field),
            Some(&row["updatedAt"]),
            "{field} stamped"
        );
    }
    assert_eq!(row["sortIndex"], "V", "the first key there is");
    assert_eq!(new[1]["payload"]["contentKey"], "bjork|post|army of me|234");
    assert_eq!(new[1]["payload"]["playlistId"], d.id.as_str());
    assert_eq!(
        on_device(&d.seen).1,
        [("V".into(), "1".into()), ("W".into(), "2".into())]
    );
    let (lists, _) = on_device(&d.seen);
    assert_eq!(lists[&d.id]["name"], "Late Night");
}

#[tokio::test]
async fn edits_on_the_web_and_a_device_merge_field_by_field() {
    let mut d = made_on_the_web().await;
    let row = d.sync().await[0]["payload"].clone();
    let made_at = row["updatedAt"].as_i64().unwrap();
    // The web changes the notes; the phone, offline, had renamed it a moment before and syncs
    // after. The web's write is the later one, yet both stand, on the web and on the phone,
    // as each field keeps its own time.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let uri = format!("/api/v1/playlists/{}", d.id);
    let (status, body) = d
        .web("PUT", &uri, Some(json!({ "notes": "Windows down" })))
        .await;
    assert_eq!(
        (status, &body["notes"]),
        (StatusCode::OK, &json!("Windows down"))
    );
    let mut stamps = row["fieldUpdatedAt"].clone();
    stamps["name"] = json!(made_at + 1);
    d.phone_pushes(
        "playlists",
        json!({ "id": d.id, "name": "Night Drive", "notes": "For the drive",
        "isSmart": false, "sortIndex": row["sortIndex"], "updatedAt": made_at + 1,
        "deleted": false, "originDevice": "phone", "fieldUpdatedAt": stamps }),
    )
    .await;
    let (_, body) = d.web("GET", &uri, None).await;
    assert_eq!(
        (&body["name"], &body["notes"]),
        (&json!("Night Drive"), &json!("Windows down"))
    );
    let new = d.sync().await;
    assert_eq!(
        new[0]["payload"]["fieldUpdatedAt"]["name"], made_at,
        "only what changed is stamped anew"
    );
    let (lists, _) = on_device(&d.seen);
    assert_eq!(
        (&lists[&d.id]["name"], &lists[&d.id]["notes"]),
        (&json!("Night Drive"), &json!("Windows down"))
    );
}

#[tokio::test]
async fn a_device_item_moved_and_removed_on_the_web() {
    let mut d = made_on_the_web().await;
    d.sync().await;
    // The phone adds a track; the web sees it and moves it to the top.
    d.phone_pushes(
        "playlist_items",
        json!({ "id": "PHONE-ITEM", "playlistId": d.id,
        "jellyfinId": "3", "contentKey": "bjork|post|isobel|347", "position": "X",
        "updatedAt": 5000, "deleted": false, "originDevice": "phone" }),
    )
    .await;
    let uri = format!("/api/v1/playlists/{}", d.id);
    assert_eq!(tracks_of(&d.web("GET", &uri, None).await.1), [1, 2, 3]);
    let moved = format!("{uri}/items/PHONE-ITEM");
    let (status, body) = d.web("PUT", &moved, Some(json!({ "index": 0 }))).await;
    assert_eq!((status, tracks_of(&body)), (StatusCode::OK, vec![3, 1, 2]));
    // Already there: nothing more is written.
    d.web("PUT", &moved, Some(json!({ "index": 0 }))).await;
    let new = d.sync().await;
    assert_eq!(
        new.len(),
        2,
        "the phone's own item, then the web's move: {new:?}"
    );
    assert_eq!(new[1]["entityId"], "PHONE-ITEM");
    let order: Vec<String> = on_device(&d.seen).1.into_iter().map(|(_, t)| t).collect();
    assert_eq!(order, ["3", "1", "2"]);
    // Removed on the web: the phone's merge drops it.
    let item = body["items"][1]["id"].as_str().unwrap().to_owned();
    let (status, body) = d.web("DELETE", &format!("{uri}/items/{item}"), None).await;
    assert_eq!((status, tracks_of(&body)), (StatusCode::OK, vec![3, 2]));
    let new = d.sync().await;
    assert_eq!(
        (new.len(), new[0]["operation"].as_str()),
        (1, Some("delete"))
    );
    let order: Vec<String> = on_device(&d.seen).1.into_iter().map(|(_, t)| t).collect();
    assert_eq!(order, ["3", "2"]);
}

#[tokio::test]
async fn a_playlist_deleted_on_the_web_goes_from_the_device() {
    let mut d = made_on_the_web().await;
    d.sync().await;
    let uri = format!("/api/v1/playlists/{}", d.id);
    let (status, _) = d.web("DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // Its items first, then the playlist, as a device deletes one.
    let new = d.sync().await;
    let kinds: Vec<&str> = new
        .iter()
        .map(|op| op["entity"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["playlist_items", "playlist_items", "playlists"]);
    let (lists, items) = on_device(&d.seen);
    assert!(playlists::deleted(lists[&d.id].as_object().unwrap()) && items.is_empty());
    assert_eq!(d.web("GET", &uri, None).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_the_owner_edits_and_smart_playlists_stay_on_the_devices() {
    let f = fixture().await;
    let alice = session(&f, "alice").await;
    let bob = session(&f, "bob").await;
    let (_, made) = call(
        &f,
        &alice,
        "POST",
        "/api/v1/playlists",
        Some(json!({ "name": "Mine" })),
    )
    .await;
    let id = made["id"].as_str().unwrap().to_owned();
    let edits = [
        (
            "PUT",
            format!("/api/v1/playlists/{id}"),
            Some(json!({ "name": "Ours" })),
        ),
        ("DELETE", format!("/api/v1/playlists/{id}"), None),
        (
            "POST",
            format!("/api/v1/playlists/{id}/items"),
            Some(json!({ "track_ids": [1] })),
        ),
    ];
    // Not shared with him: no such playlist.
    for (method, uri, body) in &edits {
        let (status, _) = call(&f, &bob, method, uri, body.clone()).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    // Shared with him for editing: he edits it here as his devices may, but only its owner
    // deletes it.
    sqlx::query(
        "INSERT INTO aoide_playlist_shares (playlist_id, grantee_user_id, can_edit, created_at)
                 SELECT ?, id, 1, 0 FROM users WHERE username = 'bob'",
    )
    .bind(&id)
    .execute(&f.pool)
    .await
    .unwrap();
    for (method, uri, body) in &edits {
        let (status, body) = call(&f, &bob, method, uri, body.clone()).await;
        if *method == "DELETE" {
            assert_eq!(
                (status, &body["error"]),
                (
                    StatusCode::FORBIDDEN,
                    &json!("only its owner deletes a playlist or files it in a folder")
                )
            );
        } else {
            assert_eq!(status, StatusCode::OK, "{method} {uri}: {body}");
        }
    }
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM aoide_ops")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    };
    assert_eq!(count().await, 3, "his rename and his track, and no delete");

    // A smart playlist from a device.
    push(&f, &alice, "phone", "playlists", json!({ "id": "SMART", "name": "Rules", "isSmart": true,
        "smartRules": "{}", "sortIndex": "W", "updatedAt": 10, "deleted": false, "originDevice": "phone" })).await;
    for (method, uri, body) in [
        (
            "PUT",
            "/api/v1/playlists/SMART",
            Some(json!({ "name": "x" })),
        ),
        ("DELETE", "/api/v1/playlists/SMART", None),
        (
            "POST",
            "/api/v1/playlists/SMART/items",
            Some(json!({ "track_ids": [1] })),
        ),
    ] {
        let (status, _) = call(&f, &alice, method, uri, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {uri}");
    }
    assert_eq!(count().await, 4);
}

#[tokio::test]
async fn bad_requests_are_refused_before_anything_is_written() {
    let f = fixture().await;
    let alice = session(&f, "alice").await;
    for body in [
        json!({ "name": "  " }),
        json!({ "name": "x".repeat(201) }),
        json!({ "name": "a", "smart": true }),
    ] {
        let (status, _) = call(&f, &alice, "POST", "/api/v1/playlists", Some(body.clone())).await;
        assert!(status.is_client_error(), "{body}: {status}");
    }
    let (_, made) = call(
        &f,
        &alice,
        "POST",
        "/api/v1/playlists",
        Some(json!({ "name": "Mine" })),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let items = format!("/api/v1/playlists/{id}/items");
    for (body, status) in [
        (json!({ "track_ids": [] }), StatusCode::BAD_REQUEST),
        (json!({ "track_ids": [1, 99] }), StatusCode::NOT_FOUND),
    ] {
        assert_eq!(call(&f, &alice, "POST", &items, Some(body)).await.0, status);
    }
    let (status, _) = call(&f, &alice, "DELETE", &format!("{items}/NOPE"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Notes cleared with null; the name left as it was.
    call(
        &f,
        &alice,
        "PUT",
        &format!("/api/v1/playlists/{id}"),
        Some(json!({ "notes": "x" })),
    )
    .await;
    let (_, body) = call(
        &f,
        &alice,
        "PUT",
        &format!("/api/v1/playlists/{id}"),
        Some(json!({ "notes": null })),
    )
    .await;
    assert_eq!(
        (&body["name"], &body["notes"]),
        (&json!("Mine"), &Value::Null)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_ops")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(count, 3, "the playlist and two changes to its notes");
}

/// A PNG's signature and a few bytes: enough for the type check.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really the rest of a picture";

/// Sends `body` as an upload of type `mime`.
async fn upload(
    f: &Fixture,
    token: &str,
    uri: &str,
    mime: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("PUT")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, mime)
        .body(Body::from(body))
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn images_held(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM aoide_images")
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_picture_set_on_the_web_reaches_the_device() {
    let mut d = made_on_the_web().await;
    d.sync().await;
    let uri = format!("/api/v1/playlists/{}/image", d.id);
    let (status, body) = upload(&d.f, &d.web, &uri, "image/png", PNG.to_vec()).await;
    let hash = crate::aoide::library::sha256_hex(PNG);
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["image_url"], format!("/aoide/images/{hash}"));
    let alice: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'")
        .fetch_one(&d.f.pool)
        .await
        .unwrap();
    let held = crate::aoide::library::image_seen_by(&d.f.pool, alice, &hash)
        .await
        .unwrap();
    assert_eq!(held, Some((PNG.to_vec(), "image/png".to_owned())));

    // The phone gets the picture as a device's change, its own field stamped.
    let new = d.sync().await;
    assert_eq!(new.len(), 1);
    let row = &new[0]["payload"];
    assert_eq!(row["imageHash"], hash.as_str());
    assert_eq!(row["fieldUpdatedAt"]["imageHash"], row["updatedAt"]);
    assert_eq!(row["name"], "Late Night", "the rest of the row as it was");
    assert_eq!(on_device(&d.seen).0[&d.id]["imageHash"], hash.as_str());

    // The same picture again changes nothing.
    upload(&d.f, &d.web, &uri, "image/png", PNG.to_vec()).await;
    assert!(d.sync().await.is_empty());

    let (status, body) = d.web("DELETE", &uri, None).await;
    assert_eq!((status, &body["image_url"]), (StatusCode::OK, &Value::Null));
    d.sync().await;
    assert!(on_device(&d.seen).0[&d.id].get("imageHash").is_none());
}

#[tokio::test]
async fn a_picture_is_refused_before_anything_is_stored() {
    let d = made_on_the_web().await;
    let uri = format!("/api/v1/playlists/{}/image", d.id);
    let refused = [
        ("image/gif", PNG.to_vec(), StatusCode::BAD_REQUEST),
        ("image/png", Vec::new(), StatusCode::BAD_REQUEST),
        ("image/jpeg", PNG.to_vec(), StatusCode::BAD_REQUEST),
        ("image/webp", b"<html>".to_vec(), StatusCode::BAD_REQUEST),
        (
            "image/png",
            [PNG, &vec![0; crate::aoide::library::MAX_IMAGE_BYTES]].concat(),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ];
    for (mime, body, expected) in refused {
        let (status, said) = upload(&d.f, &d.web, &uri, mime, body).await;
        assert_eq!(status, expected, "{mime}: {said}");
    }
    // Someone else's playlist: no such playlist for them, and nothing stored.
    let bob = session(&d.f, "bob").await;
    let (status, _) = upload(&d.f, &bob, &uri, "image/png", PNG.to_vec()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&d.f, &bob, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(images_held(&d.f).await, 0);
    let (_, still) = d
        .web("GET", &format!("/api/v1/playlists/{}", d.id), None)
        .await;
    assert_eq!(still["image_url"], Value::Null);
}

/// Someone a playlist is shared with to edit edits it on the web as their devices may: renames
/// it, sets its picture, adds, moves and removes songs and adds one waiting to arrive, each
/// written into their own log, which the owner's devices pull. Only the owner deletes it or files
/// it in a folder; someone it's shared with to listen to changes nothing; a smart playlist stays
/// with its rules.
#[tokio::test]
async fn an_editor_edits_a_shared_playlist_but_doesnt_delete_or_file_it() {
    let mut d = made_on_the_web().await;
    d.sync().await;
    let bob = session(&d.f, "bob").await;
    let carol = session(&d.f, "carol").await;
    let id_of = |name: &'static str| {
        let pool = d.f.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE username = ?")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let (bob_id, carol_id) = (id_of("bob").await, id_of("carol").await);
    for (user, can_edit) in [(bob_id, true), (carol_id, false)] {
        crate::aoide::library::share(&d.f.pool, &d.id, user, can_edit, 1)
            .await
            .unwrap();
    }
    let playlist = format!("/api/v1/playlists/{}", d.id);
    let items = format!("{playlist}/items");

    // What an editor may do.
    let (status, body) = call(
        &d.f,
        &bob,
        "PUT",
        &playlist,
        Some(json!({ "name": "Our Night", "notes": "Both of us" })),
    )
    .await;
    assert_eq!(
        (status, &body["name"]),
        (StatusCode::OK, &json!("Our Night"))
    );
    let (status, body) = upload(
        &d.f,
        &bob,
        &format!("{playlist}/image"),
        "image/png",
        PNG.to_vec(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &d.f,
        &bob,
        "POST",
        &items,
        Some(json!({ "track_ids": [3], "index": 0 })),
    )
    .await;
    assert_eq!((status, tracks_of(&body)), (StatusCode::OK, vec![3, 1, 2]));
    let first = body["items"][0]["id"].as_str().unwrap().to_owned();
    let (status, body) = call(
        &d.f,
        &bob,
        "PUT",
        &format!("{items}/{first}"),
        Some(json!({ "index": 2 })),
    )
    .await;
    assert_eq!((status, tracks_of(&body)), (StatusCode::OK, vec![1, 2, 3]));
    let last = body["items"][0]["id"].as_str().unwrap().to_owned();
    let (status, body) = call(&d.f, &bob, "DELETE", &format!("{items}/{last}"), None).await;
    assert_eq!((status, tracks_of(&body)), (StatusCode::OK, vec![2, 3]));

    // The owner's phone pulls all of it, from his device.
    let new = d.sync().await;
    assert!(!new.is_empty());
    assert!(new.iter().all(|op| op["deviceId"] != "phone"));
    let (lists, live) = on_device(&d.seen);
    assert_eq!(lists[&d.id]["name"], "Our Night");
    assert_eq!(lists[&d.id]["notes"], "Both of us");
    assert_eq!(
        lists[&d.id]["imageHash"],
        crate::aoide::library::sha256_hex(PNG).as_str()
    );
    assert_eq!(
        live.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
        ["2", "3"]
    );

    // And a song he asked for, waiting in it until it arrives.
    let asked: i64 = sqlx::query_scalar(
        "INSERT INTO requests (user_id, kind, mb_recording_id, title, artist, state, created_at)
         VALUES (?, 'song', 'rec-new', 'Hunter', 'Björk', 'requested', 1) RETURNING id",
    )
    .bind(bob_id)
    .fetch_one(&d.f.pool)
    .await
    .unwrap();
    let (status, body) = call(
        &d.f,
        &bob,
        "POST",
        &format!("{playlist}/waiting"),
        Some(json!({ "request_id": asked })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["items"][2]["pending"]["state"], "requested");
    // The owner sees it waiting, as he would one of his own.
    let (_, seen) = call(&d.f, &d.web, "GET", &playlist, None).await;
    assert_eq!(seen["items"][2]["pending"]["title"], "Hunter");

    // What an editor still may not do: delete it, or file it in a folder (folders are each
    // user's own).
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM aoide_ops")
            .fetch_one(&d.f.pool)
            .await
            .unwrap()
    };
    let before = count().await;
    for (method, body) in [
        ("DELETE", None),
        ("PUT", Some(json!({ "folder_id": null }))),
    ] {
        let (status, said) = call(&d.f, &bob, method, &playlist, body).await;
        assert_eq!(
            (status, &said["error"]),
            (
                StatusCode::FORBIDDEN,
                &json!("only its owner deletes a playlist or files it in a folder")
            ),
            "{method}"
        );
    }
    // Shared to listen to: nothing.
    for (method, uri, body) in [
        ("PUT", playlist.clone(), Some(json!({ "name": "Mine now" }))),
        ("POST", items.clone(), Some(json!({ "track_ids": [1] }))),
        ("DELETE", playlist.clone(), None),
    ] {
        let (status, said) = call(&d.f, &carol, method, &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {said}");
    }
    let (_, said) = call(
        &d.f,
        &carol,
        "PUT",
        &playlist,
        Some(json!({ "name": "Mine now" })),
    )
    .await;
    assert_eq!(
        said["error"],
        "it's shared with you to listen to, not to change"
    );
    // A smart playlist shared to edit stays with its rules here.
    push(
        &d.f,
        &d.phone,
        "phone",
        "playlists",
        json!({ "id": "SMART", "name": "Rules",
        "isSmart": true, "smartRules": "{}", "sortIndex": "W", "updatedAt": 10,
        "deleted": false, "originDevice": "phone" }),
    )
    .await;
    crate::aoide::library::share(&d.f.pool, "SMART", bob_id, true, 1)
        .await
        .unwrap();
    let before_smart = count().await;
    let (status, _) = call(
        &d.f,
        &bob,
        "PUT",
        "/api/v1/playlists/SMART",
        Some(json!({ "name": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(count().await, before_smart);
    assert_eq!(
        before_smart,
        before + 1,
        "only the smart playlist's own push"
    );
}
