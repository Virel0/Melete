//! A file that takes another's place: renamed, replaced by another tool, or by an upgrade.
//! Watch progress is kept per file, so without this a renamed episode would come back
//! unwatched. Only Melete's own rows change; nothing on disk is touched.
//!
//! A file succeeds another only within the same title row: the same movie, or the same series
//! and exactly the same episodes. Provider ids are never used, so titles from different
//! providers that share a number can't be mixed up.

use sqlx::SqliteConnection;

/// Gives `to` each user's watched state and play count from `from`, and their position when
/// `position` says the two play the same. A user with progress on `to` already keeps their
/// position there; it's watched if either was, and the plays of both add up.
pub async fn carry_progress(
    conn: &mut SqliteConnection,
    from: i64,
    to: i64,
    position: bool,
) -> sqlx::Result<u64> {
    // The WHERE before ON CONFLICT is needed: without one SQLite would read ON as a join's.
    Ok(sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
             watched, play_count, updated_at)
         SELECT user_id, ?2, CASE WHEN ?3 THEN position_seconds ELSE 0 END,
             CASE WHEN ?3 THEN duration_seconds END, watched, play_count, updated_at
         FROM playback_progress WHERE file_id = ?1
         ON CONFLICT (user_id, file_id) DO UPDATE SET
             watched = max(watched, excluded.watched),
             play_count = play_count + excluded.play_count",
    )
    .bind(from)
    .bind(to)
    .bind(position)
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// Each new file of a scan and a missing file of the same movie, or of exactly the same
/// episodes: (new, old, whether the two are the same size and time). Movies and episodes are
/// matched apart, so each side finds the missing files through its own index
/// (`media_files_movie`, `media_files_series`) instead of comparing every new file with every
/// file in the library.
const PAIRS: &str = "
    SELECT n.id, o.id, o.size = n.size AND o.modified_at = n.modified_at
    FROM media_files n JOIN media_files o ON o.library_id = n.library_id AND o.id <> n.id
    WHERE n.library_id = ?1 AND n.first_seen_generation = ?2 AND n.missing_since IS NULL
      AND o.missing_since IS NOT NULL
      AND (o.seen_generation = ?2 - 1 OR (?3 IS NOT NULL AND o.missing_since = ?4))
      AND n.movie_id IS NOT NULL AND o.movie_id = n.movie_id
    UNION ALL
    SELECT n.id, o.id, o.size = n.size AND o.modified_at = n.modified_at
    FROM media_files n JOIN media_files o ON o.library_id = n.library_id AND o.id <> n.id
    WHERE n.library_id = ?1 AND n.first_seen_generation = ?2 AND n.missing_since IS NULL
      AND o.missing_since IS NOT NULL
      AND (o.seen_generation = ?2 - 1 OR (?3 IS NOT NULL AND o.missing_since = ?4))
      AND n.season IS NOT NULL AND o.series_id = n.series_id AND o.season = n.season
      AND o.episode = n.episode
      AND coalesce(o.episode_end, o.episode) = coalesce(n.episode_end, n.episode)
    ORDER BY 1";

/// After a scan of `library_id` (generation `generation`, at `now`, of one title folder when
/// `scope` names it) has stored what it found and marked what it didn't: each file first seen in
/// it takes over the progress of the one file of the same movie, or of exactly the same
/// episodes, that went missing in it. A whole scan finds those as last seen by the scan before;
/// a folder's, which other folders' scans may have come between, as marked missing by it. Its position goes
/// with it only when the two are the same size and were last changed at the same time (a
/// rename); otherwise nothing yet says they play alike. Where two went missing, which one it
/// replaces is unclear, so nothing is carried. Answers how many files took over.
pub(super) async fn carry_renamed(
    tx: &mut SqliteConnection,
    library_id: i64,
    (generation, now, scope): (i64, i64, Option<&str>),
) -> sqlx::Result<usize> {
    let pairs: Vec<(i64, i64, bool)> = sqlx::query_as(PAIRS)
        .bind(library_id)
        .bind(generation)
        .bind(scope)
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;
    let mut carried = 0;
    for (i, &(new, old, same_file)) in pairs.iter().enumerate() {
        let alone = pairs
            .iter()
            .enumerate()
            .all(|(j, &(other, _, _))| j == i || other != new);
        if alone {
            carry_progress(tx, old, new, same_file).await?;
            crate::history::renamed(tx, old, new, now).await?;
            carried += 1;
        }
    }
    Ok(carried)
}

#[cfg(test)]
mod tests;
