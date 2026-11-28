//! Matched titles TMDB no longer has: the cached copy is kept and served, TMDB is asked again
//! once a day rather than at every run (and that it's gone is logged once a day with it), and
//! the title is listed to be matched again (`GET /api/v1/metadata/unmatched`, status `gone`)
//! until TMDB has it once more, when storing it clears the mark.

use sqlx::SqlitePool;

use super::DAY;

/// Which of TMDB's tables a title is cached in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cached {
    Movie,
    Series,
}

impl Cached {
    fn table(self) -> &'static str {
        match self {
            Cached::Movie => "tmdb_movies",
            Cached::Series => "tmdb_series",
        }
    }
}

/// `ids` without the titles TMDB said in the last day it no longer has, in their order.
pub(super) async fn not_gone_today(
    pool: &SqlitePool,
    cached: Cached,
    ids: Vec<i64>,
    now: i64,
) -> sqlx::Result<Vec<i64>> {
    let gone: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT tmdb_id FROM {} WHERE gone_at > ?",
        cached.table()
    )))
    .bind(now - DAY)
    .fetch_all(pool)
    .await?;
    Ok(ids.into_iter().filter(|id| !gone.contains(id)).collect())
}

/// Notes that TMDB no longer has the title `id`, and logs it: once a day, since
/// [`not_gone_today`] leaves it out until then.
pub(super) async fn gone(pool: &SqlitePool, cached: Cached, id: i64, now: i64) -> sqlx::Result<()> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {} SET gone_at = ? WHERE tmdb_id = ?",
        cached.table()
    )))
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    match cached {
        Cached::Movie => tracing::warn!(
            tmdb_id = id,
            "TMDB no longer has this movie; keeping the cached copy, asking again tomorrow, and listing it to be matched again"
        ),
        Cached::Series => tracing::warn!(
            tmdb_id = id,
            "TMDB no longer has this series; keeping the cached copy, asking again tomorrow, and listing it to be matched again"
        ),
    }
    Ok(())
}
