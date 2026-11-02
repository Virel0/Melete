//! TMDB's scores for titles in the library whose details were fetched before scores were kept:
//! asked for once, one request a title by the TMDB id already used, and only the score written,
//! so nothing else about the title is fetched again. Titles fetched since carry theirs.

use super::*;

/// Titles filled in one run, so a large library is done over a few runs rather than all at
/// once.
const BATCH: i64 = 200;

/// Fetches the score of present titles that have none yet. Stops when TMDB refuses the token or
/// asks for a long pause; other failures skip a title until the next run. Returns how many
/// titles got one.
pub async fn refresh_scores(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<usize> {
    let mut done = 0;
    let movies: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_movies t JOIN movies m ON m.match_tmdb_id = t.tmdb_id
         WHERE m.missing_since IS NULL AND t.score_votes IS NULL LIMIT ?",
    )
    .bind(BATCH)
    .fetch_all(pool)
    .await?;
    for id in movies {
        match tmdb.movie(id).await {
            Ok(found) => {
                let d = found.as_ref();
                keep(
                    pool,
                    "tmdb_movies",
                    id,
                    d.and_then(|d| d.vote_average),
                    d.and_then(|d| d.vote_count),
                )
                .await?;
                done += 1;
            }
            Err(err) => skip(
                err,
                "movie score",
                &id.to_string(),
                &mut RefreshReport::default(),
            )?,
        }
    }
    let series: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT t.tmdb_id FROM tmdb_series t JOIN series s ON s.match_tmdb_id = t.tmdb_id
         WHERE s.missing_since IS NULL AND t.score_votes IS NULL LIMIT ?",
    )
    .bind(BATCH)
    .fetch_all(pool)
    .await?;
    for id in series {
        match tmdb.series(id).await {
            Ok(found) => {
                let d = found.as_ref();
                keep(
                    pool,
                    "tmdb_series",
                    id,
                    d.and_then(|d| d.vote_average),
                    d.and_then(|d| d.vote_count),
                )
                .await?;
                done += 1;
            }
            Err(err) => skip(
                err,
                "series score",
                &id.to_string(),
                &mut RefreshReport::default(),
            )?,
        }
    }
    Ok(done)
}

/// Writes a title's score alone. A title TMDB no longer has, or without votes, keeps 0 votes,
/// so it isn't asked about again.
async fn keep(
    pool: &SqlitePool,
    table: &'static str,
    id: i64,
    average: Option<f64>,
    votes: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET score = ?, score_votes = ? WHERE tmdb_id = ?"
    )))
    .bind(store::score(average))
    .bind(votes.unwrap_or(0).max(0))
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}
