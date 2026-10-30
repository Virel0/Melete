//! Liked tracks and albums for apps that don't sync (the web UI): read from the caller's Aoide
//! sync log and set by writing into it what a device of theirs would, so every device sees the
//! change through its next pull and the log stays the one place likes are kept. Like the rest of
//! what's in that log, they're closed to a user who may see only some music libraries.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::{Shown, albums_by_id, tracks_by_id};
use crate::aoide::likes::{self, Kind};
use crate::aoide::writing::{WriteError, session_device};
use crate::db::now_ms;

#[derive(Clone)]
struct LikesState {
    pool: SqlitePool,
    /// Whether covers come from the Cover Art Archive.
    archive: bool,
}

/// The like routes, for everyone signed in who may see every music library. `archive`: whether
/// covers come from the Cover Art Archive.
pub fn routes(pool: SqlitePool, archive: bool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/likes", get(list))
        .route("/api/v1/music/tracks/{id}/like", put(like).delete(unlike))
        .route(
            "/api/v1/music/albums/{id}/like",
            put(like_album).delete(unlike_album),
        )
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(LikesState { pool, archive })
}

/// What a failed write is to the caller: a refusal is theirs to see, anything else a 500.
pub(super) fn write_failed(err: WriteError) -> ApiError {
    match err {
        WriteError::Refused(why) => ApiError::new(StatusCode::FORBIDDEN, why),
        WriteError::Database(err) => err.into(),
        WriteError::NoRandomness => anyhow::anyhow!("no randomness available").into(),
    }
}

/// The caller's liked tracks and albums, newest like first, each with its track or album as
/// the music API shows it, or null when it isn't in the library now (its content key says what
/// it was).
async fn list(
    State(state): State<LikesState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let (pool, me) = (&state.pool, current.user.id);
    let liked = likes::liked(pool, Kind::Track, me).await?;
    let ids: Vec<i64> = liked.iter().filter_map(|(track, _)| *track).collect();
    let tracks = tracks_by_id(pool, &current.access, me, &ids).await?;
    let liked_albums = likes::liked(pool, Kind::Album, me).await?;
    let ids: Vec<i64> = liked_albums
        .iter()
        .filter_map(|(album, _)| *album)
        .collect();
    let shown = Shown::to(pool, state.archive, me).await?;
    let albums = albums_by_id(pool, &current.access, shown, &ids).await?;
    let items = |liked: &[(Option<i64>, likes::Like)], found: &HashMap<i64, Value>, key: &str| {
        liked
            .iter()
            .map(|(id, row)| {
                json!({
                    "liked_at": row.updated_at.div_euclid(1000),
                    "content_key": row.content_key,
                    key: id.and_then(|id| found.get(&id)).cloned(),
                })
            })
            .collect::<Vec<Value>>()
    };
    Ok(Json(json!({
        "tracks": items(&liked, &tracks, "track"),
        "albums": items(&liked_albums, &albums, "album"),
    })))
}

async fn like(
    State(state): State<LikesState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    set(&state.pool, &current, Kind::Track, id, true).await
}

async fn unlike(
    State(state): State<LikesState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    set(&state.pool, &current, Kind::Track, id, false).await
}

async fn like_album(
    State(state): State<LikesState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    set(&state.pool, &current, Kind::Album, id, true).await
}

async fn unlike_album(
    State(state): State<LikesState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    set(&state.pool, &current, Kind::Album, id, false).await
}

async fn set(
    pool: &SqlitePool,
    current: &CurrentUser,
    kind: Kind,
    id: i64,
    like: bool,
) -> Result<StatusCode, ApiError> {
    let (seen, missing) = match kind {
        Kind::Track => (current.access.sees_track(pool, id).await?, "no such track"),
        Kind::Album => (current.access.sees_album(pool, id).await?, "no such album"),
    };
    if like && !seen {
        return Err(ApiError::new(StatusCode::NOT_FOUND, missing));
    }
    let device = session_device(current.token_id);
    let done = likes::set(pool, kind, current.user.id, &device, id, like, now_ms())
        .await
        .map_err(write_failed)?;
    if !done {
        return Err(ApiError::new(StatusCode::NOT_FOUND, missing));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
