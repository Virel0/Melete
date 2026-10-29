//! Finding subtitles a file lacks on OpenSubtitles, for anyone signed in, once an admin has
//! set it up: search first and see what there is, then fetch one. One fetch per file version
//! and language; searches are remembered, an empty one included.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback};
use super::playback::{LoadedFile, load_file, require_playable, resolve};
use crate::opensubtitles::{self, Failure, Found, Version, Wanted};

#[derive(Clone)]
struct SearchState {
    pool: SqlitePool,
    ffprobe: Arc<PathBuf>,
    subtitles: Arc<PathBuf>,
    /// OpenSubtitles' API: the real one, or a stand-in in tests.
    api: Arc<str>,
}

/// Searching and fetching, for everyone signed in who may play the file: part of playing, so
/// playback keys reach it too.
pub fn routes(pool: SqlitePool, ffprobe: PathBuf, subtitles: PathBuf) -> Router {
    routes_at(pool, ffprobe, subtitles, opensubtitles::API)
}

/// The same, with OpenSubtitles' API at `api`.
pub fn routes_at(pool: SqlitePool, ffprobe: PathBuf, subtitles: PathBuf, api: &str) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/files/{id}/subtitles/search", post(search))
        .route("/api/v1/files/{id}/subtitles/fetch", post(fetch))
        .route_layer(middleware::from_fn_with_state(auth, require_playback))
        .with_state(SearchState {
            pool,
            ffprobe: Arc::new(ffprobe),
            subtitles: Arc::new(subtitles),
            api: api.into(),
        })
}

/// A fetched subtitle as the subtitle list shows it, with the encoding its file is read as.
pub(super) fn fetched_track(fetched: &opensubtitles::Fetched, encoding: Option<&str>) -> Value {
    json!({
        "track": opensubtitles::track_id(&fetched.language),
        "source": "opensubtitles",
        "codec": "subrip",
        "language": fetched.language,
        "title": fetched.release,
        "default": false,
        "forced": false,
        "hearing_impaired": fetched.hearing_impaired,
        "text": true,
        "file": null,
        "encoding": encoding,
    })
}

/// The encoding a fetched subtitle's file is read as.
async fn encoding_of(
    state: &SearchState,
    version: Version,
    language: &str,
) -> Option<&'static str> {
    let path = opensubtitles::fetched_path(&state.subtitles, version, language);
    tokio::task::spawn_blocking(move || crate::subtitles::file_encoding(&path))
        .await
        .ok()
        .flatten()
}

/// OpenSubtitles' client, when the caller may play the file, it's a film's or an episode's
/// (OpenSubtitles has subtitles for those, not for their extras), and OpenSubtitles is set up.
async fn playable(
    state: &SearchState,
    current: &CurrentUser,
    id: i64,
) -> Result<opensubtitles::Client, ApiError> {
    require_playable(&state.pool, &current.access, id).await?;
    if crate::library::extras::is_extra_id(id) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "OpenSubtitles has subtitles for films and episodes, not their extras",
        ));
    }
    client(state).await
}

/// The OpenSubtitles client as the settings stand now; 409 while it isn't set up.
async fn client(state: &SearchState) -> Result<opensubtitles::Client, ApiError> {
    let config = crate::settings::load(&state.pool).await?;
    let settings = config.opensubtitles.ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "OpenSubtitles isn't set up; an admin can turn it on under Settings, Outside services",
        )
    })?;
    Ok(opensubtitles::Client::with_api(&settings, &state.api)?)
}

fn failed(failure: Failure) -> ApiError {
    match failure {
        Failure::Quota(_) => ApiError::new(StatusCode::TOO_MANY_REQUESTS, failure.to_string()),
        other => ApiError::new(StatusCode::BAD_GATEWAY, other.to_string()),
    }
}

/// "en", "pt-br": as OpenSubtitles names languages.
fn checked_language(language: &str) -> Result<String, ApiError> {
    opensubtitles::language_code(language).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "language is two letters, or two and a region: en, fr, pt-br",
        )
    })
}

