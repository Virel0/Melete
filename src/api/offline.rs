//! Offline copies for the apps and the web UI: what a copy of a file would take, asking for
//! one, following it, a download link once it's ready, and deleting it. Each person sees only
//! their own copies. Nothing is made unless an admin has turned offline copies on.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tower_http::services::ServeFile;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback};
use super::playback::{holder_sees, load_file, overall_bitrate, require_playable};
use crate::offline::{self, Copy};
use crate::transcode::{self, Hardware, Settings};

#[derive(Clone)]
struct OfflineState {
    pool: SqlitePool,
    ffprobe: Arc<PathBuf>,
    root: Arc<PathBuf>,
}

/// The routes: asking and following for everyone signed in, downloads behind their link's token.
pub fn routes(pool: SqlitePool, ffprobe: PathBuf, root: PathBuf) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let signed_in = Router::new()
        .route("/api/v1/files/{id}/offline", get(choices).post(ask))
        .route("/api/v1/offline", get(mine))
        .route("/api/v1/offline/{id}", get(one).delete(remove))
        .route("/api/v1/offline/{id}/link", post(link))
        .route_layer(middleware::from_fn_with_state(auth, require_playback));
    // The link's own token is the credential here.
    let downloads = Router::new()
        .route("/api/v1/offline/download/{token}", get(download))
        .route(
            "/api/v1/offline/download/{token}/{track}",
            get(download_subtitle),
        );
    signed_in.merge(downloads).with_state(OfflineState {
        pool,
        ffprobe: Arc::new(ffprobe),
        root: Arc::new(root),
    })
}

fn off() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "offline copies are off; an admin can turn them on under Settings, Outside services",
    )
}

fn no_copy() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such offline copy")
}

/// A 404 unless the caller may play the file, and a 422 for an extra: copies are of films and
/// episodes.
async fn copyable(state: &OfflineState, current: &CurrentUser, id: i64) -> Result<(), ApiError> {
    require_playable(&state.pool, &current.access, id).await?;
    if crate::library::extras::is_extra_id(id) {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "offline copies are of films and episodes, not their extras",
        ));
    }
    Ok(())
}

/// Whether the caller may still see what a copy is of: a copy of a title hidden from them since
/// is as if it weren't there.
async fn visible(
    state: &OfflineState,
    current: &CurrentUser,
    copy: &Copy,
) -> Result<bool, ApiError> {
    Ok(current.access.sees_file(&state.pool, copy.file_id).await?)
}

/// What a copy of a file would take at each size up to the file's own, the copies the caller
/// has of it, and whether copies can be asked for.
async fn choices(
    State(state): State<OfflineState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    copyable(&state, &current, id).await?;
    let file = load_file(&state.pool, &state.ffprobe, id).await?;
    let on = crate::settings::offline_copies(&state.pool).await?;
    let mut sizes = Vec::new();
    if let Some(media) = &file.media {
        let source = media.video.first().and_then(|v| v.height).unwrap_or(1080);
        for height in [2160, 1080, 720, 480]
            .into_iter()
            .filter(|h| *h <= source.max(480))
        {
            let settings = Settings {
                max_height: Some(height),
                ..Settings::default()
            };
            if let Ok(plan) = transcode::plan(PathBuf::new(), media, &settings, Hardware::Software)
            {
                let bytes = offline::estimate(&plan, overall_bitrate(file.size, Some(media)));
                sizes.push(json!({ "max_height": height, "estimated_bytes": bytes }));
            }
        }
    }
    Ok(Json(json!({
        "on": on,
        "choices": sizes,
        "copies": offline::of_user(&state.pool, current.user.id, Some(id)).await?,
    })))
}

/// What an offline copy is asked for with: a transcode's choices, and text subtitles to keep
/// beside it.
#[derive(Deserialize)]
struct Ask {
    #[serde(flatten)]
    settings: Settings,
    #[serde(default)]
    subtitles: Vec<String>,
}

/// Asks for a copy of a file, made in the background; answers it as queued.
async fn ask(
    State(state): State<OfflineState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(asked): Json<Ask>,
) -> Result<(StatusCode, Json<Copy>), ApiError> {
    let bad = |why: &str| ApiError::new(StatusCode::BAD_REQUEST, why);
    asked.settings.validate().map_err(bad)?;
    if asked.settings.burn_subtitle.is_some() {
        return Err(bad(
            "an offline copy keeps subtitles beside it: list them in subtitles, not burn_subtitle",
        ));
    }
    let tracks_ok = asked.subtitles.len() <= 10
        && asked.subtitles.iter().all(|t| {
            let check = Settings {
                burn_subtitle: Some(t.clone()),
                ..Settings::default()
            };
            check.validate().is_ok()
        });
    if !tracks_ok {
        return Err(bad(
            "subtitles are up to 10 track ids from the subtitle list, like s3 or x1a2b3c4d",
        ));
    }
    copyable(&state, &current, id).await?;
    if !crate::settings::offline_copies(&state.pool).await? {
        return Err(off());
    }
    if offline::held(&state.pool, current.user.id).await? >= offline::PER_USER {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "you have {} offline copies already; delete some first",
                offline::PER_USER
            ),
        ));
    }
    let file = load_file(&state.pool, &state.ffprobe, id).await?;
    let media = file.media.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "ffprobe couldn't read this file, so it can't be copied",
        )
    })?;
    let plan = transcode::plan(PathBuf::new(), media, &asked.settings, Hardware::Software)
        .map_err(|why| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why))?;
    let estimated = offline::estimate(&plan, overall_bitrate(file.size, Some(media)));
    let settings = serde_json::to_string(&asked.settings)?;
    let copy_id = offline::create(
        &state.pool,
        current.user.id,
        id,
        &settings,
        &asked.subtitles,
        estimated,
    )
    .await?;
    let copy = offline::get(&state.pool, current.user.id, copy_id)
        .await?
        .ok_or_else(no_copy)?;
    Ok((StatusCode::ACCEPTED, Json(copy)))
}

