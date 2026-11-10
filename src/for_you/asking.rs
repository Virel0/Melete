//! Asking ListenBrainz, with its answers kept a week by the id asked about, so a list made again
//! (or another listener's with the same seed) doesn't ask again.

use std::collections::HashMap;

use serde::de::DeserializeOwned;

use super::*;
use crate::listenbrainz::{Recording, SimilarArtist, SimilarRecording};

/// How long an answer is kept, and used instead of asking again.
pub(super) const KEPT_FOR: i64 = 7 * 24 * 3600;

/// The answer kept about `mbid` of `kind`, while it's fresh.
async fn kept<T: DeserializeOwned>(
    pool: &SqlitePool,
    kind: &str,
    mbid: &str,
    now: i64,
) -> sqlx::Result<Option<T>> {
    let answer: Option<String> = sqlx::query_scalar(
        "SELECT answer FROM listenbrainz_answers WHERE kind = ? AND mbid = ? AND fetched_at > ?",
    )
    .bind(kind)
    .bind(mbid)
    .bind(now - KEPT_FOR)
    .fetch_optional(pool)
    .await?;
    // One kept in a shape this Melete can't read is asked for again.
    Ok(answer.and_then(|a| serde_json::from_str(&a).ok()))
}

/// Keeps `answer` about `mbid` of `kind`.
async fn keep<T: Serialize>(
    pool: &SqlitePool,
    kind: &str,
    mbid: &str,
    answer: &T,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO listenbrainz_answers (kind, mbid, answer, fetched_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (kind, mbid) DO UPDATE SET answer = excluded.answer,
             fetched_at = excluded.fetched_at",
    )
    .bind(kind)
    .bind(mbid)
    .bind(serde_json::to_string(answer)?)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// The recordings listened to alongside `recording`, closest first: as kept, else asked.
pub(super) async fn similar_recordings(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    recording: &str,
    now: i64,
) -> anyhow::Result<Vec<SimilarRecording>> {
    if let Some(found) = kept(pool, "similar_recordings", recording, now).await? {
        return Ok(found);
    }
    let found = client.similar_recordings(recording).await?;
    keep(pool, "similar_recordings", recording, &found, now).await?;
    Ok(found)
}

/// The artists listened to alongside `artist`, closest first: as kept, else asked.
pub(super) async fn similar_artists(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    artist: &str,
    now: i64,
) -> anyhow::Result<Vec<SimilarArtist>> {
    if let Some(found) = kept(pool, "similar_artists", artist, now).await? {
        return Ok(found);
    }
    let found = client.similar_artists(artist).await?;
    keep(pool, "similar_artists", artist, &found, now).await?;
    Ok(found)
}

/// The artists and album of each of `recordings` ListenBrainz knows: as kept, else asked in
/// batches. One it doesn't know is kept as unknown, so it isn't asked about again this week.
pub(super) async fn recordings(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    recordings: &[String],
    now: i64,
) -> anyhow::Result<HashMap<String, Recording>> {
    let mut known = HashMap::new();
    let mut ask = Vec::new();
    for id in recordings {
        match kept::<Option<Recording>>(pool, "recording", id, now).await? {
            Some(Some(found)) => {
                known.insert(id.clone(), found);
            }
            Some(None) => {}
            None => ask.push(id.clone()),
        }
    }
    for batch in ask.chunks(listenbrainz::METADATA_BATCH) {
        let mut found = client.recordings(batch).await?;
        for id in batch {
            let one = found.remove(id);
            keep(pool, "recording", id, &one, now).await?;
            if let Some(one) = one {
                known.insert(id.clone(), one);
            }
        }
    }
    Ok(known)
}

/// Forgets answers past [`KEPT_FOR`].
pub(super) async fn forget_old(pool: &SqlitePool, now: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM listenbrainz_answers WHERE fetched_at <= ?")
        .bind(now - KEPT_FOR)
        .execute(pool)
        .await?;
    Ok(())
}
