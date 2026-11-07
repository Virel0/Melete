//! The web player's queue for the Aoide apps: a browser that doesn't sync publishes what it's
//! playing as its session's `queue_state` row, the row an Aoide device pushes for its own queue,
//! so the user's other devices can offer "Pick up from …" with it (`GET /aoide/queue`). A
//! session writes only its own row, in its own user's log.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::put;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::json;
use serde_json::value::RawValue;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback};
use crate::aoide::sync;
use crate::aoide::writing::{canonical_json, new_id, session_device};
use crate::db::now_ms;

/// The most track ids a queue row carries, as the apps cap theirs (`QueuePolicy.maxTracks`).
const MAX_TRACKS: usize = 500;

/// The route, for a signed-in user who may see all the music, as `GET /aoide/queue` asks.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/queue", put(publish))
        .route_layer(middleware::from_fn_with_state(
            pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_playback))
        .with_state(pool)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueBody {
    track_ids: Vec<i64>,
    #[serde(default)]
    position: i64,
    #[serde(default)]
    elapsed_ms: i64,
}

fn bad(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

/// Keeps this session's queue as its device's `queue_state` row: the tracks in play order
/// (played ones too), which one is playing, and how far into it. An empty list says nothing is
/// queued, which the apps don't offer.
async fn publish(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<QueueBody>,
) -> Result<StatusCode, ApiError> {
    if body.track_ids.len() > MAX_TRACKS {
        return Err(bad("a queue carries at most 500 tracks"));
    }
    let last = i64::try_from(body.track_ids.len()).unwrap_or(i64::MAX) - 1;
    if body.position < 0 || body.position > last.max(0) {
        return Err(bad("position must be the index of a track in track_ids"));
    }
    if body.elapsed_ms < 0 {
        return Err(bad("elapsed_ms can't be negative"));
    }
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM tokens WHERE id = ?")
        .bind(current.token_id)
        .fetch_optional(&pool)
        .await?;
    let device = session_device(current.token_id);
    let ids: Vec<String> = body.track_ids.iter().map(i64::to_string).collect();
    let now = now_ms();
    // The row as the apps write it, the ids as JSON text inside it.
    let op = json!({
        "opId": new_id().map_err(|_| anyhow::anyhow!("no randomness for an op id"))?,
        "entity": "queue_state",
        "entityId": device,
        "operation": "upsert",
        "createdAt": now,
        "payload": {
            "deviceId": device,
            "deviceName": name.unwrap_or_else(|| "Web browser".to_owned()),
            "trackIds": serde_json::to_string(&ids)?,
            "position": body.position,
            "elapsedMs": body.elapsed_ms,
            "updatedAt": now,
            "originDevice": device,
        },
    });
    let raw = RawValue::from_string(canonical_json(&op))?;
    let op = sync::validate(&raw).map_err(|(_, why)| bad(&why))?;
    sync::push(&pool, current.user.id, &device, vec![op], now).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
