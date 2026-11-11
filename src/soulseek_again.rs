//! Soulseek again, a day on: who shares what changes as people come and go, so a wanted song
//! Soulseek had nothing for, or whose Soulseek downloads all failed (refused, banned, timed out,
//! changed since the search, as many times as the next source goes), and a monitored album whose
//! Soulseek downloads all failed, are searched for again [`AFTER`] their last search or download,
//! [`TIMES`] times at most. The best accepted result is grabbed as an approved request's is,
//! passing over the users a download of it already failed with; one that fails gets its next
//! sources as any does. A song also gets its try on YouTube in between, when that's on
//! (`song_youtube`). An album no one has had at all is left to RSS syncs and Search all missing.
//! A wanted song with no search on record at all is searched for too ([`never_searched`]).
//! Runs hourly, [`PER_RUN`] at most (and [`FIRST_PER_RUN`] first searches), one search at a
//! time, only with slskd set up. Nothing is fetched for a song the library has already.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::artist_search::{self, Sources};
use crate::db::unix_now;
use crate::progress::Step;

/// How long after a song's or album's last search or download it's searched for again.
pub const AFTER: i64 = 86_400;
/// How many times Soulseek is searched again for one song or album.
pub const TIMES: i64 = 3;
/// Searches one run makes at most.
pub const PER_RUN: usize = 10;
/// First searches one run makes at most, for songs Soulseek has no search of on record.
pub const FIRST_PER_RUN: usize = 60;
/// How long a song is wanted before it's searched for when it has no search on record, so
/// its request's own search isn't doubled.
pub const NEW_FOR: i64 = 3600;
/// How often a run is made, and how long after starting the first one is.
const EVERY: Duration = Duration::from_secs(3600);
const FIRST: Duration = Duration::from_secs(600);

/// One song or album to search for again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Again {
    Song(i64),
    Album(i64),
}

