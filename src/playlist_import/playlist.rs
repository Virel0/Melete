//! The Aoide playlist an import fills, when it makes one. Each track goes in as it's handled,
//! at the end, so the playlist keeps the import's order and fills in while the import runs. A
//! track the library has already (matched as `/aoide/match` matches) goes in as it is and isn't
//! asked for; one asked for, or asked for already, goes in pending on its request; one nobody
//! could ask for goes in pending on none, with why. Items are written as Melete's own device,
//! each with an id made from the playlist and the track's place, so a track handled again after
//! a restart is the same item, not a second one. A pending one whose song is in the library
//! already is swapped for its track at once; the rest are swapped as their songs arrive.

use super::*;
use crate::aoide::matcher::{Imported, LibraryTrack, Matcher};
use crate::aoide::pending::{self, Pending};
use crate::aoide::playlist_edits::{self, EditError, Editor, Holds, NewItem};
use crate::aoide::writing::{self, MELETE_DEVICE, TrackLink};
use crate::db::now_ms;

/// The playlist an import fills, and the library its tracks are looked for in.
pub(super) struct Filling {
    /// The playlist's id.
    pub(super) playlist: String,
    /// Every present track, as matching sees it.
    pub(super) library: Vec<LibraryTrack>,
}

/// The playlist import `id` fills, with the library as it is now; none when it makes none.
pub(super) async fn filling(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Filling>> {
    let playlist: Option<String> =
        sqlx::query_scalar("SELECT playlist_id FROM playlist_imports WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let Some(playlist) = playlist else {
        return Ok(None);
    };
    let library = crate::aoide::matcher::present_tracks(pool).await?;
    Ok(Some(Filling { playlist, library }))
}

/// The library's track for `track`, by the rules `/aoide/match` uses; none when it has none.
pub(super) async fn in_library(
    pool: &SqlitePool,
    matcher: &Matcher<'_>,
    track: &Track,
) -> anyhow::Result<Option<TrackLink>> {
    let row = Imported {
        title: Some(track.title.clone()),
        artists: track.artists.clone(),
        duration_ms: track.length_ms,
        isrc: track.isrc.clone(),
    };
    let Some(id) = matcher.find(&row).and_then(|m| m.track.id.parse().ok()) else {
        return Ok(None);
    };
    Ok(writing::track_link(pool, id).await?)
}

/// Puts one handled track in the playlist, at the end. A failure is logged and the import goes
/// on: its requests stand without the playlist. One deleted, or made smart, takes no more.
pub(super) async fn put_in(
    pool: &SqlitePool,
    user_id: i64,
    playlist: &str,
    position: i64,
    track: &Track,
    handled: &Handled,
) {
    match write(pool, user_id, playlist, position, track, handled).await {
        Ok(())
        | Err(
            EditError::NoSuchPlaylist
            | EditError::NotOwner
            | EditError::ListenOnly
            | EditError::Smart,
        ) => {}
        Err(err) => tracing::warn!(
            playlist,
            position,
            error = ?err,
            "couldn't put an imported track in its playlist"
        ),
    }
}

async fn write(
    pool: &SqlitePool,
    user_id: i64,
    playlist: &str,
    position: i64,
    track: &Track,
    handled: &Handled,
) -> Result<(), EditError> {
    let holds = match &handled.track {
        Some(link) => Holds::Track(link.clone()),
        None => Holds::Pending(Pending {
            request_id: handled.request,
            title: track.title.clone(),
            artist: (!track.artists.is_empty()).then(|| track.artists.join(", ")),
            album: track.album.clone(),
            duration_ms: track.length_ms,
            why: handled.why.clone(),
        }),
    };
    let item = NewItem {
        id: writing::id_from(&format!("{playlist}/{position}")),
        holds,
    };
    let editor = Editor {
        user_id,
        device: MELETE_DEVICE,
        now: now_ms(),
    };
    playlist_edits::add_items(pool, &editor, playlist, vec![item], None).await?;
    if handled.request.is_some() {
        pending::resolve(pool, Some(playlist)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
