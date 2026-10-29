//! Seek-bar previews: a file's index for everyone signed in, and its sheets behind a stream
//! link's token, as subtitles are, so a player can load them without headers.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::playback::{LINKS, STREAM_PREFIX, holder_sees, require_playable};
use crate::auth;
use crate::db::unix_now;
use crate::trickplay::{self, Index};

#[derive(Clone)]
struct TrickplayState {
    pool: SqlitePool,
    dir: Arc<PathBuf>,
}

/// A file's preview index, for everyone signed in, and its sheets behind a stream link's token.
pub fn routes(pool: SqlitePool, dir: PathBuf) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let signed_in = Router::new()
        .route("/api/v1/files/{id}/trickplay", get(index))
        .route_layer(middleware::from_fn_with_state(auth, require_user));
    // The stream link's own token is the credential here.
    let sheets = Router::new().route("/api/v1/stream/{token}/trickplay/{sheet}", get(sheet));
    signed_in.merge(sheets).with_state(TrickplayState {
        pool,
        dir: Arc::new(dir),
    })
}

fn none_yet() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "this file has no seek-bar previews: they're off, or not made yet",
    )
}

/// The file's previews: each picture's size, how the sheets are laid out and each picture's
/// time. 404 when there are none for the file as it is now.
async fn index(
    State(state): State<TrickplayState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Index>, ApiError> {
    require_playable(&state.pool, &current.access, id).await?;
    trickplay::index_of(&state.pool, &state.dir, id)
        .await?
        .map(Json)
        .ok_or_else(none_yet)
}

/// One sheet (`{n}.jpg`) of the previews of a stream link's file.
async fn sheet(
    State(state): State<TrickplayState>,
    Path((token, sheet)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    if !token.starts_with(STREAM_PREFIX) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "not a stream link"));
    }
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "this stream link is unknown or has expired; ask to play again",
        )
    };
    let session: Option<(i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT file_id, token_id FROM {LINKS} WHERE token_hash = ? AND expires_at > ?"
    )))
    .bind(auth::token_hash(&token))
    .bind(unix_now())
    .fetch_optional(&state.pool)
    .await?;
    let Some((file_id, token_id)) = session else {
        return Err(gone());
    };
    // A link made before the title was hidden from its holder stops with that.
    if !holder_sees(&state.pool, token_id, file_id).await? {
        return Err(gone());
    }
    let number = sheet
        .strip_suffix(".jpg")
        .filter(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|n| n.parse::<i64>().ok())
        .ok_or_else(none_yet)?;
    let path = trickplay::sheet_path(&state.pool, &state.dir, file_id, number)
        .await?
        .ok_or_else(none_yet)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| none_yet())?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            // The link's token is in the address, so only this browser keeps it.
            (header::CACHE_CONTROL, "private, max-age=86400"),
        ],
        bytes,
    )
        .into_response())
}

#[cfg(test)]
mod tests;
