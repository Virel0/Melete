//! Matches an admin makes or rules out by hand.

use super::*;

/// Matches a movie to the TMDB entry an admin chose. `false` if TMDB has no such movie.
pub async fn match_movie_manually(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    movie_id: i64,
    tmdb_id: i64,
) -> anyhow::Result<bool> {
    let Some(details) = tmdb.movie(tmdb_id).await? else {
        return Ok(false);
    };
    let now = unix_now();
    store_movie(pool, &details, now).await?;
    set_match(
        pool,
        Kind::Movie,
        movie_id,
        Some((details.id, "manual")),
        None,
        now,
    )
    .await?;
    Ok(true)
}

/// Matches a series to the TMDB entry an admin chose. `false` if TMDB has no such series.
pub async fn match_series_manually(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    series_id: i64,
    tmdb_id: i64,
) -> anyhow::Result<bool> {
    let Some((details, seasons)) = fetch_series(tmdb, tmdb_id).await? else {
        return Ok(false);
    };
    let now = unix_now();
    store_series(pool, &details, &seasons, now).await?;
    set_match(
        pool,
        Kind::Series,
        series_id,
        Some((details.id, "manual")),
        None,
        now,
    )
    .await?;
    Ok(true)
}

/// Records an admin's decision that a title has no TMDB entry. Automatic matching leaves
/// manual decisions alone, so it is never retried.
pub async fn unmatch_manually(pool: &SqlitePool, kind: Kind, id: i64) -> anyhow::Result<()> {
    let query = match kind {
        Kind::Movie => {
            "UPDATE movies SET match_status = 'unmatched', match_tmdb_id = NULL,
                 match_method = 'manual', match_candidates = NULL, match_attempted_at = ?
             WHERE id = ?"
        }
        Kind::Series => {
            "UPDATE series SET match_status = 'unmatched', match_tmdb_id = NULL,
                 match_method = 'manual', match_candidates = NULL, match_attempted_at = ?
             WHERE id = ?"
        }
    };
    sqlx::query(query)
        .bind(unix_now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
