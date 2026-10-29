//! The aoide-sidecar's sync endpoints, at the paths, fields (camelCase) and
//! status codes of its contract (aoide-sidecar's docs/client-integration.md): push, pull,
//! status, and play-history retention. Signed in as for every other route; errors have
//! Melete's `{"error": …}` shape, and a store that can't be used is a 503, so clients keep
//! their ops queued and retry.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_music_sync};
use crate::aoide::sync::{
    self, DEFAULT_PULL, ENTITIES, MAX_OPS_PER_PUSH, MAX_PULL, RETENTION_DAYS,
};
use crate::db::now_ms;

/// A push of a thousand ops of up to 256 KB each would be far larger; real batches aren't.
const MAX_PUSH_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct SyncState {
    pool: SqlitePool,
    data_dir: PathBuf,
}

/// The sidecar's sync and retention routes, for everyone signed in. Pushed state is kept under
/// `data_dir`.
pub fn routes(pool: SqlitePool, data_dir: PathBuf) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route(
            "/aoide/sync/push",
            post(push).layer(DefaultBodyLimit::max(MAX_PUSH_BYTES)),
        )
        .route("/aoide/sync/pull", get(pull))
        .route("/aoide/sync/status", get(status))
        .route("/aoide/retention", get(retention))
        .route("/aoide/retention/prune", post(prune))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_music_sync))
        .with_state(SyncState { pool, data_dir })
}

/// A device's own name for itself is the curation store's; failing that, its session.
fn session_device(current: &CurrentUser) -> String {
    format!("token-{}", current.token_id)
}

fn unavailable(err: sqlx::Error) -> ApiError {
    tracing::error!(error = %err, "aoide sync storage failed");
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        format!("sync storage unavailable: {err}"),
    )
}

#[derive(Deserialize)]
struct PushBody<'a> {
    #[serde(rename = "deviceId")]
    device_id: Option<String>,
    #[serde(borrow, default)]
    ops: Vec<&'a RawValue>,
}

/// Appends a batch to the user's log. Each op is checked on its own: refused ones are listed
/// with a reason and the rest still land.
async fn push(
    State(state): State<SyncState>,
    Extension(current): Extension<CurrentUser>,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let malformed = |detail: String| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("malformed request body: {detail}"),
        )
    };
    let text = std::str::from_utf8(&body).map_err(|err| malformed(err.to_string()))?;
    // Parsed as a value first: that stops at 128 levels, the contract's ceiling for a whole
    // request (reading raw values skips nesting without counting it).
    serde_json::from_str::<Value>(text).map_err(|err| malformed(err.to_string()))?;
    let request: PushBody = serde_json::from_str(text).map_err(|err| malformed(err.to_string()))?;
    if request.ops.len() > MAX_OPS_PER_PUSH {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "batch too large: {} ops is over the limit of {MAX_OPS_PER_PUSH}; split the batch",
                request.ops.len()
            ),
        ));
    }
    let device = request
        .device_id
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| session_device(&current));
    let mut valid = Vec::with_capacity(request.ops.len());
    let mut accepted = Vec::with_capacity(request.ops.len());
    let mut rejected = Vec::new();
    for raw in request.ops {
        match sync::validate(raw) {
            Ok(op) => {
                accepted.push(op.op_id.clone());
                valid.push(op);
            }
            Err((op_id, reason)) => rejected.push(json!({ "opId": op_id, "reason": reason })),
        }
    }
    let pushed = sync::push(&state.pool, current.user.id, &device, valid, now_ms())
        .await
        .map_err(unavailable)?;
    // Listens reach the play history now; the ops are stored either way.
    crate::plays::catch_up_or_log(&state.pool).await;
    for (op_id, reason) in pushed.denied {
        accepted.retain(|id| *id != op_id);
        rejected.push(json!({ "opId": op_id, "reason": reason }));
    }
    if !rejected.is_empty() {
        tracing::warn!(
            user = current.user.id,
            device,
            count = rejected.len(),
            "rejected aoide ops"
        );
    }
    Ok(Json(json!({
        "accepted": accepted,
        "rejected": rejected,
        "cursor": pushed.cursor,
    })))
}

