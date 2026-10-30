use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

/// A like row as an Aoide device decodes it (the apps' `Like` record): every field required,
/// flags as booleans or integers, nothing else read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct DeviceLike {
    id: String,
    jellyfin_id: String,
    content_key: String,
    #[serde(deserialize_with = "flag")]
    liked: bool,
    updated_at: i64,
    #[serde(deserialize_with = "flag")]
    deleted: bool,
    origin_device: String,
}

fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
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

/// Two tracks, the likes routes, the music routes and Aoide's sync routes.
async fn fixture() -> Fixture {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Post', 'Björk', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
             duration_seconds) VALUES
             (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Army of Me', 'Björk', '[]', 234.0),
             (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Hyperballad', 'Björk', '[]', 321.0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app = routes(pool.clone(), false)
        .merge(crate::api::aoide::routes(pool.clone(), dir.path().into()))
        .merge(crate::api::music::routes(
            pool.clone(),
            dir.path().join("covers"),
            dir.path().join("motion"),
            None,
        ));
    Fixture {
        app,
        pool,
        _dir: dir,
    }
}

async fn session(f: &Fixture, name: &str) -> String {
    let user = match auth::find_user(&f.pool, name).await.unwrap() {
        Some(user) => user,
        None => auth::create_user(&f.pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap(),
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

/// What a device pulls past `since`: the ops, and its new cursor.
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

/// Checks a pulled op is one a device applies, as its sync engine checks it, and decodes it.
fn applies(op: &Value) -> DeviceLike {
    assert_eq!(op["entity"], "likes");
    let like: DeviceLike = serde_json::from_value(op["payload"].clone())
        .unwrap_or_else(|err| panic!("a device can't decode {op}: {err}"));
    assert_eq!(op["entityId"], like.id.as_str(), "the op names its row");
    assert_eq!(
        op["deviceId"],
        like.origin_device.as_str(),
        "pushed by the device it names"
    );
    assert_eq!(
        op["operation"] == "delete",
        like.deleted,
        "a delete says so in its row"
    );
    let keys: Vec<&String> = op["payload"].as_object().unwrap().keys().collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "keys sorted, as the apps write them");
    like
}

/// The tracks a device holds as liked after applying `ops`, by the apps' merge.
fn liked_on_device(ops: &[Value]) -> Vec<String> {
    let rows: Vec<(String, String, String)> = ops
        .iter()
        .filter(|op| op["entity"] == "likes")
        .map(|op| {
            (
                op["entityId"].as_str().unwrap().to_owned(),
                op["operation"].as_str().unwrap().to_owned(),
                op["payload"].to_string(),
            )
        })
        .collect();
    let mut liked: Vec<String> = likes::fold(
        Kind::Track,
        rows.iter()
            .map(|(e, o, p)| (e.as_str(), o.as_str(), p.as_str())),
    )
    .into_iter()
    .filter(|r| r.liked && !r.deleted)
    .map(|r| r.item_id)
    .collect();
    liked.sort();
    liked
}

fn liked_ids(body: &Value) -> Vec<i64> {
    body["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["track"]["id"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn a_device_like_shows_on_the_web_and_a_web_like_reaches_the_device() {
    let f = fixture().await;
    let phone = session(&f, "alice").await;
    let web = session(&f, "alice").await;
    // The phone likes track 1 and syncs.
    let op = json!({ "opId": "op-1", "entity": "likes", "entityId": "PHONE-ROW", "operation": "upsert",
        "payload": { "id": "PHONE-ROW", "jellyfinId": "1", "contentKey": "bjork|post|army of me|234",
                     "liked": true, "updatedAt": 1000, "deleted": false, "originDevice": "phone" },
        "createdAt": 1000 });
    let (status, pushed) = call(
        &f,
        &phone,
        "POST",
        "/aoide/sync/push",
        Some(json!({ "deviceId": "phone", "ops": [op] })),
    )
    .await;
    assert_eq!((status, &pushed["rejected"]), (StatusCode::OK, &json!([])));
    let (seen, mut cursor) = pull(&f, &phone, 0).await;

    // The web sees it, on the list and on the track.
    let (_, body) = call(&f, &web, "GET", "/api/v1/music/likes", None).await;
    assert_eq!(liked_ids(&body), [1]);
    assert_eq!(body["tracks"][0]["liked_at"], 1);
    assert_eq!(body["tracks"][0]["track"]["liked"], true);
    let (_, list) = call(&f, &web, "GET", "/api/v1/music/tracks", None).await;
    let marks: Vec<(&Value, &Value)> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (&t["id"], &t["liked"]))
        .collect();
    assert_eq!(
        marks,
        [(&json!(1), &json!(true)), (&json!(2), &json!(false))]
    );

    // The web likes track 2: the phone's next pull carries a row it applies.
    let (status, _) = call(&f, &web, "PUT", "/api/v1/music/tracks/2/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (new, next) = pull(&f, &phone, cursor).await;
    cursor = next;
    assert_eq!(new.len(), 1);
    let row = applies(&new[0]);
    assert_eq!(
        (row.jellyfin_id.as_str(), row.content_key.as_str()),
        ("2", "bjork|post|hyperballad|321")
    );
    assert!(row.liked && !row.deleted);
    assert!(row.origin_device.starts_with("token-"));
    let mut all = seen.clone();
    all.extend(new);
    assert_eq!(liked_on_device(&all), ["1", "2"]);

    // The web unlikes track 1: the phone's own row is deleted, not a second one made.
    let (status, _) = call(&f, &web, "DELETE", "/api/v1/music/tracks/1/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (new, _) = pull(&f, &phone, cursor).await;
    let row = applies(&new[0]);
    assert_eq!(
        (new[0]["operation"].as_str(), row.id.as_str()),
        (Some("delete"), "PHONE-ROW")
    );
    all.extend(new);
    assert_eq!(liked_on_device(&all), ["2"]);
    let (_, body) = call(&f, &web, "GET", "/api/v1/music/likes", None).await;
    assert_eq!(liked_ids(&body), [2]);

    // Someone else sees none of it.
    let bob = session(&f, "bob").await;
    let (_, body) = call(&f, &bob, "GET", "/api/v1/music/likes", None).await;
    assert_eq!(body, json!({ "tracks": [], "albums": [] }));
    assert!(pull(&f, &bob, 0).await.0.is_empty());
}

#[tokio::test]
async fn a_like_of_a_track_gone_from_the_library_keeps_its_key() {
    let f = fixture().await;
    let web = session(&f, "alice").await;
    call(&f, &web, "PUT", "/api/v1/music/tracks/2/like", None).await;
    sqlx::query("UPDATE music_tracks SET missing_since = 5 WHERE id = 2")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, body) = call(&f, &web, "GET", "/api/v1/music/likes", None).await;
    assert_eq!(body["tracks"][0]["track"], Value::Null);
    assert_eq!(
        body["tracks"][0]["content_key"],
        "bjork|post|hyperballad|321"
    );
    let (status, _) = call(&f, &web, "PUT", "/api/v1/music/tracks/1/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(&f, &web, "PUT", "/api/v1/music/tracks/99/like", None).await;
    assert_eq!(
        (status, &body["error"]),
        (StatusCode::NOT_FOUND, &json!("no such track"))
    );
    // Unliking what was never liked is nothing to do.
    let (status, _) = call(&f, &web, "DELETE", "/api/v1/music/tracks/99/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn an_album_like_reaches_the_device_and_shows_on_the_web() {
    let f = fixture().await;
    let phone = session(&f, "alice").await;
    let web = session(&f, "alice").await;
    let (status, _) = call(&f, &web, "PUT", "/api/v1/music/albums/1/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The phone pulls a row of its own entity, naming the album as the apps do.
    let (ops, cursor) = pull(&f, &phone, 0).await;
    assert_eq!(ops.len(), 1);
    assert_eq!(
        (&ops[0]["entity"], &ops[0]["operation"]),
        (&json!("album_likes"), &json!("upsert"))
    );
    let row = &ops[0]["payload"];
    assert_eq!(
        (
            &row["albumId"],
            &row["contentKey"],
            &row["liked"],
            &row["deleted"]
        ),
        (
            &json!("1"),
            &json!("bjork|post"),
            &json!(true),
            &json!(false)
        )
    );
    assert_eq!(row["id"], ops[0]["entityId"]);
    assert!(row.get("jellyfinId").is_none());

    // The web sees it on the list and on the album, and no track is liked by it.
    let (_, body) = call(&f, &web, "GET", "/api/v1/music/likes", None).await;
    assert_eq!(body["tracks"], json!([]));
    assert_eq!(body["albums"][0]["album"]["id"], 1);
    assert_eq!(body["albums"][0]["content_key"], "bjork|post");
    let (_, album) = call(&f, &web, "GET", "/api/v1/music/albums/1", None).await;
    assert_eq!(album["liked"], true);
    assert!(
        album["track_list"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["liked"] == false)
    );

    // Unliking deletes the same row.
    let (status, _) = call(&f, &web, "DELETE", "/api/v1/music/albums/1/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (ops, _) = pull(&f, &phone, cursor).await;
    assert_eq!(
        (
            &ops[0]["operation"],
            &ops[0]["entityId"],
            &ops[0]["payload"]["deleted"]
        ),
        (&json!("delete"), &row["id"], &json!(true))
    );
    let (_, album) = call(&f, &web, "GET", "/api/v1/music/albums/1", None).await;
    assert_eq!(album["liked"], false);

    let (status, body) = call(&f, &web, "PUT", "/api/v1/music/albums/99/like", None).await;
    assert_eq!(
        (status, &body["error"]),
        (StatusCode::NOT_FOUND, &json!("no such album"))
    );
}

#[tokio::test]
async fn a_device_album_like_is_accepted_and_shown() {
    let f = fixture().await;
    let phone = session(&f, "alice").await;
    let op = json!({ "opId": "op-1", "entity": "album_likes", "entityId": "A", "operation": "upsert",
        "payload": { "id": "A", "albumId": "1", "contentKey": "bjork|post", "liked": true,
                     "updatedAt": 2000, "deleted": false, "originDevice": "phone" },
        "createdAt": 2000 });
    let (status, pushed) = call(
        &f,
        &phone,
        "POST",
        "/aoide/sync/push",
        Some(json!({ "deviceId": "phone", "ops": [op] })),
    )
    .await;
    assert_eq!((status, &pushed["rejected"]), (StatusCode::OK, &json!([])));
    let (_, body) = call(&f, &phone, "GET", "/api/v1/music/likes", None).await;
    assert_eq!(body["albums"][0]["album"]["id"], 1);
    assert_eq!(body["albums"][0]["liked_at"], 2);
}
