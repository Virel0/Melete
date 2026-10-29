//! Playing files: stream URLs, byte-range streaming, and per-user watch progress.
//!
//! An app asks to play a file and gets a stream URL carrying its own token, so players that
//! can't send headers (AVPlayer, VLC) never see the user's real token. Progress is per user and
//! per file; reaching 90% of a file counts as watching it.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRef, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower_http::services::ServeFile;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback};
use super::media::{Exclusions, artwork_url};
use crate::access::Access;
use crate::db::unix_now;
use crate::library::extras;
use crate::{auth, probe};

mod files;
mod limits;
mod reports;
mod resuming;
mod streams;

pub(super) use files::*;
pub(super) use limits::*;
use reports::*;
pub(super) use resuming::*;
pub(super) use streams::*;

pub(super) const STREAM_PREFIX: &str = "mls_";
/// Live stream links to files and to extras alike, for a `WHERE` on their token hash and
/// expiry: each one's file id (an extra's id for an extra), conversion settings (for an HLS
/// playlist), the session or key it was given to, library folder and path. Only present files
/// and extras.
pub(super) const LINKS: &str = "(SELECT p.token_hash, p.expires_at, p.file_id, p.transcode,
         p.token_id, l.path AS root, f.rel_path
     FROM play_sessions p JOIN media_files f ON f.id = p.file_id
     JOIN libraries l ON l.id = f.library_id WHERE f.missing_since IS NULL
     UNION ALL SELECT p.token_hash, p.expires_at, p.extra_id, p.transcode, p.token_id, l.path,
         e.rel_path
     FROM extra_play_sessions p JOIN media_extras e ON e.id = p.extra_id
     JOIN libraries l ON l.id = e.library_id WHERE e.missing_since IS NULL)";
pub(super) const STREAM_LIFETIME: i64 = 24 * 60 * 60;
/// Reaching this share of a file's duration marks it watched.
const WATCHED_FRACTION: f64 = 0.9;
/// No real file plays for longer than a week; anything above is a client bug.
const MAX_SECONDS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
/// A report this close to the start...
const NEAR_START: f64 = 5.0;
/// ...when the saved position is at least this far in is ignored, unless it says it's a
/// restart: a player reporting before it has seeked to where it resumes looks just like it.
const FAR_IN: f64 = 60.0;
pub(super) const CONTINUE_LIMIT: i64 = 20;
/// Live stream links kept per session or API key; older ones are dropped.
pub(super) const MAX_STREAMS_PER_TOKEN: i64 = 50;
/// ffprobe runs started by requests at once; the background job has its own limit.
static ON_DEMAND_PROBES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Clone)]
struct PlaybackState {
    pool: SqlitePool,
    ffprobe: Arc<PathBuf>,
}

impl FromRef<PlaybackState> for SqlitePool {
    fn from_ref(state: &PlaybackState) -> Self {
        state.pool.clone()
    }
}

/// File info, playing and watch progress, for everyone signed in, and the stream link playing
/// gives, which carries its own token.
pub fn routes(pool: SqlitePool, ffprobe: PathBuf) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let signed_in = Router::new()
        .route("/api/v1/files/{id}", get(file_info))
        .route("/api/v1/files/{id}/play", post(play))
        .route("/api/v1/files/{id}/progress", put(report_progress))
        .route("/api/v1/files/{id}/watched", put(set_watched))
        .route("/api/v1/series/{id}/watched", put(set_series_watched))
        .route("/api/v1/me/continue", get(continue_watching))
        .route("/api/v1/me/progress", get(progress))
        .route_layer(middleware::from_fn_with_state(auth, require_playback));
    // The stream URL's own token is the credential here.
    let stream = Router::new().route("/api/v1/stream/{token}", get(stream));
    signed_in.merge(stream).with_state(PlaybackState {
        pool,
        ffprobe: Arc::new(ffprobe),
    })
}

#[derive(Clone, Copy, Default, sqlx::FromRow)]
pub(super) struct Progress {
    /// Where playback was last reported, in seconds; 0 once watched.
    pub position_seconds: f64,
    /// The file's length in seconds, when a player said.
    pub duration_seconds: Option<f64>,
    /// Whether the user has watched it.
    pub watched: bool,
    /// How many times the user has watched it through.
    pub play_count: i64,
}

pub(super) async fn load_progress(
    pool: &SqlitePool,
    user_id: i64,
    file_id: i64,
) -> Result<Option<Progress>, ApiError> {
    Ok(sqlx::query_as(
        "SELECT position_seconds, duration_seconds, watched, play_count FROM playback_progress
         WHERE user_id = ? AND file_id = ?",
    )
    .bind(user_id)
    .bind(file_id)
    .fetch_optional(pool)
    .await?)
}

fn progress_json(p: &Progress) -> Value {
    json!({
        "position_seconds": p.position_seconds,
        "duration_seconds": p.duration_seconds,
        "watched": p.watched,
        "play_count": p.play_count,
    })
}

#[cfg(test)]
mod tests;