#[derive(Deserialize)]
struct PullQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

/// Serialised straight to the response, so each payload goes out as it was stored (through a
/// JSON value its keys would be reordered and huge numbers rounded).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PulledOp {
    seq: i64,
    op_id: String,
    device_id: String,
    author_user_id: String,
    entity: String,
    entity_id: String,
    operation: String,
    payload: Box<RawValue>,
    created_at: i64,
    received_at: i64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PullResponse {
    ops: Vec<PulledOp>,
    cursor: i64,
    has_more: bool,
}

/// Ops the device hasn't seen, in sequence order, its own included.
async fn pull(
    State(state): State<SyncState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResponse>, ApiError> {
    let since = query.since.unwrap_or(0);
    if since < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "since must be zero or greater",
        ));
    }
    let limit = query.limit.unwrap_or(DEFAULT_PULL).clamp(1, MAX_PULL);
    let (ops, cursor, has_more) = sync::pull(&state.pool, current.user.id, since, limit)
        .await
        .map_err(unavailable)?;
    // How far this device has read; retention needs it.
    sync::record_cursor(
        &state.pool,
        current.user.id,
        &session_device(&current),
        cursor,
        now_ms(),
    )
    .await
    .map_err(unavailable)?;
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        out.push(PulledOp {
            seq: op.seq,
            op_id: op.op_id,
            device_id: op.device_id,
            author_user_id: op.user_id.to_string(),
            entity: op.entity,
            entity_id: op.entity_id,
            operation: op.operation,
            payload: RawValue::from_string(op.payload)?,
            created_at: op.created_at,
            received_at: op.received_at,
        });
    }
    Ok(Json(PullResponse {
        ops: out,
        cursor,
        has_more,
    }))
}

/// The store's view of itself, including whether a write actually succeeds, and the entities
/// push accepts (clients hold ops of an entity not listed rather than pushing them).
async fn status(
    State(state): State<SyncState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let database = state.data_dir.join("melete.db");
    let probe = state.data_dir.join(".aoide-write-probe");
    let directory_writable =
        std::fs::write(&probe, b"").is_ok() && std::fs::remove_file(&probe).is_ok();
    let checked: Result<(i64, String, i64, i64), sqlx::Error> = async {
        let schema: i64 =
            sqlx::query_scalar("SELECT coalesce(max(version), 0) FROM _sqlx_migrations")
                .fetch_one(&state.pool)
                .await?;
        let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&state.pool)
            .await?;
        let cursor = sync::head(&state.pool, current.user.id).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_ops WHERE user_id = ?")
            .bind(current.user.id)
            .fetch_one(&state.pool)
            .await?;
        // A real write that changes nothing: the same path a push takes.
        let mut tx = state.pool.begin().await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA user_version = {version}"
        )))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((schema, journal, cursor, count))
    }
    .await;
    let (schema, journal, cursor, count, writable, error) = match checked {
        Ok((schema, journal, cursor, count)) => (schema, Some(journal), cursor, count, true, None),
        Err(err) => (-1, None, 0, 0, false, Some(err.to_string())),
    };
    if let Some(error) = &error {
        tracing::error!(error, "aoide sync storage isn't writable");
    }
    Ok(Json(json!({
        "pluginVersion": concat!("melete/", env!("CARGO_PKG_VERSION")),
        "acceptedEntities": ENTITIES,
        "databasePath": database.to_string_lossy(),
        "schemaVersion": schema,
        "writable": writable,
        "journalMode": journal,
        "directoryWritable": directory_writable,
        "cursor": cursor,
        "opCount": count,
        "error": error,
    })))
}

