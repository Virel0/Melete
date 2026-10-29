//! Editing a playlist for apps that don't sync (the web UI), by writing into the owner's sync
//! log the ops an Aoide device of theirs would push for the same change: creating, renaming,
//! deleting, adding tracks, removing and moving them. Conflicts with edits made on devices
//! resolve as the log resolves any two devices' edits.
//!
//! Each write starts from the playlist as the owner's devices hold it (`playlists::visible`)
//! and sends the whole row after the change. A playlist's fields carry when each last changed
//! (`fieldUpdatedAt`): a change stamps the fields it changed, as a device does, so an edit to
//! another field elsewhere survives. Items have no such stamps; each is a row of its own, its
//! place a fractional key.
//!
//! Only a playlist's owner edits it here. Smart playlists aren't edited here at all: their
//! rules are worked out on the devices.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value};
use sqlx::SqlitePool;

use super::fractional_index;
use super::playlists::{self, Playlist, Row};
use super::writing::{self, Change, WriteError};

mod items;

pub use items::*;

/// Why an edit wasn't made.
#[derive(Debug)]
pub enum EditError {
    /// No such playlist the user can see.
    NoSuchPlaylist,
    /// The playlist is someone else's.
    NotOwner,
    /// It's a smart playlist, edited on the devices.
    Smart,
    /// No such item in the playlist.
    NoSuchItem,
    /// A track to add isn't in the library.
    NoSuchTrack(i64),
    /// The neighbours' keys leave no room between them.
    NoGap,
    /// Writing it failed.
    Write(WriteError),
}

impl From<WriteError> for EditError {
    fn from(err: WriteError) -> Self {
        Self::Write(err)
    }
}

impl From<sqlx::Error> for EditError {
    fn from(err: sqlx::Error) -> Self {
        Self::Write(WriteError::Database(err))
    }
}

/// Who is editing: the user, and the device their ops carry.
#[derive(Clone, Copy)]
pub struct Editor<'a> {
    /// The user.
    pub user_id: i64,
    /// The device name the ops carry, as `writing::session_device` gives it.
    pub device: &'a str,
    /// When, in milliseconds since the epoch.
    pub now: i64,
}

/// Whether a playlist's row says it's smart: true, 1 or "1".
pub fn is_smart(row: &Row) -> bool {
    match row.get("isSmart") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true"),
        _ => false,
    }
}

/// The user's own hand-made playlist `id`, as their devices hold it, and the editor with a
/// time after every change it holds: two edits within one millisecond would otherwise tie,
/// and a tie from one device is the same write, so the second would be lost.
async fn own<'a>(
    pool: &SqlitePool,
    editor: &Editor<'a>,
    id: &str,
) -> Result<(Playlist, Editor<'a>), EditError> {
    let playlist = playlists::visible(pool, editor.user_id)
        .await?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or(EditError::NoSuchPlaylist)?;
    if playlist.owner_id != editor.user_id {
        return Err(EditError::NotOwner);
    }
    if is_smart(&playlist.row) {
        return Err(EditError::Smart);
    }
    let mut playlist = playlist;
    // Fields every device's row has; an old log's row may lack them, and a row written
    // without them would be refused by the devices.
    playlist.row.entry("isSmart").or_insert(false.into());
    if !playlist.row.contains_key("sortIndex") {
        let key = fractional_index::between(None, None);
        playlist.row.insert("sortIndex".into(), key.into());
    }
    let latest = std::iter::once(&playlist.row)
        .chain(&playlist.items)
        .filter_map(|r| r.get("updatedAt")?.as_i64())
        .max()
        .unwrap_or(0);
    let editor = Editor {
        now: editor.now.max(latest.saturating_add(1)),
        ..*editor
    };
    Ok((playlist, editor))
}

/// Stamps as a payload carries them: whole milliseconds.
fn stamps_value(stamps: &BTreeMap<String, i64>) -> Value {
    Value::Object(
        stamps
            .iter()
            .map(|(k, v)| (k.clone(), (*v).into()))
            .collect(),
    )
}

