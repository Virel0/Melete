//! The aoide-sidecar's playlist artwork, collaborative playlists and queue handoff, at its
//! contract's paths and fields; plus the household's user names for picking someone to share
//! with.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_music_sync};
use crate::aoide::library::{self, GRACE_DAYS, IMAGE_TYPES, MAX_IMAGE_BYTES};
use crate::db::now_ms;

/// Playlist artwork, shares, the queue and the user directory, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route(
            "/aoide/images/{hash}",
            put(put_image)
                .get(get_image)
                .head(head_image)
                .layer(DefaultBodyLimit::max(MAX_IMAGE_BYTES)),
        )
        .route("/aoide/images/orphans", get(orphans))
        .route("/aoide/images/orphans/reclaim", post(reclaim))
        .route("/aoide/shares", get(list_shares).post(share))
        .route("/aoide/shares/{playlist}/{grantee}", delete(revoke))
        .route("/aoide/queue", get(queue))
        .route("/api/v1/users/directory", get(directory))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_music_sync))
        .with_state(pool)
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

fn bad_hash() -> ApiError {
    bad("the path segment must be a lowercase hex SHA-256")
}

// ---------- Artwork ----------

/// Stores an image under the SHA-256 of its bytes, which must be the one named: the check is
/// what makes a hash every device trusts mean those bytes.
async fn put_image(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(hash): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    if !library::is_hash(&hash) {
        return Err(bad_hash());
    }
    let mime = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !IMAGE_TYPES.contains(&mime.as_str()) {
        return Err(bad(format!(
            "Content-Type must be one of: {}",
            IMAGE_TYPES.join(", ")
        )));
    }
    if body.is_empty() {
        return Err(bad("no image bytes were sent"));
    }
    let actual = library::sha256_hex(&body);
    if actual != hash {
        return Err(bad(format!("the body hashes to {actual}, not {hash}")));
    }
    library::store_image(&pool, current.user.id, &hash, &mime, &body, now_ms()).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// An image the user stored, or someone they share a playlist with did.
async fn get_image(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    if !library::is_hash(&hash) {
        return Err(bad_hash());
    }
    let Some((bytes, mime)) = library::image_seen_by(&pool, current.user.id, &hash).await? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such image"));
    };
    Ok(image_response(&hash, mime, bytes))
}

/// "Do I need to upload this?": only the user's own store answers, so an image that's only
/// someone else's is still uploaded, and stays theirs after a share ends.
async fn head_image(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(hash): Path<String>,
) -> Result<Response, ApiError> {
    if !library::is_hash(&hash) {
        return Err(bad_hash());
    }
    let Some((bytes, mime)) = library::image(&pool, current.user.id, &hash).await? else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such image"));
    };
    // The body goes unsent for HEAD; its length is still the one GET would give.
    Ok(image_response(&hash, mime, bytes))
}

fn image_response(hash: &str, mime: String, bytes: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime),
            // The bytes behind a hash never change; private, since it needs a signed-in user.
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_owned(),
            ),
            (header::ETAG, format!("\"{hash}\"")),
        ],
        bytes,
    )
        .into_response()
}

fn orphans_json(report: &library::Orphans) -> Value {
    json!({
        "totalBlobs": report.total_blobs,
        "totalBytes": report.total_bytes,
        "orphans": report.orphans.iter().map(|o| json!({
            "imageHash": o.hash,
            "sizeBytes": o.size,
            "ageDays": o.age_days,
            "reclaimable": o.reclaimable,
        })).collect::<Vec<_>>(),
        "orphanBytes": report.orphan_bytes,
        "graceDays": report.grace_days,
        "reclaimed": report.reclaimed,
        "reclaimedBytes": report.reclaimed_bytes,
    })
}