fn retention_json(report: &sync::Retention) -> Value {
    json!({
        "opsByEntity": report
            .ops_by_entity
            .iter()
            .map(|(entity, count)| (entity.clone(), json!(count)))
            .collect::<serde_json::Map<String, Value>>(),
        "cursor": report.cursor,
        "devices": report.devices.iter().map(|d| json!({
            "deviceId": d.device_id,
            "cursor": d.cursor,
            "lastSeenDays": d.last_seen_days,
            "active": d.active,
        })).collect::<Vec<_>>(),
        "safeCursor": report.safe_cursor,
        "retentionDays": report.retention_days,
        "prunablePlayEvents": report.prunable_play_events,
        "pruned": report.pruned,
    })
}

/// What retention would trim: play events older than 90 days every recent device has read.
async fn retention(
    State(state): State<SyncState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let report = sync::retention(
        &state.pool,
        current.user.id,
        RETENTION_DAYS,
        false,
        now_ms(),
    )
    .await
    .map_err(unavailable)?;
    Ok(Json(retention_json(&report)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PruneQuery {
    /// Can only make the sweep more cautious than 90 days.
    older_than_days: Option<i64>,
}

async fn prune(
    State(state): State<SyncState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<PruneQuery>,
) -> Result<Json<Value>, ApiError> {
    let days = query.older_than_days.unwrap_or(RETENTION_DAYS);
    let report = sync::retention(&state.pool, current.user.id, days, true, now_ms())
        .await
        .map_err(unavailable)?;
    Ok(Json(retention_json(&report)))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn fixture() -> (Router, SqlitePool, String, tempfile::TempDir) {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "phone")
                .await
                .unwrap();
        let data = tempfile::tempdir().unwrap();
        (
            routes(pool.clone(), data.path().to_path_buf()),
            pool,
            token,
            data,
        )
    }

    async fn send(
        app: &Router,
        token: Option<&str>,
        method: &str,
        uri: &str,
        body: Option<String>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body)
            }
            None => Body::empty(),
        };
        let response = app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
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

    fn op(id: &str, entity: &str, payload: &str) -> String {
        format!(
            r#"{{"opId":"{id}","entity":"{entity}","entityId":"e-{id}","operation":"upsert","payload":{payload},"createdAt":1754500000000}}"#
        )
    }

    #[tokio::test]
    async fn pushes_and_pulls_with_payloads_as_sent() {
        let (app, _, token, _data) = fixture().await;
        let body = format!(
            r#"{{"deviceId":"phone-1","ops":[{},{},{},{}]}}"#,
            op(
                "a",
                "play_events",
                r#"{"trackId":"42","completed":1,"big":123456789012345678901234567890,"z":1,"a":2}"#
            ),
            op("b", "songs", "{}"),
            op("c", "likes", r#"{"liked":1}"#),
            r#"{"opId":"d","entity":"likes","entityId":"x","operation":"upsert","payload":"nope","createdAt":1}"#,
        );
        let (status, pushed) = send(
            &app,
            Some(&token),
            "POST",
            "/aoide/sync/push",
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{pushed}");
        assert_eq!(pushed["accepted"], json!(["a", "c"]));
        assert_eq!(pushed["rejected"][0]["opId"], "b");
        assert!(
            pushed["rejected"][1]["reason"]
                .as_str()
                .unwrap()
                .contains("JSON object")
        );
        assert_eq!(pushed["cursor"], 2);
        assert_shape("AoidePushResponse", &pushed);
        assert_shape("AoideRejectedOp", &pushed["rejected"][0]);
        // Pushing again is accepted and changes nothing.
        let (_, again) = send(&app, Some(&token), "POST", "/aoide/sync/push", Some(body)).await;
        assert_eq!(
            (again["accepted"].clone(), again["cursor"].clone()),
            (json!(["a", "c"]), json!(2))
        );

        let response = app
            .clone()
            .oneshot(
                Request::get("/aoide/sync/pull?since=0&limit=1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let text = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        // The payload comes back byte for byte: key order, and a number too big for 64 bits.
        assert!(
            text.contains(
                r#""payload":{"trackId":"42","completed":1,"big":123456789012345678901234567890,"z":1,"a":2}"#
            ),
            "{text}"
        );
        let page: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            (page["cursor"].clone(), page["hasMore"].clone()),
            (json!(1), json!(true))
        );
        let first = &page["ops"][0];
        assert_eq!(first["deviceId"], "phone-1");
        assert_eq!(first["authorUserId"], "1");
        assert!(first["receivedAt"].as_i64().unwrap() > 1_700_000_000_000);
        assert_shape("AoidePullResponse", &page);
        assert_shape("AoideOp", first);
        let (_, rest) = send(&app, Some(&token), "GET", "/aoide/sync/pull?since=1", None).await;
        assert_eq!(
            (
                rest["ops"].as_array().unwrap().len(),
                rest["hasMore"].clone()
            ),
            (1, json!(false))
        );
    }

    #[tokio::test]
    async fn refuses_broken_batches_whole_and_needs_a_user() {
        let (app, _, token, _data) = fixture().await;
        let deep = format!(r#"{{"ops":[{}1{}]}}"#, "[".repeat(130), "]".repeat(130));
        let (status, body) = send(&app, Some(&token), "POST", "/aoide/sync/push", Some(deep)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .starts_with("malformed request body")
        );
        let many = format!(
            r#"{{"ops":[{}]}}"#,
            vec![op("x", "likes", "{}"); MAX_OPS_PER_PUSH + 1].join(",")
        );
        let (status, body) = send(&app, Some(&token), "POST", "/aoide/sync/push", Some(many)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("split the batch"));
        assert_eq!(
            send(&app, Some(&token), "GET", "/aoide/sync/pull?since=-1", None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        for (method, uri) in [
            ("GET", "/aoide/sync/status"),
            ("POST", "/aoide/sync/push"),
            ("GET", "/aoide/retention"),
        ] {
            assert_eq!(
                send(&app, None, method, uri, None).await.0,
                StatusCode::UNAUTHORIZED,
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn a_push_without_a_device_id_is_the_sessions() {
        let (app, pool, token, _data) = fixture().await;
        let body = format!(r#"{{"ops":[{}]}}"#, op("a", "likes", "{}"));
        send(&app, Some(&token), "POST", "/aoide/sync/push", Some(body)).await;
        let device: String = sqlx::query_scalar("SELECT device_id FROM aoide_ops")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(device.starts_with("token-"));
    }

    #[tokio::test]
    async fn reports_status_and_retention() {
        let (app, _, token, data) = fixture().await;
        let (status, body) = send(&app, Some(&token), "GET", "/aoide/sync/status", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["acceptedEntities"], json!(ENTITIES));
        assert_shape("AoideSyncStatus", &body);
        assert_eq!(body["writable"], true);
        assert_eq!(body["directoryWritable"], true);
        assert_eq!(body["error"], Value::Null);
        assert_eq!(
            body["databasePath"],
            data.path().join("melete.db").to_str().unwrap()
        );
        assert!(body["schemaVersion"].as_i64().unwrap() >= 26);
        assert!(!data.path().join(".aoide-write-probe").exists());

        send(
            &app,
            Some(&token),
            "POST",
            "/aoide/sync/push",
            Some(format!(r#"{{"ops":[{}]}}"#, op("p", "play_events", "{}"))),
        )
        .await;
        send(&app, Some(&token), "GET", "/aoide/sync/pull", None).await;
        let (_, report) = send(&app, Some(&token), "GET", "/aoide/retention", None).await;
        assert_eq!(report["opsByEntity"], json!({ "play_events": 1 }));
        assert_eq!(report["devices"][0]["cursor"], 1);
        assert_eq!(report["retentionDays"], 90);
        assert_shape("AoideRetention", &report);
        assert_shape("AoideDeviceCursor", &report["devices"][0]);
        let (status, pruned) = send(
            &app,
            Some(&token),
            "POST",
            "/aoide/retention/prune?olderThanDays=10",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (pruned["retentionDays"].clone(), pruned["pruned"].clone()),
            (json!(90), json!(0))
        );
    }
}
