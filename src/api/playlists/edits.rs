//! Creating, renaming and deleting one's own playlists and adding, removing and moving their
//! tracks, each written into the sync log as an Aoide device of the user's would write it, as
//! the session the request came with. Answers with the playlist as `GET` gives it after.

use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::{Deserialize, Deserializer};

use super::*;
use crate::aoide::library;
use crate::aoide::playlist_edits::{self, EditError, Editor};
use crate::aoide::writing::session_device;
use crate::db::now_ms;

/// Longest name, in characters.
const MAX_NAME: usize = 200;
/// Longest notes, in characters.
const MAX_NOTES: usize = 2000;
/// Most tracks added at once.
const MAX_ADD: usize = 500;

fn failed(err: EditError) -> ApiError {
    let (status, message) = match err {
        EditError::NoSuchPlaylist => (StatusCode::NOT_FOUND, "no such playlist".into()),
        EditError::NotOwner => (
            StatusCode::FORBIDDEN,
            "only its owner edits a playlist here".into(),
        ),
        EditError::Smart => (
            StatusCode::CONFLICT,
            "a smart playlist is edited on the devices, where its rules are worked out".into(),
        ),
        EditError::NoSuchItem => (StatusCode::NOT_FOUND, "no such item".into()),
        EditError::NoSuchTrack(id) => (StatusCode::NOT_FOUND, format!("no such track: {id}")),
        EditError::NoGap => (
            StatusCode::CONFLICT,
            "the playlist's order leaves no room there; move a neighbour on a device first".into(),
        ),
        EditError::Write(err) => return super::super::music_likes::write_failed(err),
    };
    ApiError::new(status, message)
}

fn editor<'a>(current: &CurrentUser, device: &'a str) -> Editor<'a> {
    Editor {
        user_id: current.user.id,
        device,
        now: now_ms(),
    }
}

/// A name or notes, trimmed and checked.
fn text(value: &str, max: usize, what: &str, empty: bool) -> Result<String, ApiError> {
    let value = value.trim();
    if (!empty && value.is_empty()) || value.chars().count() > max {
        let least = if empty { 0 } else { 1 };
        return Err(super::super::bad_request(&format!(
            "{what} is {least} to {max} characters"
        )));
    }
    Ok(value.to_owned())
}

/// Present and null, told apart from absent.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewPlaylist {
    name: String,
    notes: Option<String>,
}

pub(super) async fn create(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<NewPlaylist>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = text(&body.name, MAX_NAME, "name", false)?;
    let notes = match &body.notes {
        Some(notes) => Some(text(notes, MAX_NOTES, "notes", true)?).filter(|n| !n.is_empty()),
        None => None,
    };
    let device = session_device(current.token_id);
    let editor = editor(&current, &device);
    let id = playlist_edits::create(&pool, &editor, &name, notes.as_deref())
        .await
        .map_err(failed)?;
    Ok((
        StatusCode::CREATED,
        Json(detail(&pool, &current.access, current.user.id, &id).await?),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    name: Option<String>,
    #[serde(default, deserialize_with = "present")]
    notes: Option<Option<String>>,
}

pub(super) async fn update(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<Change>,
) -> Result<Json<Value>, ApiError> {
    let name = match &body.name {
        Some(name) => Some(text(name, MAX_NAME, "name", false)?),
        None => None,
    };
    let notes = match &body.notes {
        Some(Some(notes)) => Some(Some(text(notes, MAX_NOTES, "notes", true)?)),
        Some(None) => Some(None),
        None => None,
    };
    // Empty notes are no notes.
    let notes = notes.map(|n| n.filter(|n| !n.is_empty()));
    let device = session_device(current.token_id);
    let editor = editor(&current, &device);
    playlist_edits::rename(
        &pool,
        &editor,
        &id,
        name.as_deref(),
        notes.as_ref().map(Option::as_deref),
    )
    .await
    .map_err(failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

/// Whether the bytes start as an image of that type does.
fn looks_like(mime: &str, bytes: &[u8]) -> bool {
    match mime {
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}

/// Sets the playlist's picture: the image is stored as the devices store theirs, under the
/// SHA-256 of its bytes, then named in the playlist.
pub(super) async fn set_image(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let mime = super::super::aoide_library::image_type(&headers, &body)?;
    if !looks_like(&mime, &body) {
        return Err(super::super::bad_request(
            "the file isn't the JPEG, PNG or WebP image its Content-Type says",
        ));
    }
    let device = session_device(current.token_id);
    let editor = editor(&current, &device);
    playlist_edits::editable(&pool, &editor, &id)
        .await
        .map_err(failed)?;
    let hash = library::sha256_hex(&body);
    library::store_image(&pool, current.user.id, &hash, &mime, &body, editor.now).await?;
    playlist_edits::set_image(&pool, &editor, &id, Some(&hash))
        .await
        .map_err(failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

/// Clears the playlist's picture; the image stays stored, as unnamed artwork does until it's
/// reclaimed.
pub(super) async fn remove_image(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let device = session_device(current.token_id);
    let editor = editor(&current, &device);
    playlist_edits::set_image(&pool, &editor, &id, None)
        .await
        .map_err(failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

pub(super) async fn remove_playlist(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let device = session_device(current.token_id);
    playlist_edits::delete(&pool, &editor(&current, &device), &id)
        .await
        .map_err(failed)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewItems {
    track_ids: Vec<i64>,
    index: Option<usize>,
}

pub(super) async fn add_items(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<NewItems>,
) -> Result<Json<Value>, ApiError> {
    if body.track_ids.is_empty() || body.track_ids.len() > MAX_ADD {
        return Err(super::super::bad_request(&format!(
            "track_ids is 1 to {MAX_ADD} track ids"
        )));
    }
    for &track in &body.track_ids {
        if !current.access.sees_track(&pool, track).await? {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                format!("no such track: {track}"),
            ));
        }
    }
    let device = session_device(current.token_id);
    let editor = editor(&current, &device);
    playlist_edits::add(&pool, &editor, &id, &body.track_ids, body.index)
        .await
        .map_err(failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

pub(super) async fn remove_item(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path((id, item)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let device = session_device(current.token_id);
    playlist_edits::remove(&pool, &editor(&current, &device), &id, &item)
        .await
        .map_err(failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Move {
    index: usize,
}

pub(super) async fn move_item(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path((id, item)): Path<(String, String)>,
    Json(body): Json<Move>,
) -> Result<Json<Value>, ApiError> {
    let device = session_device(current.token_id);
    playlist_edits::reorder(&pool, &editor(&current, &device), &id, &item, body.index)
        .await
        .map_err(failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

#[cfg(test)]
mod tests;
