//! Browsing the library for the apps: movies, series, seasons, episodes and artwork.
//! Every route needs a signed-in user.
//!
//! Titles show TMDB's data when matched and fall back to what was parsed from the folder name.
//! Artwork URLs are named after TMDB's image files, so a URL never changes meaning and clients
//! may cache images forever.

use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::title_artwork::{ArtworkState, extras};
use crate::db::unix_now;
use crate::tmdb::Images;

mod added;
mod artwork;
mod bonus;
mod browse;
mod lists;
mod titles;

pub(super) use added::*;
pub(super) use artwork::*;
use bonus::*;
pub(super) use browse::*;
use lists::*;
use titles::*;

/// The sizes this API hands out for each kind of image. Only these are fetched, so the cache
/// can't grow past what browsing fills.
pub(super) const POSTER_SIZES: &[&str] = &["w342", "w500"];
pub(super) const BACKDROP_SIZES: &[&str] = &["w1280"];
const STILL_SIZES: &[&str] = &["w300"];
pub(super) const LOGO_SIZES: &[&str] = &["w500"];
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;
/// Longest episode range one file may claim (S01E01-E99), to bound responses.
const MAX_EPISODES_PER_FILE: i64 = 100;

/// How long an image TMDB says it doesn't have is answered 404 without asking again.
const MISS_FOR: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

#[derive(Clone)]
struct MediaState {
    pool: SqlitePool,
    artwork_dir: PathBuf,
    images: Arc<Images>,
    /// Images TMDB answered 404 for, and when. Only images a cached title refers to get
    /// here, so it stays small; it's forgotten on restart, which costs one more request.
    misses: Arc<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>>,
    /// For the textless backdrop, logo and focal points details carry.
    title_artwork: ArtworkState,
}

/// Browsing, with details giving only the textless backdrops, logos and focal points already
/// stored.
#[cfg(test)]
pub fn routes(pool: SqlitePool, artwork_dir: PathBuf, images: Images) -> Router {
    let stored = ArtworkState::stored_only(pool.clone());
    routes_with(pool, artwork_dir, images, stored)
}

/// Browsing, with details picking and working out textless backdrops, logos and focal points
/// as `/artwork` does.
pub(super) fn routes_with(
    pool: SqlitePool,
    artwork_dir: PathBuf,
    images: Images,
    title_artwork: ArtworkState,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let state = MediaState {
        pool,
        artwork_dir,
        images: Arc::new(images),
        misses: Arc::default(),
        title_artwork,
    };
    Router::new()
        .route("/api/v1/movies", get(list_movies))
        .route("/api/v1/movies/{id}", get(movie))
        .route("/api/v1/movies/{id}/extras", get(movie_extras))
        .route("/api/v1/series", get(list_series))
        .route("/api/v1/series/{id}", get(series))
        .route("/api/v1/series/{id}/extras", get(series_extras))
        .route("/api/v1/series/{id}/seasons/{season}", get(season))
        .route("/api/v1/artwork/{size}/{file}", get(artwork))
        .route("/api/v1/browse/libraries", get(browse_libraries))
        .route("/api/v1/browse/genres", get(browse_genres))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(state)
}

/// Makes `%`, `_` and `\` in a search match themselves.
pub(super) fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub(super) fn artwork_url(size: &str, path: Option<String>) -> Option<String> {
    path.map(|p| format!("/api/v1/artwork/{size}{p}"))
}

fn not_found(what: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, format!("no such {what}"))
}

fn genres(json: Option<String>) -> Result<Value, ApiError> {
    Ok(match json {
        Some(json) => serde_json::from_str(&json)?,
        None => json!([]),
    })
}

#[cfg(test)]
mod tests;
