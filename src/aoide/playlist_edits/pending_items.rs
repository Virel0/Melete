//! Items that hold a song not in the library yet (`aoide::pending`): adding them, and writing
//! one again as its track once the song arrives.

use std::collections::HashSet;

use super::*;
use crate::aoide::pending::{self, Arrived, Pending};

/// What an item holds: a track in the library, or a song pending until it is.
pub enum Holds {
    /// A track, as a device links one.
    Track(writing::TrackLink),
    /// A song not in the library yet.
    Pending(Pending),
}

/// An item to add: its id, and what it holds.
pub struct NewItem {
    /// The item's id, as [`writing::new_id`] or [`writing::id_from`] spells one.
    pub id: String,
    /// What it holds.
    pub holds: Holds,
}

impl NewItem {
    /// Its row in playlist `playlist` at `position`, before the metadata a write stamps.
    pub(super) fn row(self, playlist: &str, position: String) -> Row {
        let mut row = Row::new();
        row.insert("id".into(), self.id.into());
        row.insert("playlistId".into(), playlist.into());
        match self.holds {
            Holds::Track(link) => {
                row.insert("jellyfinId".into(), link.jellyfin_id.into());
                row.insert("contentKey".into(), link.content_key.into());
            }
            Holds::Pending(song) => {
                row.insert("pending".into(), song.to_value());
            }
        }
        row.insert("position".into(), position.into());
        row
    }
}

/// Adds `items` in order, the first at `index` (counted in the playlist as it is), or at the
/// end. An item whose id the playlist holds already is left where it is, so adding the same
/// items again adds nothing.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist, or a write error.
pub async fn add_items(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    items: Vec<NewItem>,
    index: Option<usize>,
) -> Result<(), EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let held: HashSet<&str> = playlist
        .items
        .iter()
        .filter_map(|i| i.get("id").and_then(Value::as_str))
        .collect();
    let items: Vec<NewItem> = items
        .into_iter()
        .filter(|i| !held.contains(i.id.as_str()))
        .collect();
    if items.is_empty() {
        return Ok(());
    }
    place(pool, &editor, &playlist, items, index).await
}

/// Writes each item still pending on a request in `arrived` as the track that request
/// brought: its own id, playlist and place, the track's id and content key in place of
/// `pending`. Only requests made by those who may edit the playlist count. Returns how many
/// items it wrote; none once they're all swapped.
///
/// # Errors
///
/// When it isn't the user's own hand-made playlist, or a write error.
pub async fn swap_in(
    pool: &SqlitePool,
    editor: &Editor<'_>,
    id: &str,
    arrived: &Arrived,
) -> Result<usize, EditError> {
    let (playlist, editor) = own(pool, editor, id).await?;
    let askers = pending::askers(pool, id, playlist.owner_id).await?;
    let changes: Vec<Change> = playlist
        .items
        .iter()
        .filter_map(|item| {
            let request = Pending::of(item)?.request_id?;
            let (asker, link) = arrived.get(&request)?;
            if !askers.contains(asker) {
                return None;
            }
            let mut row = item.clone();
            row.remove("pending");
            row.insert("jellyfinId".into(), link.jellyfin_id.clone().into());
            row.insert("contentKey".into(), link.content_key.clone().into());
            Some(item_change(&row, &editor, false))
        })
        .collect();
    let written = changes.len();
    if written > 0 {
        writing::append(pool, editor.user_id, editor.device, changes, editor.now).await?;
    }
    Ok(written)
}
