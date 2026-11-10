//! A kept list as it's served: what's still there to offer for the listener now.

use std::collections::HashSet;

use super::picking::{by_artists, by_recording, left_out};
use super::*;
use crate::search_keys::fold;

/// The kept list's songs still to offer, in order: those to play still present for `access`
/// and not heard lately, finished often or flagged by `user` since; those to request still not
/// in the library for them.
pub(super) async fn still_there(
    pool: &SqlitePool,
    user: i64,
    access: &Access,
    items: Vec<Item>,
    now: i64,
) -> anyhow::Result<Vec<Item>> {
    let flagged = track_ids(crate::aoide::listening::not_interested(pool, user).await?);
    let left_out = left_out(pool, user, &flagged, now).await?;
    let tracks: Vec<i64> = items.iter().filter_map(|i| i.track_id).collect();
    let present: HashSet<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT t.id FROM music_tracks t WHERE t.missing_since IS NULL AND {}
           AND t.id IN (SELECT value FROM json_each(?))",
        access.track_sql("t")
    )))
    .bind(serde_json::to_string(&tracks)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    let songs: Vec<String> = items
        .iter()
        .filter_map(|i| Some(i.song.as_ref()?.mb_recording_id.clone()))
        .collect();
    let arrived = by_recording(pool, access, &songs).await?;
    // Or arrived as another recording: one of its artists' tracks with its title, folded, as
    // a list counts one owned.
    let credited: HashSet<&str> = items
        .iter()
        .filter_map(|i| i.song.as_ref())
        .flat_map(|s| s.mb_artist_ids.iter().map(String::as_str))
        .collect();
    let titled: HashSet<(String, String)> = by_artists(pool, access, &credited)
        .await?
        .into_iter()
        .map(|(_, title, artist)| (artist, fold(&title)))
        .collect();
    let came = |song: &crate::for_you::Song| {
        arrived.contains_key(&song.mb_recording_id)
            || song
                .mb_artist_ids
                .iter()
                .any(|a| titled.contains(&(a.clone(), fold(&song.title))))
    };
    Ok(items
        .into_iter()
        .filter(|i| match (&i.track_id, &i.song) {
            (Some(track), _) => present.contains(track) && !left_out.contains(track),
            (None, Some(song)) => !came(song),
            (None, None) => false,
        })
        .collect())
}
