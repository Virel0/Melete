//! Playlist folders, each user's own: listing them, making one, renaming one and deleting one,
//! each written into the sync log as an Aoide device of the user's would write it, as the
//! session the request came with. A playlist is filed through `PUT /playlists/{id}`.

use serde::Deserialize;

use super::edits::{failed, text};
use super::*;
use crate::aoide::playlist_edits::{self, Editor};
use crate::aoide::writing::session_device;
use crate::db::now_ms;

/// Longest folder name, in characters.
const MAX_NAME: usize = 200;

/// The user's folders, in the order their devices keep them: each with its parent, none at
/// the top, and how many of the user's playlists are filed directly in it.
async fn folder_list(pool: &SqlitePool, me: i64) -> Result<Vec<Value>, ApiError> {
    let held = playlist_edits::folders(pool, me).await?;
    let owned = playlists::visible(pool, me).await?;
    let mut rows: Vec<(&String, &playlists::Row)> =
        held.iter().map(|(id, (row, _))| (id, row)).collect();
    let key = |row: &playlists::Row| str_of(row, "sortIndex").unwrap_or_default().to_owned();
    rows.sort_by(|a, b| key(a.1).cmp(&key(b.1)).then_with(|| a.0.cmp(b.0)));
    Ok(rows
        .into_iter()
        .map(|(id, row)| {
            let filed = owned
                .iter()
                .filter(|p| p.owner_id == me && str_of(&p.row, "folderId") == Some(id))
                .count();
            json!({
                "id": id,
                "name": str_of(row, "name").unwrap_or("Untitled folder"),
                // A parent that's gone leaves the folder at the top, as the devices show it.
                "parent_id": str_of(row, "parentId").filter(|p| held.contains_key(*p)),
                "playlists": filed,
            })
        })
        .collect())
}

/// The folder `id` as listed.
async fn one_folder(pool: &SqlitePool, me: i64, id: &str) -> Result<Value, ApiError> {
    folder_list(pool, me)
        .await?
        .into_iter()
        .find(|f| f["id"] == id)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such folder"))
}

pub(super) async fn list_folders(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Vec<Value>>, ApiError> {
    Ok(Json(folder_list(&pool, current.user.id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewFolder {
    name: String,
    parent_id: Option<String>,
}

pub(super) async fn create_folder(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<NewFolder>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let name = text(&body.name, MAX_NAME, "name", false)?;
    let device = session_device(current.token_id);
    let editor = Editor {
        user_id: current.user.id,
        device: &device,
        now: now_ms(),
    };
    let id = playlist_edits::create_folder(&pool, &editor, &name, body.parent_id.as_deref())
        .await
        .map_err(failed)?;
    Ok((
        StatusCode::CREATED,
        Json(one_folder(&pool, current.user.id, &id).await?),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FolderChange {
    name: String,
}

pub(super) async fn rename_folder(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<FolderChange>,
) -> Result<Json<Value>, ApiError> {
    let name = text(&body.name, MAX_NAME, "name", false)?;
    let device = session_device(current.token_id);
    let editor = Editor {
        user_id: current.user.id,
        device: &device,
        now: now_ms(),
    };
    playlist_edits::rename_folder(&pool, &editor, &id, &name)
        .await
        .map_err(failed)?;
    Ok(Json(one_folder(&pool, current.user.id, &id).await?))
}

pub(super) async fn delete_folder(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let device = session_device(current.token_id);
    let editor = Editor {
        user_id: current.user.id,
        device: &device,
        now: now_ms(),
    };
    playlist_edits::delete_folder(&pool, &editor, &id)
        .await
        .map_err(failed)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
