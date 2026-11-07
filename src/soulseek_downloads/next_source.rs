//! The next source, when a Soulseek download of a wanted album or song fails once slskd has it:
//! a file the peer refused, timed out, errored or was cancelled, or a batch slskd lost. The
//! next best accepted Soulseek result of the same search, as [`artist_search::best_first`]
//! ranks them for grabbing, is grabbed from a user none of the album's or song's downloads has
//! failed with, until the search has had [`TRIES`] downloads, the first one included. Nothing
//! is grabbed when the album or song is on disk or on its way meanwhile, or isn't wanted or
//! monitored any more. The failed download says what became of the next source, unless its
//! result was the search's only accepted Soulseek one, and the next source's download says why
//! it's from there while it downloads.

use std::collections::HashSet;

use super::*;
use crate::{album_search, artist_search, song_search};

/// Soulseek downloads a search gets at most for its album or song, the first one included.
const TRIES: i64 = 3;

/// What a failed transfer's state says happened, as the next source's note puts it.
pub(super) fn cause(state: &str) -> &'static str {
    if state.contains("Rejected") {
        "a Soulseek user refused the file"
    } else if state.contains("TimedOut") {
        "a Soulseek user's transfer timed out"
    } else if state.contains("Cancelled") {
        "a Soulseek user's transfer was cancelled"
    } else {
        "a Soulseek user's transfer failed"
    }
}

/// What happened when slskd no longer knows a download's batch.
pub(super) const LOST: &str = "slskd lost track of the last one";

#[derive(sqlx::FromRow)]
struct Failed {
    search_id: Option<i64>,
    wanted_album_id: Option<i64>,
    wanted_song_id: Option<i64>,
}

/// What's been used up: the search's Soulseek downloads, and the users the album or song has
/// failed with.
struct Used {
    tries: i64,
    users: HashSet<String>,
}

/// Grabs the next source after download `id` failed for `cause`, and adds what became of it to
/// the failed download's message. Trouble is said there and logged, not passed up, so the other
/// downloads are still followed.
pub(super) async fn after_failure(pool: &SqlitePool, slskd: &slskd::Client, id: i64, cause: &str) {
    let said = match next(pool, slskd, id, cause).await {
        Ok(None) => return,
        Ok(Some(said)) => said,
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't grab the next Soulseek source"
            );
            format!("couldn't try the next source: {err:#}")
        }
    };
    let added = sqlx::query(
        "UPDATE downloads SET message = coalesce(message || '; ', '') || ? WHERE id = ?",
    )
    .bind(said)
    .bind(id)
    .execute(pool)
    .await;
    if let Err(err) = added {
        tracing::warn!(
            error = format!("{err:#}"),
            "couldn't say what became of the next Soulseek source"
        );
    }
}

/// Grabs the next source for failed download `id`; what to add to its message, or None when
/// nothing was looked for.
async fn next(
    pool: &SqlitePool,
    slskd: &slskd::Client,
    id: i64,
    cause: &str,
) -> anyhow::Result<Option<String>> {
    let failed: Failed = sqlx::query_as(
        "SELECT search_id, wanted_album_id, wanted_song_id FROM downloads WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    let Some(search_id) = failed.search_id else {
        return Ok(None);
    };
    if !needed(pool, &failed).await? {
        return Ok(None);
    }
    let soulseek: Vec<(i64, String)> = artist_search::best_first(pool, search_id)
        .await?
        .into_iter()
        .filter_map(|(release_id, _, user)| Some((release_id, user?)))
        .collect();
    let mut used = used_up(pool, &failed, search_id).await?;
    // The search's one Soulseek result failed: there was never another to try.
    if used.tries <= 1 && soulseek.len() <= 1 {
        return Ok(None);
    }
    for (release_id, user) in soulseek {
        if used.tries >= TRIES {
            break;
        }
        if used.users.contains(&user) {
            continue;
        }
        match grab(pool, slskd, release_id).await? {
            Ok(download) => {
                let n = used.tries + 1;
                let note = format!("trying the next source ({n} of {TRIES}) after {cause}");
                sqlx::query(
                    "UPDATE downloads SET next_source_note = ?1,
                         message = ?1 || ': ' || coalesce(message, '')
                     WHERE id = ?2",
                )
                .bind(note)
                .bind(download.id)
                .execute(pool)
                .await?;
                return Ok(Some(format!(
                    "the next source is being tried ({n} of {TRIES})"
                )));
            }
            // A queue slskd refused is a failed download of its own, and counts as a try.
            Err(refused) => {
                tracing::info!(
                    release_id,
                    why = refused.to_string(),
                    "couldn't grab a next Soulseek source"
                );
                used = used_up(pool, &failed, search_id).await?;
            }
        }
    }
    Ok(Some(if used.tries >= TRIES {
        format!("all {TRIES} tries are used, so it stays failed")
    } else {
        "no other accepted result from another Soulseek user is left, so it stays failed".into()
    }))
}

/// Whether the album or song still wants a download: wanted, monitored (an album and its
/// artist), and neither on disk nor on its way.
async fn needed(pool: &SqlitePool, failed: &Failed) -> anyhow::Result<bool> {
    match (failed.wanted_album_id, failed.wanted_song_id) {
        (Some(album), _) => {
            let monitored: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM wanted_albums w
                                JOIN wanted_artists a ON a.id = w.wanted_artist_id
                                WHERE w.id = ? AND w.monitored = 1 AND a.monitored = 1)",
            )
            .bind(album)
            .fetch_one(pool)
            .await?;
            Ok(monitored && !album_search::taken(pool, album).await?)
        }
        (None, Some(song)) => Ok(!song_search::taken(pool, song).await?),
        (None, None) => Ok(false),
    }
}

/// What the search and its album or song have used up so far.
async fn used_up(pool: &SqlitePool, failed: &Failed, search_id: i64) -> sqlx::Result<Used> {
    let tries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM downloads WHERE source = 'soulseek' AND search_id = ?",
    )
    .bind(search_id)
    .fetch_one(pool)
    .await?;
    let users: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT json_extract(soulseek, '$.username') FROM downloads
         WHERE source = 'soulseek' AND state = 'failed'
           AND (wanted_album_id = ? OR wanted_song_id = ?)",
    )
    .bind(failed.wanted_album_id)
    .bind(failed.wanted_song_id)
    .fetch_all(pool)
    .await?;
    Ok(Used {
        tries,
        users: users.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests;
