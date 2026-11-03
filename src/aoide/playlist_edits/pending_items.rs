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
/// end. An item whose id the playlist has ever held is left as it is, where it is or deleted,
/// so adding the same items again adds nothing and never brings back one deleted since.
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
    let held: HashSet<String> = sqlx::query_scalar(
        "SELECT DISTINCT entity_id FROM aoide_ops
         WHERE entity = 'playlist_items' AND playlist_id = ?",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let items: Vec<NewItem> = items
        .into_iter()
        .filter(|i| !held.contains(&i.id))
        .collect();
    if items.is_empty() {
        return Ok(());
    }
    place(pool, &editor, &playlist, items, index).await
}

/// Writes each item still pending on a request in `arrived` as the track that request
/// brought: its own id, playlist and place, the track's id and content key in place of
/// `pending`. Only requests their own asker put in the playlist count (`pending::vouched`).
/// Each is timed just after the item's own last write, so an edit a device made after that
/// wins even when it arrives later. Returns how many items it wrote; none once they're all
/// swapped.
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
    let waiting: Vec<(&Row, i64)> = playlist
        .items
        .iter()
        .filter_map(|item| Some((item, Pending::of(item)?.request_id?)))
        .filter(|(_, request)| arrived.contains_key(request))
        .collect();
    let asked: Vec<i64> = waiting.iter().map(|(_, request)| *request).collect();
    let vouched = pending::vouched(pool, id, &asked).await?;
    let changes: Vec<Change> = waiting
        .into_iter()
        .filter(|(_, request)| vouched.contains_key(request))
        .filter_map(|(item, request)| {
            let link = arrived.get(&request)?;
            let mut row = item.clone();
            row.remove("pending");
            row.insert("jellyfinId".into(), link.jellyfin_id.clone().into());
            row.insert("contentKey".into(), link.content_key.clone().into());
            let at = item.get("updatedAt").and_then(Value::as_i64).unwrap_or(0);
            let just_after = Editor {
                now: at.saturating_add(1),
                ..editor
            };
            Some(item_change(&row, &just_after, false))
        })
        .collect();
    let written = changes.len();
    if written > 0 {
        writing::append(pool, editor.user_id, editor.device, changes, editor.now).await?;
    }
    Ok(written)
}