/// The caller's copies of what they may see, newest first.
async fn mine(
    State(state): State<OfflineState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Vec<Copy>>, ApiError> {
    let mut shown = Vec::new();
    for copy in offline::of_user(&state.pool, current.user.id, None).await? {
        if visible(&state, &current, &copy).await? {
            shown.push(copy);
        }
    }
    Ok(Json(shown))
}

/// One of the caller's copies, with how far along it is.
async fn one(
    State(state): State<OfflineState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Copy>, ApiError> {
    let copy = offline::get(&state.pool, current.user.id, id)
        .await?
        .ok_or_else(no_copy)?;
    if !visible(&state, &current, &copy).await? {
        return Err(no_copy());
    }
    Ok(Json(copy))
}

/// Deletes one of the caller's copies: a device does once it has the file, or to cancel. Their
/// own copy can be deleted even of a title hidden from them since.
async fn remove(
    State(state): State<OfflineState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    offline::get(&state.pool, current.user.id, id)
        .await?
        .ok_or_else(no_copy)?;
    offline::delete(&state.pool, &state.root, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A download link for a ready copy, with its subtitles', a day long and tied to the session
/// asking, so an interrupted download can go on where it stopped.
async fn link(
    State(state): State<OfflineState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let copy = offline::get(&state.pool, current.user.id, id)
        .await?
        .ok_or_else(no_copy)?;
    if !visible(&state, &current, &copy).await? {
        return Err(no_copy());
    }
    if copy.status != "ready" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "the copy isn't ready yet",
        ));
    }
    let (token, expires_at) = offline::link(&state.pool, id, current.token_id).await?;
    let base = format!("/api/v1/offline/download/{token}");
    let subtitles: Vec<Value> = copy
        .subtitles
        .iter()
        .map(|t| json!({ "track": t, "url": format!("{base}/{t}.vtt") }))
        .collect();
    Ok(Json(
        json!({ "download_url": base, "expires_at": expires_at, "subtitles": subtitles }),
    ))
}

fn not_a_link(token: &str) -> Option<ApiError> {
    (!token.starts_with(offline::LINK_PREFIX))
        .then(|| ApiError::new(StatusCode::BAD_REQUEST, "not an offline copy's link"))
}

fn link_gone() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "this link is unknown or has expired, or the copy is gone; ask for a link again",
    )
}

/// The copy a link opens and its subtitle tracks, noting the download, while the link's holder
/// may still see what it's a copy of.
async fn opened(state: &OfflineState, token: &str) -> Result<(i64, Vec<String>), ApiError> {
    if let Some(err) = not_a_link(token) {
        return Err(err);
    }
    let opened = offline::opened_by(&state.pool, token)
        .await?
        .ok_or_else(link_gone)?;
    if !holder_sees(&state.pool, opened.holder, opened.file).await? {
        return Err(link_gone());
    }
    offline::downloaded(&state.pool, opened.copy).await?;
    Ok((opened.copy, opened.subtitles))
}

/// A copy's file, with byte ranges so a download can resume.
async fn download(
    State(state): State<OfflineState>,
    Path(token): Path<String>,
    request: Request,
) -> Result<Response, ApiError> {
    let (id, _) = opened(&state, &token).await?;
    let response = ServeFile::new(offline::video_path(&state.root, id))
        .try_call(request)
        .await?;
    let mut response = response.map(Body::new);
    let name = format!("attachment; filename=\"melete-{id}.mp4\"");
    if let Ok(value) = name.parse() {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

/// One of a copy's subtitle files (`{track}.vtt`).
async fn download_subtitle(
    State(state): State<OfflineState>,
    Path((token, track)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (id, tracks) = opened(&state, &token).await?;
    let track = track.strip_suffix(".vtt").ok_or_else(link_gone)?;
    if !tracks.iter().any(|t| t == track) {
        return Err(link_gone());
    }
    let bytes = tokio::fs::read(offline::subtitle_path(&state.root, id, track))
        .await
        .map_err(|_| link_gone())?;
    Ok(([(header::CONTENT_TYPE, "text/vtt; charset=utf-8")], bytes).into_response())
}

#[cfg(test)]
mod tests;
