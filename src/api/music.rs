//! Browsing music for the apps: artists, albums, tracks and genres, with
//! MusicBrainz ids wherever they're known, lists filtered by several artists or albums
//! at once, and album covers and motion covers. Every route needs a signed-in user.
//!
//! An album's cover is the image in its folder, else the picture in a track's tags, else the
//! Cover Art Archive's for its MusicBrainz release group (fetched once when first asked for,
//! and only with MusicBrainz matching on). A motion cover is a looping video in its folder, else
//! one made from the cover, shown while motion covers are on to users who haven't turned
//! those off for themselves.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower_http::services::ServeFile;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
pub(super) use super::bad_request;
use super::media::escape_like;
use super::playback::resolve;
use crate::access::Access;
use crate::coverart;

mod albums;
mod artists;
mod covers;
mod tracks;

pub(super) use albums::*;
pub(super) use artists::*;
use covers::*;
pub(super) use tracks::*;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;
/// Most ids one filter takes.
const MAX_IDS: usize = 100;
/// How long a release group the Cover Art Archive has no cover for is answered 404 without
/// asking again.
const MISS_FOR: Duration = Duration::from_secs(24 * 3600);
/// Covers can change (a new folder image, a retag), so clients keep them a day, not forever.
const COVER_CACHE: &str = "private, max-age=86400";

#[derive(Clone)]
pub(super) struct MusicState {
    pub(super) pool: SqlitePool,
    /// Where pictures pulled from tags and the Cover Art Archive's covers are kept.
    covers: PathBuf,
    /// Motion covers made from still covers.
    motion: PathBuf,
    pub(super) coverart: Option<Arc<coverart::Client>>,
}

/// The music browsing routes, for everyone signed in. `covers` is where pictures from tags and
/// the Cover Art Archive are kept, `motion_dir` where motion covers are; without `coverart`, the
/// Cover Art Archive isn't asked.
pub fn routes(
    pool: SqlitePool,
    covers: PathBuf,
    motion_dir: PathBuf,
    coverart: Option<Arc<coverart::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/artists", get(list_artists))
        .route("/api/v1/music/artists/{id}", get(artist))
        .route("/api/v1/music/albums", get(list_albums))
        .route("/api/v1/music/albums/{id}", get(album))
        .route("/api/v1/music/albums/{id}/cover", get(cover))
        .route("/api/v1/music/albums/{id}/motion", get(motion))
        .route("/api/v1/music/tracks", get(list_tracks))
        .route("/api/v1/music/genres", get(genres))
        .merge(super::music_classical::routes())
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(MusicState {
            pool,
            covers,
            motion: motion_dir,
            coverart,
        })
}

pub(super) fn paging(limit: Option<i64>, offset: Option<i64>) -> Result<(i64, i64), ApiError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let offset = offset.unwrap_or(0);
    if !(1..=MAX_LIMIT).contains(&limit) || offset < 0 {
        return Err(bad_request("limit is 1 to 500 and offset is 0 or more"));
    }
    Ok((limit, offset))
}

/// "3,7,12" as a JSON array for `json_each`, or `None` when not given.
pub(super) fn ids(list: Option<&str>, name: &str) -> Result<Option<String>, ApiError> {
    let Some(list) = list.filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    let parsed: Result<Vec<i64>, _> = list.split(',').map(|id| id.trim().parse()).collect();
    match parsed {
        Ok(ids) if ids.len() <= MAX_IDS => Ok(Some(json!(ids).to_string())),
        _ => Err(bad_request(&format!(
            "{name} is up to {MAX_IDS} ids separated by commas"
        ))),
    }
}

pub(super) fn pattern(q: Option<&str>) -> Option<String> {
    q.filter(|q| !q.is_empty())
        .map(|q| format!("%{}%", escape_like(q)))
}

// ---------- Albums ----------

#[cfg(test)]
mod tests;