/// Images no playlist names now; deletes nothing.
async fn orphans(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let report = library::orphans(&pool, current.user.id, GRACE_DAYS, false, now_ms()).await?;
    Ok(Json(orphans_json(&report)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReclaimQuery {
    /// Can only make it more cautious than 30 days.
    older_than_days: Option<i64>,
}

async fn reclaim(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ReclaimQuery>,
) -> Result<Json<Value>, ApiError> {
    let days = query.older_than_days.unwrap_or(GRACE_DAYS);
    let report = library::orphans(&pool, current.user.id, days, true, now_ms()).await?;
    if report.reclaimed > 0 {
        tracing::info!(
            user = current.user.id,
            reclaimed = report.reclaimed,
            "reclaimed unused playlist artwork"
        );
    }
    Ok(Json(orphans_json(&report)))
}

// ---------- Collaborative playlists ----------

async fn list_shares(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let shares = library::shares(&pool, current.user.id).await?;
    Ok(Json(json!(
        shares
            .iter()
            .map(|s| json!({
                "playlistId": s.playlist_id,
                "ownerUserId": s.owner_user_id.to_string(),
                "ownerName": s.owner_name,
                "granteeUserId": s.grantee_user_id.to_string(),
                "granteeName": s.grantee_name,
                "canEdit": s.can_edit,
                "isOwner": s.owner_user_id == current.user.id,
                "createdAt": s.created_at,
            }))
            .collect::<Vec<_>>()
    )))
}

/// A user id as Melete has it: a number, or the same as a string (ids travel as strings in
/// the contract).
fn user_id(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

async fn user_exists(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = ?)")
        .bind(id)
        .fetch_one(pool)
        .await
}

/// Only the owner shares; a collaborator then pushes edits to it and both pull each other's.
async fn share(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Value>,
) -> Result<StatusCode, ApiError> {
    let Some(playlist) = body["playlistId"].as_str().filter(|p| !p.trim().is_empty()) else {
        return Err(bad("playlistId is required"));
    };
    let grantee = match user_id(&body["granteeUserId"]) {
        Some(id) if user_exists(&pool, id).await? => id,
        _ => return Err(bad("granteeUserId must be an existing user on this server")),
    };
    if grantee == current.user.id {
        return Err(bad("you can't share a playlist with yourself"));
    }
    match library::owner(&pool, playlist).await? {
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "that playlist hasn't been synced to the server yet",
        )),
        Some(owner) if owner != current.user.id => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only the owner can share a playlist",
        )),
        Some(_) => {
            let can_edit = body["canEdit"].as_bool().unwrap_or(true);
            library::share(&pool, playlist, grantee, can_edit, now_ms()).await?;
            tracing::info!(
                playlist,
                owner = current.user.id,
                grantee,
                "shared a playlist"
            );
            Ok(StatusCode::NO_CONTENT)
        }
    }
}

/// Either side can end a share.
async fn revoke(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path((playlist, grantee)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, "no such share");
    let grantee: i64 = grantee.trim().parse().map_err(|_| not_found())?;
    let Some(owner) = library::owner(&pool, &playlist).await? else {
        return Err(not_found());
    };
    if owner != current.user.id && grantee != current.user.id {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only the owner or the collaborator can end a share",
        ));
    }
    if library::revoke(&pool, &playlist, grantee).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

// ---------- Queue handoff ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueueQuery {
    /// This device's curation-store id, to mark its own queue; the session's when absent.
    device_id: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceQueue {
    device_id: String,
    is_current_device: bool,
    age_seconds: i64,
    received_at: i64,
    updated_at: i64,
    payload: Box<RawValue>,
}

/// What each of the user's devices is playing, most recently pushed first: the one to offer
/// is normally the first that isn't this device. Ages are by the server's clock.
async fn queue(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<QueueQuery>,
) -> Result<Json<Vec<DeviceQueue>>, ApiError> {
    let this_device = query
        .device_id
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| format!("token-{}", current.token_id));
    let now = now_ms();
    let mut out = Vec::new();
    for q in library::queues(&pool, current.user.id).await? {
        out.push(DeviceQueue {
            is_current_device: q.device_id == this_device,
            device_id: q.entity_id,
            age_seconds: ((now - q.received_at) / 1000).max(0),
            received_at: q.received_at,
            updated_at: q.created_at,
            payload: RawValue::from_string(q.payload)?,
        });
    }
    Ok(Json(out))
}

// ---------- The household's names ----------