#[derive(Deserialize)]
struct SearchQuery {
    language: String,
}

/// What a file is, for OpenSubtitles: its hash, its title's ids, and its season and episode.
async fn wanted(
    state: &SearchState,
    file: &LoadedFile,
    language: &str,
) -> Result<Wanted, ApiError> {
    let video = resolve(&file.root, &file.rel_path).await?;
    let hash = tokio::task::spawn_blocking(move || opensubtitles::hash(&video))
        .await?
        .unwrap_or(None);
    Ok(opensubtitles::wanted(&state.pool, file.id, hash, language).await?)
}

/// What OpenSubtitles has for a file in one language, made for this file first. A search is
/// sent once a week at most per file version and language; until then what it found, nothing
/// included, is shown again. Says which subtitle was fetched already, if any.
async fn search(
    State(state): State<SearchState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let language = checked_language(&query.language)?;
    let client = playable(&state, &current, id).await?;
    let file = load_file(&state.pool, &state.ffprobe, id).await?;
    let version: Version = (file.id, file.size, file.modified_at);
    let (results, remembered) =
        match opensubtitles::remembered(&state.pool, version, &language).await? {
            Some(found) => (found, true),
            None => {
                let found = client
                    .search(&wanted(&state, &file, &language).await?)
                    .await
                    .map_err(failed)?;
                opensubtitles::remember(&state.pool, version, &language, &found).await?;
                (found, false)
            }
        };
    let fetched = opensubtitles::fetched_for(&state.pool, id)
        .await?
        .into_iter()
        .find(|f| f.language == language);
    let fetched = match fetched {
        Some(f) => Some(fetched_track(
            &f,
            encoding_of(&state, version, &language).await,
        )),
        None => None,
    };
    Ok(Json(json!({
        "language": language,
        "results": results,
        "remembered": remembered,
        "fetched": fetched,
    })))
}

#[derive(Deserialize)]
struct FetchBody {
    language: String,
    /// The result to fetch, by its id from the search.
    result: i64,
}

/// Fetches one of a search's results as the file's subtitle in its language, and gives it as
/// the subtitle list shows it. One per file version and language: asked again, the one
/// fetched is given.
async fn fetch(
    State(state): State<SearchState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<FetchBody>,
) -> Result<Json<Value>, ApiError> {
    let language = checked_language(&body.language)?;
    let client = playable(&state, &current, id).await?;
    let file = load_file(&state.pool, &state.ffprobe, id).await?;
    let version: Version = (file.id, file.size, file.modified_at);
    let known = opensubtitles::fetched_for(&state.pool, id).await?;
    if let Some(fetched) = known.iter().find(|f| f.language == language) {
        let encoding = encoding_of(&state, version, &language).await;
        return Ok(Json(fetched_track(fetched, encoding)));
    }
    if let Some(reset_at) = opensubtitles::used_up_until(&state.pool).await? {
        return Err(failed(Failure::Quota(reset_at)));
    }
    let results = opensubtitles::remembered(&state.pool, version, &language).await?;
    let mut found: Found = results
        .unwrap_or_default()
        .into_iter()
        .find(|f| f.id == body.result)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "that isn't one of this file's search results; search again",
            )
        })?;
    // Kept under the language asked for, however OpenSubtitles writes it.
    found.language = language.clone();
    let bytes = match client.download(found.id).await {
        Ok(bytes) => bytes,
        Err(Failure::Quota(reset_at)) => {
            opensubtitles::used_up(&state.pool, reset_at).await?;
            return Err(failed(Failure::Quota(reset_at)));
        }
        Err(other) => return Err(failed(other)),
    };
    opensubtitles::keep(&state.pool, &state.subtitles, version, &found, &bytes).await?;
    let encoding = encoding_of(&state, version, &language).await;
    let fetched = opensubtitles::Fetched {
        language,
        release: found.release,
        hearing_impaired: found.hearing_impaired,
    };
    Ok(Json(fetched_track(&fetched, encoding)))
}

#[cfg(test)]
mod tests;
