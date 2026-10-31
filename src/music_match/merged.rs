//! Albums matched to a release MusicBrainz has since merged into another: moved to the one it
//! joined, as a fresh match would have them, once a later fetch of the old release answers with
//! the survivor.

use super::*;

/// Moves every album matched to `merged` to `survivor` (just fetched, as MusicBrainz now has
/// it): caches the survivor, points each album at it and its release group, links each album's
/// tracks to its tracks, and drops the merged release once nothing points at it. Logs each
/// album moved, once; returns how many.
pub(super) async fn move_to_survivor(
    pool: &SqlitePool,
    merged: &str,
    survivor: &Release,
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await?;
    store_release(&mut tx, survivor, unix_now()).await?;
    let albums: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, title FROM music_albums WHERE match_release_id = ? ORDER BY id")
            .bind(merged)
            .fetch_all(&mut *tx)
            .await?;
    for (album, _) in &albums {
        sqlx::query(
            "UPDATE music_albums SET match_release_id = ?, match_release_group_id = ? WHERE id = ?",
        )
        .bind(&survivor.id)
        .bind(&survivor.release_group.id)
        .bind(album)
        .execute(&mut *tx)
        .await?;
        link_tracks(&mut tx, *album, &survivor.id).await?;
    }
    sqlx::query(
        "DELETE FROM mb_releases WHERE id = ?1
           AND NOT EXISTS (SELECT 1 FROM music_albums WHERE match_release_id = ?1)",
    )
    .bind(merged)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    for (album, title) in &albums {
        tracing::info!(
            album,
            title,
            from = merged,
            to = survivor.id,
            "moved an album from a merged MusicBrainz release to the one it joined"
        );
    }
    Ok(albums.len())
}