/// A row's metadata after a change by `editor`.
fn stamp_row(row: &mut Row, editor: &Editor<'_>, deleted: bool) {
    row.insert("updatedAt".into(), editor.now.into());
    row.insert("deleted".into(), deleted.into());
    row.insert("originDevice".into(), editor.device.into());
}

/// A new hand-made playlist named `name`, after every other the user can see; its id.
///
/// # Errors
///
/// A write error.
pub async fn create(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    name: &str,
    notes: Option<&str>,
) -> Result<String, EditError> {
    let last = playlists::visible(pool, editor.user_id)
        .await?
        .iter()
        .filter_map(|p| p.row.get("sortIndex")?.as_str().map(str::to_owned))
        .max();
    let id = writing::new_id()?;
    let mut row = Row::new();
    row.insert("id".into(), id.clone().into());
    row.insert("name".into(), name.into());
    if let Some(notes) = notes {
        row.insert("notes".into(), notes.into());
    }
    row.insert("isSmart".into(), false.into());
    row.insert(
        "sortIndex".into(),
        fractional_index::between(last.as_deref(), None).into(),
    );
    stamp_row(&mut row, editor, false);
    // A new row's every field is new.
    let stamps: BTreeMap<String, i64> = row.keys().map(|k| (k.clone(), editor.now)).collect();
    row.insert("fieldUpdatedAt".into(), stamps_value(&stamps));
    let change = Change {
        entity: "playlists",
        delete: false,
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
    Ok(id)
}

/// Renames the playlist and sets or clears its notes; `None` leaves either as it is.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist, or a write error.
pub async fn rename(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    name: Option<&str>,
    notes: Option<Option<&str>>,
) -> Result<(), EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let editor = &editor;
    let mut row = playlist.row.clone();
    if let Some(name) = name {
        row.insert("name".into(), name.into());
    }
    match notes {
        Some(Some(notes)) => {
            row.insert("notes".into(), notes.into());
        }
        // Left out, as a device clears them: the newer row has none.
        Some(None) => {
            row.remove("notes");
        }
        None => {}
    }
    if row == playlist.row {
        return Ok(());
    }
    let mut stamps = held_stamps(&playlist.stamps);
    // The fields the change gave a new value, as a device compares the row it writes with
    // the one it held; a field cleared isn't one.
    for (key, value) in &row {
        if playlist.row.get(key) != Some(value) {
            stamps.insert(key.clone(), editor.now);
        }
    }
    write_playlist(pool, editor, row, &stamps, false).await
}

/// Deletes the playlist and its items: the items first, then the playlist, as a device does.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist, or a write error.
pub async fn delete(pool: &SqlitePool, editor: &Editor<'_>, id: &str) -> Result<(), EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let editor = &editor;
    let mut changes: Vec<Change> = playlist
        .items
        .iter()
        .map(|item| item_change(item, editor, true))
        .collect();
    // A delete changes no field, so it stamps none.
    let mut row = playlist.row.clone();
    stamp_row(&mut row, editor, true);
    row.insert(
        "fieldUpdatedAt".into(),
        stamps_value(&held_stamps(&playlist.stamps)),
    );
    changes.push(Change {
        entity: "playlists",
        delete: true,
        row,
    });
    writing::append(pool, editor.user_id, editor.device, changes, editor.now).await?;
    Ok(())
}

/// The stamps a playlist holds, as whole milliseconds.
fn held_stamps(stamps: &HashMap<String, f64>) -> BTreeMap<String, i64> {
    stamps
        .iter()
        .map(|(k, v)| (k.clone(), v.round() as i64))
        .collect()
}

async fn write_playlist(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    mut row: Row,
    stamps: &BTreeMap<String, i64>,
    deleted: bool,
) -> Result<(), EditError> {
    stamp_row(&mut row, editor, deleted);
    row.insert("fieldUpdatedAt".into(), stamps_value(stamps));
    let change = Change {
        entity: "playlists",
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

/// An item's row after a change by `editor`, as a change to write.
fn item_change(item: &Row, editor: &Editor<'_>, deleted: bool) -> Change {
    let mut row: Map<String, Value> = item.clone();
    stamp_row(&mut row, editor, deleted);
    Change {
        entity: "playlist_items",
        delete: deleted,
        row,
    }
}

#[cfg(test)]
mod tests;
