//! Swapping a song's track in for the items pending on its request, once the library has it:
//! a track whose MusicBrainz recording (from its tags, else from matching) is the request's,
//! else the one track the request's own song download put in the library, whatever recording
//! it's tagged with (MusicBrainz often lists a song several times).
//! Each item is written again with its own id, playlist and place, now naming the track, as
//! the playlist's owner from Melete's own device, timed just after the item's own last write,
//! so an edit a device made later (even one it sends later, from offline) still wins; an item
//! still pending after that is swapped on the next run. An item swapped is no longer pending,
//! so running this again writes nothing; an item a device has since deleted isn't in the
//! playlist any more and isn't written, and one moved keeps the place it was moved to. Only a
//! request its own asker put in the playlist counts (`pending::vouched`).

use super::*;
use crate::aoide::playlist_edits::{self, EditError, Editor};
use crate::aoide::writing::{self, MELETE_DEVICE, TrackLink};
use crate::db::now_ms;

/// The track each request that brought its song brought.
pub type Arrived = HashMap<i64, TrackLink>;

/// The items still waiting on a song request, in every playlist or only `playlist`: each
/// playlist with its owner and the request. An item counts as it stands now, by its latest
/// write (on a tie, a pending one, so none is passed over); one swapped or deleted since
/// doesn't. Only the log's pending writes are read (by their own index), then each such
/// item's writes.
async fn waiting(
    pool: &SqlitePool,
    playlist: Option<&str>,
) -> sqlx::Result<Vec<(String, i64, i64)>> {
    sqlx::query_as(
        "WITH named AS (
             SELECT DISTINCT entity_id FROM aoide_ops INDEXED BY aoide_ops_by_pending_request
             WHERE entity = 'playlist_items' AND (?1 IS NULL OR playlist_id = ?1)
               AND json_extract(payload, '$.pending.requestId') IS NOT NULL),
         latest AS (
             SELECT o.playlist_id, o.operation, o.payload,
                 row_number() OVER (PARTITION BY o.entity_id
                     ORDER BY json_extract(o.payload, '$.updatedAt') DESC,
                         json_extract(o.payload, '$.pending.requestId') IS NULL, o.seq DESC) AS n
             FROM aoide_ops o JOIN named ON named.entity_id = o.entity_id
             WHERE o.entity = 'playlist_items')
         SELECT DISTINCT l.playlist_id, w.owner_user_id,
             json_extract(l.payload, '$.pending.requestId')
         FROM latest l JOIN aoide_playlist_owners w ON w.playlist_id = l.playlist_id
         WHERE l.n = 1 AND l.operation = 'upsert'
           AND json_type(l.payload, '$.pending.requestId') = 'integer'
           AND coalesce(json_extract(l.payload, '$.deleted'), 0) = 0
           AND coalesce(json_extract(l.payload, '$.jellyfinId'), '') = ''
           AND coalesce(json_extract(l.payload, '$.trackId'), '') = ''",
    )
    .bind(playlist)
    .fetch_all(pool)
    .await
}

/// Which of `requests` brought their song: each as (request, the track). A track of the
/// request's recording comes first, the lowest id when several are; else the track its wanted
/// song's imported downloads placed, by its path, when they placed exactly one.
async fn arrived(pool: &SqlitePool, requests: &[i64]) -> sqlx::Result<Vec<(i64, i64)>> {
    sqlx::query_as(
        "WITH asked AS (
             SELECT id, mb_recording_id, wanted_song_id FROM requests
             WHERE kind = 'song' AND id IN (SELECT value FROM json_each(?))),
         by_recording AS (
             SELECT a.id, min(t.id) AS track FROM asked a
             JOIN music_tracks t ON t.missing_since IS NULL
                  AND coalesce(t.mb_recording_id, t.match_recording_id) = +a.mb_recording_id
             GROUP BY a.id),
         by_download AS (
             SELECT a.id, min(t.id) AS track FROM asked a
             JOIN downloads d ON d.wanted_song_id = a.wanted_song_id AND d.state = 'imported'
             JOIN json_each(d.placed) p
             JOIN libraries l
                  ON substr(p.value, 1, length(rtrim(l.path, '/')) + 1) = rtrim(l.path, '/') || '/'
             JOIN music_tracks t ON t.library_id = l.id AND t.missing_since IS NULL
                  AND t.rel_path = substr(p.value, length(rtrim(l.path, '/')) + 2)
             GROUP BY a.id
             HAVING count(DISTINCT t.id) = 1)
         SELECT a.id, coalesce(r.track, d.track) FROM asked a
         LEFT JOIN by_recording r ON r.id = a.id
         LEFT JOIN by_download d ON d.id = a.id
         WHERE coalesce(r.track, d.track) IS NOT NULL",
    )
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
    let waiting = waiting(pool, playlist).await?;
    let requests: Vec<i64> = waiting
        .iter()
        .map(|(_, _, request)| *request)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if requests.is_empty() {
        return Ok(0);
    }
    let mut found = Arrived::new();
    for (request, track) in arrived(pool, &requests).await? {
        if let Some(link) = writing::track_link(pool, track).await? {
            found.insert(request, link);
        }
    }
    let holding: std::collections::BTreeSet<(String, i64)> = waiting
        .into_iter()
        .filter(|(_, _, request)| found.contains_key(request))
        .map(|(playlist, owner, _)| (playlist, owner))
        .collect();
    let mut swapped = 0;
    for (playlist, owner) in holding {
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
