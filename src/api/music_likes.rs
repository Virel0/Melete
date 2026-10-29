//! Liked tracks for apps that don't sync (the web UI): read from the caller's Aoide sync
//! log and set by writing into it what a device of theirs would, so every device sees the
//! change through its next pull and the log stays the one place likes are kept. Like the rest of
//! what's in that log, they're closed to a user who may see only some music libraries.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::tracks_by_id;
use crate::aoide::likes;
use crate::aoide::writing::{WriteError, session_device};
use crate::db::now_ms;

/// The like routes, for everyone signed in who may see every music library.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/likes", get(list))
        .route("/api/v1/music/tracks/{id}/like", put(like).delete(unlike))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// What a failed write is to the caller: a refusal is theirs to see, anything else a 500.
pub(super) fn write_failed(err: WriteError) -> ApiError {
    match err {
        WriteError::Refused(why) => ApiError::new(StatusCode::FORBIDDEN, why),
        WriteError::Database(err) => err.into(),
        WriteError::NoRandomness => anyhow::anyhow!("no randomness available").into(),
    }
}

/// The caller's liked tracks, newest like first, each with its track as the music API shows
/// it, or null when it isn't in the library now (its content key says what it was).
async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let me = current.user.id;
    let liked = likes::liked(&pool, me).await?;
    let ids: Vec<i64> = liked.iter().filter_map(|(track, _)| *track).collect();
    let tracks = tracks_by_id(&pool, &current.access, me, &ids).await?;
    let items: Vec<Value> = liked
        .iter()
        .map(|(track, row)| {
            json!({
                "liked_at": row.updated_at.div_euclid(1000),
                "content_key": row.content_key,
                "track": track.and_then(|t| tracks.get(&t)).cloned(),
            })
        })
        .collect();
    Ok(Json(json!({ "tracks": items })))
}

async fn like(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    set(&pool, &current, id, true).await
}

async fn unlike(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    set(&pool, &current, id, false).await
}

async fn set(
    pool: &SqlitePool,
    current: &CurrentUser,
    track: i64,
    like: bool,
) -> Result<StatusCode, ApiError> {
    if like && !current.access.sees_track(pool, track).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    let device = session_device(current.token_id);
    let done = likes::set(pool, current.user.id, &device, track, like, now_ms())
        .await
        .map_err(write_failed)?;
    if !done {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
