//! Swapping a song's track in for the items pending on its request, once the library has it:
//! a track whose MusicBrainz recording (from its tags, else from matching) is the request's.
//! Each item is written again with its own id, playlist and place, now naming the track, as
//! the playlist's owner from Melete's own device, timed after everything the playlist holds so
//! it wins. An item swapped is no longer pending, so running this again writes nothing; an
//! item a device has since deleted isn't in the playlist any more and isn't written, and one
//! moved keeps the place it was moved to.

use super::*;
use crate::aoide::playlist_edits::{self, EditError, Editor};
use crate::aoide::writing::{self, MELETE_DEVICE, TrackLink};
use crate::db::now_ms;

/// What each request that brought its song brought: who asked for it, and the track.
pub type Arrived = HashMap<i64, (i64, TrackLink)>;

/// Songs that pending items wait for and the library now has, in every playlist or only
/// `playlist`: each as (request, who asked, the track), the lowest track id when several fit.
async fn arrived(pool: &SqlitePool, playlist: Option<&str>) -> sqlx::Result<Vec<(i64, i64, i64)>> {
    sqlx::query_as(
        "SELECT r.id, r.user_id, min(t.id) FROM requests r
         JOIN music_tracks t ON t.missing_since IS NULL
              AND coalesce(t.mb_recording_id, t.match_recording_id) = r.mb_recording_id
         WHERE r.kind = 'song' AND r.id IN (
             SELECT json_extract(payload, '$.pending.requestId') FROM aoide_ops
             WHERE entity = 'playlist_items' AND (?1 IS NULL OR playlist_id = ?1)
               AND json_type(payload, '$.pending.requestId') = 'integer')
         GROUP BY r.id",
    )
    .bind(playlist)
    .fetch_all(pool)
    .await
}

/// The playlists with an item that was ever pending on one of `requests`, and their owners.
async fn holding(
    pool: &SqlitePool,
    playlist: Option<&str>,
    requests: &[i64],
) -> sqlx::Result<Vec<(String, i64)>> {
    sqlx::query_as(
        "SELECT DISTINCT o.playlist_id, w.owner_user_id FROM aoide_ops o
         JOIN aoide_playlist_owners w ON w.playlist_id = o.playlist_id
         WHERE o.entity = 'playlist_items' AND (?1 IS NULL OR o.playlist_id = ?1)
           AND json_extract(o.payload, '$.pending.requestId') IN (SELECT value FROM json_each(?2))",
    )
    .bind(playlist)
    .bind(Value::from(requests.to_vec()).to_string())
    .fetch_all(pool)
    .await
}

/// Swaps each song the library now has in for the items still pending on its request, in
/// every playlist or only `playlist`. Returns how many items it wrote. A playlist deleted,
/// smart, or no longer its owner's is passed over.
///
/// # Errors
///
/// A database or write error.
pub async fn resolve(pool: &SqlitePool, playlist: Option<&str>) -> Result<usize, EditError> {
    let mut found = Arrived::new();
    for (request, asker, track) in arrived(pool, playlist).await? {
        if let Some(link) = writing::track_link(pool, track).await? {
            found.insert(request, (asker, link));
        }
    }
    if found.is_empty() {
        return Ok(0);
    }
    let requests: Vec<i64> = found.keys().copied().collect();
    let mut swapped = 0;
    for (playlist, owner) in holding(pool, playlist, &requests).await? {
        let editor = Editor {
            user_id: owner,
            device: MELETE_DEVICE,
            now: now_ms(),
        };
        match playlist_edits::swap_in(pool, &editor, &playlist, &found).await {
            Ok(written) => swapped += written,
            Err(EditError::NoSuchPlaylist | EditError::NotOwner | EditError::Smart) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(swapped)
}