/// Searches again for as long as the server runs: a run an hourly, the first ten minutes in.
pub async fn work(pool: SqlitePool, sources: Sources) {
    tokio::time::sleep(FIRST).await;
    loop {
        match run(&pool, &sources, unix_now()).await {
            Ok(searched) if searched.is_empty() => {}
            Ok(searched) => tracing::info!(
                searched = searched.len(),
                "searched Soulseek again for what it couldn't give"
            ),
            Err(err) => tracing::warn!(
                error = format!("{err:#}"),
                "searching Soulseek again stopped for now"
            ),
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// One run: what's due, songs first, [`PER_RUN`] at most, each searched for again and its
/// best result grabbed. What was searched for.
pub async fn run(pool: &SqlitePool, sources: &Sources, now: i64) -> anyhow::Result<Vec<Again>> {
    let Some(slskd) = sources.slskd.as_deref() else {
        return Ok(Vec::new());
    };
    let mut again = due_songs(pool, now).await?;
    again.extend(due_albums(pool, now).await?);
    again.truncate(PER_RUN);
    let mut due = never_searched(pool, now).await?;
    due.extend(again);
    let client = crate::torznab::Client::new()?;
    let mut searched = Vec::new();
    for again in due {
        note(pool, again, now).await?;
        let (search, avoid) = match again {
            Again::Song(id) => (
                crate::song_search::search(pool, slskd, id).await?,
                crate::soulseek_downloads::failed_users(pool, None, Some(id)).await?,
            ),
            Again::Album(id) => (
                crate::album_search::search(pool, &client, Some(slskd), id).await?,
                crate::soulseek_downloads::failed_users(pool, Some(id), None).await?,
            ),
        };
        match search {
            Ok(search_id) => {
                let mut grabbed = Vec::new();
                artist_search::grab_best_except(pool, sources, search_id, &avoid, &mut grabbed)
                    .await?;
            }
            Err(refused) => tracing::info!(?again, why = %refused, "couldn't search again"),
        }
        searched.push(again);
    }
    Ok(searched)
}

/// Counts a search again for the song or album.
async fn note(pool: &SqlitePool, again: Again, now: i64) -> sqlx::Result<()> {
    let (song, album) = match again {
        Again::Song(id) => (Some(id), None),
        Again::Album(id) => (None, Some(id)),
    };
    let column = if song.is_some() {
        "wanted_song_id"
    } else {
        "wanted_album_id"
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO soulseek_again (wanted_song_id, wanted_album_id, times, last_at)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT ({column}) DO UPDATE SET times = times + 1, last_at = excluded.last_at"
    )))
    .bind(song)
    .bind(album)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Wanted songs due: Soulseek searched for them, no download of them but failed ones, nothing
/// for them or the album fetched for them for [`AFTER`], fewer than [`TIMES`] searches again,
/// and nothing on its way or in the library (from YouTube, say); the newest wanted first.
async fn due_songs(pool: &SqlitePool, now: i64) -> anyhow::Result<Vec<Again>> {
    let candidates: Vec<i64> = sqlx::query_scalar(
        "SELECT s.id FROM wanted_songs s
         LEFT JOIN soulseek_again a ON a.wanted_song_id = s.id
         WHERE EXISTS (SELECT 1 FROM searches q WHERE q.wanted_song_id = s.id)
           AND NOT EXISTS (SELECT 1 FROM downloads d
                           WHERE d.wanted_song_id = s.id AND d.state <> 'failed')
           AND (SELECT max(at) FROM (
                    SELECT searched_at AS at FROM searches WHERE wanted_song_id = s.id
                    UNION ALL
                    SELECT updated_at FROM downloads
                    WHERE wanted_song_id = s.id
                       OR (wanted_album_id IS NOT NULL
                           AND wanted_album_id = (SELECT wanted_album_id FROM wanted_songs
                                                  WHERE id = s.id)))) <= ?1
           AND coalesce(a.times, 0) < ?2 AND coalesce(a.last_at, 0) <= ?1
         ORDER BY s.added_at DESC, s.id DESC LIMIT ?3",
    )
    .bind(now - AFTER)
    .bind(TIMES)
    .bind((PER_RUN * 3) as i64)
    .fetch_all(pool)
    .await?;
    not_had(pool, candidates).await
}

/// Of the wanted songs `candidates`, those nothing is on its way for and that aren't in the
/// library by any means (a track of their recording, or with their title and artist, from
/// before they were wanted, say).
async fn not_had(pool: &SqlitePool, candidates: Vec<i64>) -> anyhow::Result<Vec<Again>> {
    let have = crate::aoide::pending::by_name::in_library(pool, &candidates).await?;
    let mut due = Vec::new();
    for id in candidates {
        if have.contains(&id) {
            continue;
        }
        let step = crate::progress::of(pool, "song", id).await?.map(|p| p.step);
        if matches!(step, Some(Step::Wanted | Step::Failed)) {
            due.push(Again::Song(id));
        }
    }
    Ok(due)
}

/// Wanted songs with no Soulseek search on record: one whose first search couldn't start
/// (slskd away when it was asked for, say), or whose searches were cleared after 30 days.
/// Nothing else looks for them, so they're searched for, the newest wanted first,
/// [`FIRST_PER_RUN`] a run at most: each wanted [`NEW_FOR`], nothing of it on its way or in the
/// library, and not searched again within [`AFTER`] nor more than [`TIMES`] times.
async fn never_searched(pool: &SqlitePool, now: i64) -> anyhow::Result<Vec<Again>> {
    let candidates: Vec<i64> = sqlx::query_scalar(
        "SELECT s.id FROM wanted_songs s
         LEFT JOIN soulseek_again a ON a.wanted_song_id = s.id
         WHERE NOT EXISTS (SELECT 1 FROM searches q WHERE q.wanted_song_id = s.id)
           AND s.added_at <= ?1
           AND NOT EXISTS (SELECT 1 FROM downloads d
                           WHERE d.wanted_song_id = s.id AND d.state <> 'failed')
           AND coalesce(a.times, 0) < ?2 AND coalesce(a.last_at, 0) <= ?3
         ORDER BY s.added_at DESC, s.id DESC LIMIT ?4",
    )
    .bind(now - NEW_FOR)
    .bind(TIMES)
    .bind(now - AFTER)
    .bind((FIRST_PER_RUN * 2) as i64)
    .fetch_all(pool)
    .await?;
    let mut due = not_had(pool, candidates).await?;
    due.truncate(FIRST_PER_RUN);
    Ok(due)
}

/// Monitored wanted albums due: a Soulseek download of them failed, none is on its way, nothing
/// for them for [`AFTER`], fewer than [`TIMES`] searches again, and not on disk; the newest
/// wanted first.
async fn due_albums(pool: &SqlitePool, now: i64) -> anyhow::Result<Vec<Again>> {
    let candidates: Vec<i64> = sqlx::query_scalar(
        "SELECT w.id FROM wanted_albums w
         JOIN wanted_artists ar ON ar.id = w.wanted_artist_id
         LEFT JOIN soulseek_again a ON a.wanted_album_id = w.id
         WHERE w.monitored = 1 AND ar.monitored = 1
           AND EXISTS (SELECT 1 FROM downloads d WHERE d.wanted_album_id = w.id
                       AND d.source = 'soulseek' AND d.state = 'failed')
           AND NOT EXISTS (SELECT 1 FROM downloads d
                           WHERE d.wanted_album_id = w.id AND d.state <> 'failed')
           AND (SELECT max(at) FROM (
                    SELECT searched_at AS at FROM searches WHERE wanted_album_id = w.id
                    UNION ALL
                    SELECT updated_at FROM downloads WHERE wanted_album_id = w.id)) <= ?1
           AND coalesce(a.times, 0) < ?2 AND coalesce(a.last_at, 0) <= ?1
         ORDER BY w.added_at DESC, w.id DESC LIMIT ?3",
    )
    .bind(now - AFTER)
    .bind(TIMES)
    .bind((PER_RUN * 3) as i64)
    .fetch_all(pool)
    .await?;
    let mut due = Vec::new();
    for id in candidates {
        if !crate::album_search::taken(pool, id).await? {
            due.push(Again::Album(id));
        }
    }
    Ok(due)
}

#[cfg(test)]
mod tests;
