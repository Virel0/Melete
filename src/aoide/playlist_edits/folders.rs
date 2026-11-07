//! Folders, the tree a user files their playlists in, each user's own: making one, renaming
//! it, deleting it, and moving a playlist into one or out to the top. Written as a device of
//! theirs writes them (iOS `Playlists.swift`, Linux `aoide-store/src/playlists/folders.rs`):
//! a folder's row is `id`, `name`, `parentId` (none at the top), `sortIndex`, and the usual
//! metadata; it merges field by field, as a playlist does. Deleting a folder deletes no
//! playlist: what was in it moves up to where the folder was.

use super::*;

/// A user's live folders as their devices hold them, by id: each row and its field stamps.
///
/// # Errors
///
/// Fails when the database does.
pub async fn folders(
    pool: &SqlitePool,
    user_id: i64,
) -> sqlx::Result<HashMap<String, (Row, HashMap<String, f64>)>> {
    let ops: Vec<(String, String)> = sqlx::query_as(
        "SELECT operation, payload FROM aoide_ops WHERE user_id = ? AND entity = 'folders' ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(
        playlists::fold_stamped(true, ops.iter().map(|(o, p)| (o.as_str(), p.as_str())))
            .into_iter()
            .filter(|(_, (row, _))| !playlists::deleted(row))
            .collect(),
    )
}

/// The user's folder `id` and the editor timed after its last change.
async fn own_folder<'a>(
    pool: &SqlitePool,
    editor: &Editor<'a>,
    id: &str,
) -> Result<(Row, HashMap<String, f64>, Editor<'a>), EditError> {
    let (row, stamps) = folders(pool, editor.user_id)
        .await?
        .remove(id)
        .ok_or(EditError::NoSuchFolder)?;
    let latest = row.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
    let editor = Editor {
        now: editor.now.max(latest.saturating_add(1)),
        ..*editor
    };
    Ok((row, stamps, editor))
}

/// Writes a folder's row, stamping the fields that changed from `held` (all of them for a new
/// one); a delete stamps none.
async fn write_folder(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    held: Option<(&Row, &HashMap<String, f64>)>,
    mut row: Row,
    deleted: bool,
) -> Result<(), EditError> {
    let mut stamps = held.map(|(_, s)| held_stamps(s)).unwrap_or_default();
    if !deleted {
        for (key, value) in &row {
            if held.is_none_or(|(old, _)| old.get(key) != Some(value)) {
                stamps.insert(key.clone(), editor.now);
            }
        }
    }
    stamp_row(&mut row, editor, deleted);
    row.insert("fieldUpdatedAt".into(), stamps_value(&stamps));
    let change = Change {
        entity: "folders",
        delete: deleted,
        row,
    };
    writing::append(
        pool,
        editor.user_id,
        editor.device,
        vec![change],
        editor.now,
    )
    .await?;
    Ok(())
}

/// A new folder named `name`, in folder `parent` or at the top, after every other; its id.
///
/// # Errors
///
/// [`EditError::NoSuchFolder`] when `parent` isn't one of the user's folders, or a write
/// error.
pub async fn create_folder(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    name: &str,
    parent: Option<&str>,
) -> Result<String, EditError> {
    let held = folders(pool, editor.user_id).await?;
    if parent.is_some_and(|p| !held.contains_key(p)) {
        return Err(EditError::NoSuchFolder);
    }
    let last = held
        .values()
        .filter_map(|(row, _)| row.get("sortIndex")?.as_str())
        .max();
    let id = writing::new_id()?;
    let mut row = Row::new();
    row.insert("id".into(), id.clone().into());
    row.insert("name".into(), name.into());
    if let Some(parent) = parent {
        row.insert("parentId".into(), parent.into());
    }
    row.insert(
        "sortIndex".into(),
        fractional_index::between(last, None).into(),
    );
    write_folder(pool, editor, None, row, false).await?;
    Ok(id)
}

/// Renames the user's folder `id`.
///
/// # Errors
///
/// [`EditError::NoSuchFolder`], or a write error.
pub async fn rename_folder(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    name: &str,
) -> Result<(), EditError> {
    let (row, stamps, editor) = own_folder(pool, editor, id).await?;
    if row.get("name").and_then(Value::as_str) == Some(name) {
        return Ok(());
    }
    let mut renamed = row.clone();
    renamed.insert("name".into(), name.into());
    write_folder(pool, &editor, Some((&row, &stamps)), renamed, false).await
}

/// Deletes the user's folder `id`: the folders and the user's own playlists in it move up to
/// where it was, then it goes.
///
/// # Errors
///
/// [`EditError::NoSuchFolder`], or a write error.
pub async fn delete_folder(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
) -> Result<(), EditError> {
    let (row, stamps, editor) = own_folder(pool, editor, id).await?;
    let parent = row.get("parentId").and_then(Value::as_str);
    let inside = |r: &Row, key: &str| r.get(key).and_then(Value::as_str) == Some(id);
    let set_parent = |r: &mut Row, key: &str| match parent {
        Some(parent) => {
            r.insert(key.into(), parent.into());
        }
        None => {
            r.remove(key);
        }
    };
    for (child, child_stamps) in folders(pool, editor.user_id).await?.values() {
        if inside(child, "parentId") {
            let mut moved = child.clone();
            set_parent(&mut moved, "parentId");
            write_folder(pool, &editor, Some((child, child_stamps)), moved, false).await?;
        }
    }
    for list in playlists::visible(pool, editor.user_id).await? {
        if list.owner_id == editor.user_id && inside(&list.row, "folderId") {
            let (list, editor) = prepared(list, &editor);
            let mut moved = list.row.clone();
            set_parent(&mut moved, "folderId");
            save_row(pool, &editor, &list, moved).await?;
        }
    }
    write_folder(pool, &editor, Some((&row, &stamps)), row.clone(), true).await
}

/// Moves the user's own playlist `id` (hand-made or smart) into folder `folder`, or out to the top with none, at
/// the end of where it goes, as a device files one.
///
/// # Errors
///
/// When it isn't the user's own playlist, [`EditError::NoSuchFolder`], or a write error.
pub async fn file_playlist(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    folder: Option<&str>,
) -> Result<(), EditError> {
    let (playlist, editor) = owned(pool, editor, id).await?;
    if folder.is_some_and(|f| playlist.row.get("folderId").and_then(Value::as_str) == Some(f))
        || (folder.is_none() && !playlist.row.contains_key("folderId"))
    {
        return Ok(());
    }
    if let Some(folder) = folder
        && !folders(pool, editor.user_id).await?.contains_key(folder)
    {
        return Err(EditError::NoSuchFolder);
    }
    let last = playlists::visible(pool, editor.user_id)
        .await?
        .iter()
        .filter_map(|p| p.row.get("sortIndex")?.as_str().map(str::to_owned))
        .max();
    let mut row = playlist.row.clone();
    match folder {
        Some(folder) => {
            row.insert("folderId".into(), folder.into());
        }
        None => {
            row.remove("folderId");
        }
    }
    row.insert(
        "sortIndex".into(),
        fractional_index::between(last.as_deref(), None).into(),
    );
    save_row(pool, &editor, &playlist, row).await
}
