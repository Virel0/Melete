//! What players that draw ASS/SSA themselves need, behind the stream link's token: an ASS/SSA
//! track as it is, and the fonts the video carries. Fonts are named by their stream index and
//! looked up in the video's own list, so no name a client sends becomes a path.

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use tower_http::services::ServeFile;

use super::super::playback::{LoadedFile, load_file, resolve};
use super::{ApiError, SubtitleState, conversion_failed, linked_file, sidecars_of};
use crate::subtitles::{self, Font, Format, Sidecar, Source, embedded_key, fonts_key, sidecar_key};

/// An ASS/SSA track as it is: an embedded one copied out of the video once and then cached, a
/// file beside the video as written, in UTF-8 (see [`subtitles::original_file`]). `name` is the
/// track's id.
pub(super) async fn ass(
    state: &SubtitleState,
    file_id: i64,
    name: &str,
) -> Result<Response, ApiError> {
    let no_track = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "no such ASS/SSA track; other tracks are served as WebVTT",
        )
    };
    let file = load_file(&state.pool, &state.ffprobe, file_id).await?;
    let video = resolve(&file.root, &file.rel_path).await?;
    let (bytes, encoding) =
        if let Some(index) = name.strip_prefix('s').and_then(|i| i.parse::<i64>().ok()) {
            file.media
                .iter()
                .flat_map(|m| &m.subtitles)
                .find(|s| s.index == index && subtitles::is_ass(&s.codec))
                .ok_or_else(no_track)?;
            embedded(state, &file, &video, index).await?
        } else if let Some(id) = name.strip_prefix('x') {
            let sidecars = sidecars_of(video).await?;
            let sidecar = sidecars
                .into_iter()
                .find(|s| s.id == id && subtitles::is_ass(&s.codec))
                .ok_or_else(no_track)?;
            beside(state, &file, &sidecar).await?
        } else {
            return Err(no_track());
        };
    let content_type = HeaderValue::from_str(&format!("text/x-ass; charset={encoding}"))
        .unwrap_or(HeaderValue::from_static("text/x-ass"));
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        bytes,
    )
        .into_response())
}

/// An embedded ASS/SSA track copied out once, and the encoding of its text.
async fn embedded(
    state: &SubtitleState,
    file: &LoadedFile,
    video: &std::path::Path,
    index: i64,
) -> Result<(Vec<u8>, &'static str), ApiError> {
    let key = embedded_key(file.id, index, file.size, file.modified_at);
    let source = Source::Embedded { video, index };
    let path = subtitles::convert(&state.ffmpeg, &state.cache, &key, source, Format::Original)
        .await
        .map_err(|err| conversion_failed(&file.rel_path, err))?;
    // Matroska keeps a track's text in UTF-8, and ffmpeg copies it as it is.
    Ok((tokio::fs::read(path).await?, "UTF-8"))
}

/// A subtitle file beside the video in UTF-8, converted once and then cached when it's in
/// another encoding, and the charset to send it with.
async fn beside(
    state: &SubtitleState,
    file: &LoadedFile,
    sidecar: &Sidecar,
) -> Result<(Vec<u8>, &'static str), ApiError> {
    let key = sidecar_key(file.id, sidecar);
    subtitles::original_beside(&state.cache, &key, &sidecar.path)
        .await
        .map_err(|err| {
            tracing::warn!(
                rel_path = file.rel_path,
                error = format!("{err:#}"),
                "reading a subtitle file failed"
            );
            ApiError::new(StatusCode::BAD_GATEWAY, "reading the subtitle file failed")
        })
}

/// The fonts the stream link's file carries, each with where to fetch it.
pub(super) async fn fonts(
    State(state): State<SubtitleState>,
    Path(token): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let file_id = linked_file(&state.pool, &token).await?;
    let file = load_file(&state.pool, &state.ffprobe, file_id).await?;
    let fonts = font_list(&state, &file).await?;
    let listed = fonts
        .iter()
        .map(|f| {
            json!({
                "id": f.index,
                "file_name": f.file_name,
                "mime_type": f.mime_type,
                "size": f.size,
                "url": format!("/api/v1/stream/{token}/fonts/{}", f.index),
            })
        })
        .collect();
    Ok(Json(listed))
}

/// One of the fonts the stream link's file carries, by its id in the list.
pub(super) async fn font(
    State(state): State<SubtitleState>,
    Path((token, font)): Path<(String, String)>,
    request: Request,
) -> Result<Response, ApiError> {
    let file_id = linked_file(&state.pool, &token).await?;
    let no_font = || ApiError::new(StatusCode::NOT_FOUND, "no such font");
    if font.is_empty() || font.len() > 5 || !font.bytes().all(|b| b.is_ascii_digit()) {
        return Err(no_font());
    }
    let index: i64 = font.parse().map_err(|_| no_font())?;
    let file = load_file(&state.pool, &state.ffprobe, file_id).await?;
    let font = font_list(&state, &file)
        .await?
        .into_iter()
        .find(|f| f.index == index)
        .ok_or_else(no_font)?;
    let video = resolve(&file.root, &file.rel_path).await?;
    let key = fonts_key(file.id, file.size, file.modified_at);
    let dir = subtitles::fonts(&state.ffprobe, &state.ffmpeg, &state.cache, &key, &video)
        .await
        .map_err(|err| fonts_failed(&file, err))?
        .ok_or_else(no_font)?;
    // Saved under a name of Melete's own, from the index the list holds.
    let path = dir.join(format!("font{}", font.index));
    let mut response = ServeFile::new(path).try_call(request).await?;
    if response.status().is_success() {
        let kind = subtitles::FONT_TYPES
            .iter()
            .find(|t| **t == font.mime_type)
            .copied()
            .unwrap_or("application/octet-stream");
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
    }
    Ok(response.map(axum::body::Body::new))
}

/// The fonts the file carries that players are offered.
async fn font_list(state: &SubtitleState, file: &LoadedFile) -> Result<Vec<Font>, ApiError> {
    let video = resolve(&file.root, &file.rel_path).await?;
    let key = fonts_key(file.id, file.size, file.modified_at);
    subtitles::font_list(&state.ffprobe, &state.cache, &key, &video)
        .await
        .map_err(|err| fonts_failed(file, err))
}

fn fonts_failed(file: &LoadedFile, err: anyhow::Error) -> ApiError {
    tracing::warn!(
        rel_path = file.rel_path,
        error = format!("{err:#}"),
        "couldn't get the video's fonts"
    );
    ApiError::new(StatusCode::BAD_GATEWAY, "reading the video's fonts failed")
}

#[cfg(test)]
mod tests;