/// Every user's id and name, for any signed-in user (sharing a playlist means picking
/// someone). Nothing else about them: not who's an admin, not their sessions.
async fn directory(State(pool): State<SqlitePool>) -> Result<Json<Value>, ApiError> {
    let users: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, username FROM users ORDER BY username COLLATE NOCASE")
            .fetch_all(&pool)
            .await?;
    Ok(Json(json!(
        users
            .iter()
            .map(|(id, name)| json!({ "id": id, "username": name }))
            .collect::<Vec<_>>()
    )))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::aoide::sync;

    /// Three users (owner, friend, stranger) with tokens, none an admin.
    async fn fixture() -> (Router, SqlitePool, Vec<String>) {
        let pool = crate::db::open_in_memory().await;
        let mut tokens = Vec::new();
        for name in ["owner", "friend", "stranger"] {
            let user = crate::auth::create_user(&pool, name, "a-password".into(), false)
                .await
                .unwrap()
                .unwrap();
            let (_, token) =
                crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                    .await
                    .unwrap();
            tokens.push(token);
        }
        (routes(pool.clone()), pool, tokens)
    }

    async fn send(
        app: &Router,
        token: &str,
        method: &str,
        uri: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let (status, headers) = (response.status(), response.headers().clone());
        let bytes = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, bytes)
    }

    /// The response's fields are exactly the spec's.
    fn assert_shape(schema: &str, value: &Value) {
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
        documented.sort();
        actual.sort();
        assert_eq!(actual, documented, "{schema}");
    }

    fn json_of(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap_or(Value::Null)
    }

    async fn push(pool: &SqlitePool, user: i64, json: String) -> sync::Pushed {
        let op = sync::validate(&RawValue::from_string(json).unwrap()).unwrap();
        sync::push(pool, user, "dev", vec![op], 1).await.unwrap()
    }

    #[tokio::test]
    async fn stores_artwork_by_its_hash() {
        let (app, _, tokens) = fixture().await;
        let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let hash = library::sha256_hex(&png);
        let uri = format!("/aoide/images/{hash}");
        assert_eq!(
            send(&app, &tokens[0], "HEAD", &uri, None, vec![]).await.0,
            StatusCode::NOT_FOUND
        );
        let (status, _, _) = send(
            &app,
            &tokens[0],
            "PUT",
            &uri,
            Some("image/png"),
            png.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            send(
                &app,
                &tokens[0],
                "PUT",
                &uri,
                Some("image/png"),
                png.clone()
            )
            .await
            .0,
            StatusCode::NO_CONTENT,
            "again is a no-op"
        );
        let (status, headers, _) = send(&app, &tokens[0], "HEAD", &uri, None, vec![]).await;
        assert_eq!(status, StatusCode::OK);
        let (_, headers_get, bytes) = send(&app, &tokens[0], "GET", &uri, None, vec![]).await;
        assert_eq!(bytes, png);
        assert_eq!(headers_get[header::CONTENT_TYPE], "image/png");
        assert!(
            headers[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
        assert_eq!(headers_get[header::ETAG], format!("\"{hash}\"").as_str());
        // Another user's store is their own.
        assert_eq!(
            send(&app, &tokens[1], "GET", &uri, None, vec![]).await.0,
            StatusCode::NOT_FOUND
        );

        let other = library::sha256_hex(b"other");
        for (path, content_type, body, status) in [
            (
                format!("/aoide/images/{other}"),
                Some("image/png"),
                png.clone(),
                StatusCode::BAD_REQUEST,
            ),
            (
                uri.clone(),
                Some("image/gif"),
                png.clone(),
                StatusCode::BAD_REQUEST,
            ),
            (
                uri.clone(),
                Some("image/png"),
                vec![],
                StatusCode::BAD_REQUEST,
            ),
            (
                uri.to_uppercase()
                    .replace("/AOIDE/IMAGES/", "/aoide/images/"),
                Some("image/png"),
                png.clone(),
                StatusCode::BAD_REQUEST,
            ),
            (
                uri.clone(),
                Some("image/png"),
                vec![0; MAX_IMAGE_BYTES + 1],
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            assert_eq!(
                send(&app, &tokens[0], "PUT", &path, content_type, body)
                    .await
                    .0,
                status,
                "{path} {content_type:?}"
            );
        }

        // Just uploaded, nothing names it yet: listed, not reclaimable.
        let (_, _, body) = send(
            &app,
            &tokens[0],
            "GET",
            "/aoide/images/orphans",
            None,
            vec![],
        )
        .await;
        let report = json_of(&body);
        assert_shape("AoideOrphans", &report);
        assert_shape("AoideOrphan", &report["orphans"][0]);
        assert_eq!(
            report["orphans"][0],
            json!({ "imageHash": hash, "sizeBytes": png.len(), "ageDays": 0, "reclaimable": false })
        );
        let (_, _, body) = send(
            &app,
            &tokens[0],
            "POST",
            "/aoide/images/orphans/reclaim?olderThanDays=0",
            None,
            vec![],
        )
        .await;
        assert_eq!(
            (
                json_of(&body)["reclaimed"].clone(),
                json_of(&body)["graceDays"].clone()
            ),
            (json!(0), json!(30))
        );
    }

    #[tokio::test]
    async fn a_shared_playlists_artwork_is_seen_by_everyone_on_it() {
        let (app, pool, tokens) = fixture().await;
        let (owner, friend, stranger) = (&tokens[0], &tokens[1], &tokens[2]);
        push(&pool, 1, r#"{"opId":"p","entity":"playlists","entityId":"p1","operation":"upsert","payload":{},"createdAt":1}"#.into()).await;
        let shared = send(
            &app,
            owner,
            "POST",
            "/aoide/shares",
            Some("application/json"),
            serde_json::to_vec(&json!({ "playlistId": "p1", "granteeUserId": 2 })).unwrap(),
        )
        .await;
        assert_eq!(shared.0, StatusCode::NO_CONTENT);
        let upload = |token: &String, bytes: &'static [u8]| {
            let (app, token) = (app.clone(), token.clone());
            async move {
                let uri = format!("/aoide/images/{}", library::sha256_hex(bytes));
                let put = send(&app, &token, "PUT", &uri, Some("image/png"), bytes.to_vec()).await;
                assert_eq!(put.0, StatusCode::NO_CONTENT);
                uri
            }
        };
        let cover = upload(owner, b"owner's cover").await;
        let (status, _, bytes) = send(&app, friend, "GET", &cover, None, vec![]).await;
        assert_eq!((status, bytes), (StatusCode::OK, b"owner's cover".to_vec()));
        assert_eq!(
            send(&app, stranger, "GET", &cover, None, vec![]).await.0,
            StatusCode::NOT_FOUND,
            "nobody else's"
        );
        assert_eq!(
            send(&app, friend, "HEAD", &cover, None, vec![]).await.0,
            StatusCode::NOT_FOUND,
            "it's still the friend's to upload, should they want it kept"
        );
        // A collaborator's upload is seen by the owner too.
        let theirs = upload(friend, b"friend's cover").await;
        assert_eq!(
            send(&app, owner, "GET", &theirs, None, vec![]).await.0,
            StatusCode::OK
        );

        // Once the share ends, so does seeing the other's store.
        let ended = send(&app, owner, "DELETE", "/aoide/shares/p1/2", None, vec![]).await;
        assert_eq!(ended.0, StatusCode::NO_CONTENT);
        assert_eq!(
            send(&app, friend, "GET", &cover, None, vec![]).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(&app, owner, "GET", &theirs, None, vec![]).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn owners_share_playlists_and_either_side_ends_it() {
        let (app, pool, tokens) = fixture().await;
        push(&pool, 1, r#"{"opId":"p","entity":"playlists","entityId":"p1","operation":"upsert","payload":{},"createdAt":1}"#.into()).await;
        let share_body = |grantee: Value| {
            serde_json::to_vec(&json!({ "playlistId": "p1", "granteeUserId": grantee })).unwrap()
        };
        let json_type = Some("application/json");
        // Only the owner, with someone real who isn't themselves, for a playlist that exists.
        assert_eq!(
            send(
                &app,
                &tokens[1],
                "POST",
                "/aoide/shares",
                json_type,
                share_body(json!(3))
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(
                &app,
                &tokens[0],
                "POST",
                "/aoide/shares",
                json_type,
                share_body(json!(1))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            send(
                &app,
                &tokens[0],
                "POST",
                "/aoide/shares",
                json_type,
                share_body(json!(99))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let unknown =
            serde_json::to_vec(&json!({ "playlistId": "nope", "granteeUserId": 2 })).unwrap();
        assert_eq!(
            send(
                &app,
                &tokens[0],
                "POST",
                "/aoide/shares",
                json_type,
                unknown
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let (status, _, _) = send(
            &app,
            &tokens[0],
            "POST",
            "/aoide/shares",
            json_type,
            share_body(json!("2")),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (_, _, body) = send(&app, &tokens[1], "GET", "/aoide/shares", None, vec![]).await;
        let listed = json_of(&body);
        assert_shape("AoideShare", &listed[0]);
        assert_eq!(listed[0]["ownerName"], "owner");
        assert_eq!(
            (
                listed[0]["granteeUserId"].clone(),
                listed[0]["isOwner"].clone(),
                listed[0]["canEdit"].clone()
            ),
            (json!("2"), json!(false), json!(true))
        );
        // The friend's edit lands; the stranger's doesn't.
        let item = |id: &str| {
            format!(
                r#"{{"opId":"{id}","entity":"playlist_items","entityId":"{id}","operation":"upsert","payload":{{"playlist_id":"p1"}},"createdAt":1}}"#
            )
        };
        assert!(push(&pool, 2, item("f")).await.denied.is_empty());
        assert_eq!(push(&pool, 3, item("s")).await.denied.len(), 1);

        assert_eq!(
            send(
                &app,
                &tokens[2],
                "DELETE",
                "/aoide/shares/p1/2",
                None,
                vec![]
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            send(
                &app,
                &tokens[1],
                "DELETE",
                "/aoide/shares/p1/2",
                None,
                vec![]
            )
            .await
            .0,
            StatusCode::NO_CONTENT,
            "the collaborator can leave"
        );
        assert_eq!(
            send(
                &app,
                &tokens[0],
                "DELETE",
                "/aoide/shares/p1/2",
                None,
                vec![]
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(
                &app,
                &tokens[0],
                "DELETE",
                "/aoide/shares/nope/2",
                None,
                vec![]
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn shows_each_devices_latest_queue() {
        let (app, pool, tokens) = fixture().await;
        let queue = |op: &str, device: &str, at: i64| {
            format!(
                r#"{{"opId":"{op}","entity":"queue_state","entityId":"{device}","operation":"upsert","payload":{{"deviceName":"{device}","position":{at}}},"createdAt":{at}}}"#
            )
        };
        for (op, device, at) in [("a", "phone", 1), ("b", "mac", 2), ("c", "phone", 3)] {
            let parsed =
                sync::validate(&RawValue::from_string(queue(op, device, at)).unwrap()).unwrap();
            sync::push(&pool, 1, device, vec![parsed], now_ms())
                .await
                .unwrap();
        }
        let (status, _, body) = send(
            &app,
            &tokens[0],
            "GET",
            "/aoide/queue?deviceId=mac",
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let queues = json_of(&body);
        assert_shape("AoideDeviceQueue", &queues[0]);
        let devices: Vec<(&str, bool, i64)> = queues
            .as_array()
            .unwrap()
            .iter()
            .map(|q| {
                (
                    q["deviceId"].as_str().unwrap(),
                    q["isCurrentDevice"].as_bool().unwrap(),
                    q["payload"]["position"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(devices, [("phone", false, 3), ("mac", true, 2)]);
        assert_eq!(queues[0]["ageSeconds"], 0);
        assert_eq!(queues[0]["updatedAt"], 3);
        let (_, _, body) = send(&app, &tokens[1], "GET", "/aoide/queue", None, vec![]).await;
        assert_eq!(
            json_of(&body),
            json!([]),
            "another user's queues aren't theirs"
        );
    }

    #[tokio::test]
    async fn any_user_sees_the_households_names_only() {
        let (app, _, tokens) = fixture().await;
        let (status, _, body) = send(
            &app,
            &tokens[2],
            "GET",
            "/api/v1/users/directory",
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            json_of(&body),
            json!([{ "id": 2, "username": "friend" }, { "id": 1, "username": "owner" }, { "id": 3, "username": "stranger" }])
        );
    }
}
