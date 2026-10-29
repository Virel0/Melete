//! Matching by hand: searching for releases, matching an album to the one chosen, or recording that
//! it has none.

use super::*;

/// MusicBrainz's releases for a search by hand, one per release group, best first.
pub async fn search_by_hand(
    mb: &musicbrainz::Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<serde_json::Value>, musicbrainz::Error> {
    let found = mb.search(title, artist).await?;
    let mut seen: Vec<&str> = Vec::new();
    Ok(found
        .iter()
        .filter(|f| {
            let new = !seen.contains(&f.release_group.id.as_str());
            if new {
                seen.push(&f.release_group.id);
            }
            new
        })
        .map(candidate)
        .collect())
}

/// How matching an album by hand went.
#[derive(Debug, PartialEq)]
pub enum ByHand {
    Matched { release_group_id: String },
    NoSuchAlbum,
    NoSuchRelease,
}

/// Matches an album to a release chosen by hand. Decisions by hand are final: matching never
/// revisits them.
pub async fn match_by_hand(
    pool: &SqlitePool,
    mb: &musicbrainz::Client,
    album_id: i64,
    release_id: &str,
) -> anyhow::Result<ByHand> {
    let album: Option<(Option<i64>, String)> = sqlx::query_as(
        "SELECT artist_id, artist_name FROM music_albums WHERE id = ? AND missing_since IS NULL",
    )
    .bind(album_id)
    .fetch_optional(pool)
    .await?;
    let Some((artist_id, artist_name)) = album else {
        return Ok(ByHand::NoSuchAlbum);
    };
    let release = match mb.release(release_id).await {
        Ok(release) => release,
        Err(musicbrainz::Error::NotFound) => return Ok(ByHand::NoSuchRelease),
        Err(err) => anyhow::bail!(err.to_string()),
    };
    let now = unix_now();
    let mut tx = pool.begin().await?;
    store_release(&mut tx, &release, now).await?;
    sqlx::query(
        "UPDATE music_albums SET match_status = 'matched', match_method = 'manual',
             match_release_id = ?, match_release_group_id = ?, match_candidates = NULL,
             matched_at = ?
         WHERE id = ?",
    )
    .bind(&release.id)
    .bind(&release.release_group.id)
    .bind(now)
    .bind(album_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE music_tracks SET match_recording_id = NULL, match_track_id = NULL WHERE album_id = ?")
        .bind(album_id)
        .execute(&mut *tx)
        .await?;
    link_tracks(&mut tx, album_id, &release.id).await?;
    if let (Some(artist_id), [credit]) = (artist_id, &release.artist_credit[..])
        && fold(&credit.artist.name) == fold(&artist_name)
    {
        sqlx::query(
            "UPDATE music_artists SET mb_artist_id = coalesce(mb_artist_id, ?) WHERE id = ?",
        )
        .bind(&credit.artist.id)
        .bind(artist_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    crate::jobs::WORKS.notify_one();
    Ok(ByHand::Matched {
        release_group_id: release.release_group.id,
    })
}

/// Records that an album has no MusicBrainz release; matching never tries it again. Returns
/// false when there's no such album.
pub async fn no_match(pool: &SqlitePool, album_id: i64) -> anyhow::Result<bool> {
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE music_albums SET match_status = 'unmatched', match_method = 'manual',
             match_release_id = NULL, match_release_group_id = NULL, match_candidates = NULL,
             matched_at = ?
         WHERE id = ? AND missing_since IS NULL",
    )
    .bind(unix_now())
    .bind(album_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("UPDATE music_tracks SET match_recording_id = NULL, match_track_id = NULL WHERE album_id = ?")
        .bind(album_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::jobs::WORKS.notify_one();
    Ok(changed > 0)
}
