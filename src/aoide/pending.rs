//! Playlist items for songs that aren't in the library yet. Such an item is a `playlist_items`
//! row like any other, but with no track id: in place of `jellyfinId` and `contentKey` it has
//! `pending`, naming the song and the request that will bring it:
//!
//! `{"requestId": int|null, "title": str, "artist": str|null, "album": str|null,
//!   "durationMs": int|null, "why": str|null}`
//!
//! `requestId` is null for a song nobody could ask for (MusicBrainz had nothing that fits);
//! `why` then says so, and the item stays pending. Once a requested song is in the library,
//! Melete swaps its track in, in the sync log, so every device just syncs it (`resolving`).

use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::playlists::Row;

pub(crate) mod by_name;
mod resolving;

pub use resolving::*;

/// A song an item holds until it's in the library.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pending {
    /// The song request that will bring it; none when it couldn't be asked for.
    pub request_id: Option<i64>,
    /// Its title.
    pub title: String,
    /// Its artists, as the list it came from names them.
    pub artist: Option<String>,
    /// Its album, when known.
    pub album: Option<String>,
    /// Its length in milliseconds, when known.
    pub duration_ms: Option<i64>,
    /// Why it couldn't be asked for; none when it was.
    pub why: Option<String>,
}

impl Pending {
    /// As an item's `pending` carries it: every field there, null when unknown.
    pub fn to_value(&self) -> Value {
        json!({
            "requestId": self.request_id,
            "title": self.title,
            "artist": self.artist,
            "album": self.album,
            "durationMs": self.duration_ms,
            "why": self.why,
        })
    }

    /// The song an item holds pending: none when it names a track, or its `pending` isn't an
    /// object with a title.
    pub fn of(item: &Row) -> Option<Self> {
        if has_track(item) {
            return None;
        }
        let pending = item.get("pending")?.as_object()?;
        let text = |key: &str| {
            pending
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            request_id: pending.get("requestId").and_then(Value::as_i64),
            title: text("title")?,
            artist: text("artist"),
            album: text("album"),
            duration_ms: pending.get("durationMs").and_then(Value::as_i64),
            why: text("why"),
        })
    }
}

/// Whether an item names a track, by either of the names the apps have used.
fn has_track(item: &Row) -> bool {
    ["jellyfinId", "trackId"].iter().any(|key| {
        item.get(*key)
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
    })
}

/// How each of `requests` stands (requested, approved or declined), for those that their own
/// asker put in playlist `playlist`; any other, and any withdrawn, is left out. Only those are
/// shown on the playlist or swapped in, so a request id someone puts in a playlist tells them
/// nothing about another user's request, even in a playlist shared with that user.
///
/// # Errors
///
/// Fails when the database does.
pub async fn vouched(
    pool: &SqlitePool,
    playlist: &str,
    requests: &[i64],
) -> sqlx::Result<HashMap<i64, String>> {
    if requests.is_empty() {
        return Ok(HashMap::new());
    }
    let found: Vec<(i64, String)> = sqlx::query_as(
        "SELECT r.id, r.state FROM requests r
         WHERE r.id IN (SELECT value FROM json_each(?2)) AND EXISTS (
             SELECT 1 FROM aoide_ops o
             WHERE o.entity = 'playlist_items' AND o.playlist_id = ?1 AND o.user_id = r.user_id
               AND json_extract(o.payload, '$.pending.requestId') = r.id)",
    )
    .bind(playlist)
    .bind(Value::from(requests.to_vec()).to_string())
    .fetch_all(pool)
    .await?;
    Ok(found.into_iter().collect())
}

#[cfg(test)]
mod tests;
