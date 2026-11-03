//! The home's rows beyond the user's own and "Because you watched", each worked out from what
//! Melete stores, with nothing leaving the server: the next film in a collection the user has
//! started, shows whose next episode came in lately, highly rated titles
//! in the genres the user watches that they haven't started, and titles they watched long ago.

use super::*;

/// A title not played for this long counts as watched long ago.
const LONG_AGO: i64 = 180 * 86_400;
/// An episode that came in this recently is new.
const LATELY: i64 = 14 * 86_400;
/// A TMDB score this high, from at least this many votes, counts as highly rated.
const HIGH_SCORE: f64 = 7.5;
const ENOUGH_VOTES: i64 = 100;

fn titles(rows: Vec<(bool, i64)>) -> Vec<Entry> {
    rows.into_iter()
        .map(|(series, id)| Entry::plain(TitleRef { series, id }))
        .collect()
}

/// For each film collection the user has finished a film of, the earliest film of it in the
/// library they haven't watched; the collection last watched first.
pub(super) async fn next_in_collection(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
) -> Result<Vec<Entry>, ApiError> {
    let ids: Vec<Option<i64>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "WITH started AS (
             SELECT t.collection_id, max(p.updated_at) AS at FROM playback_progress p
             JOIN media_files f ON f.id = p.file_id
             JOIN movies m ON m.id = f.movie_id
             JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
             WHERE p.user_id = ?1 AND p.watched = 1 AND t.collection_id IS NOT NULL
             GROUP BY t.collection_id)
         SELECT (SELECT m.id FROM movies m JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
                 WHERE t.collection_id = started.collection_id AND m.missing_since IS NULL
                   AND {movies}
                   AND EXISTS (SELECT 1 FROM media_files f
                               WHERE f.movie_id = m.id AND f.missing_since IS NULL)
                   AND NOT EXISTS (SELECT 1 FROM media_files f
                       JOIN playback_progress p ON p.file_id = f.id
                       WHERE f.movie_id = m.id AND p.user_id = ?1 AND p.watched = 1)
                 ORDER BY coalesce(t.release_date, CAST(t.year AS TEXT)) IS NULL,
                     coalesce(t.release_date, CAST(t.year AS TEXT)), m.id LIMIT 1)
         FROM started ORDER BY started.at DESC LIMIT ?2",
        movies = access.movie_sql("m"),
    )))
    .bind(me)
    .bind(ROW)
    .fetch_all(pool)
    .await?;
    Ok(ids
        .into_iter()
        .flatten()
        .map(|id| Entry::plain(TitleRef::movie(id)))
        .collect())
}

/// Splits Next up: the shows whose next episode came in lately, after the user last watched
/// them, are its new episodes; the rest stay in Next up. Each keeps its episode, to play.
pub(super) async fn new_episodes(
    pool: &SqlitePool,
    me: i64,
    next_up: Vec<Entry>,
) -> Result<(Vec<Entry>, Vec<Entry>), ApiError> {
    let files: Vec<i64> = next_up
        .iter()
        .filter_map(|e| e.episode.as_ref()?["file_id"].as_i64())
        .collect();
    let fresh: HashSet<i64> = sqlx::query_scalar(
        "SELECT f.id FROM media_files f, json_each(?2) j
         WHERE f.id = j.value AND coalesce(f.added_at, f.modified_at) >= ?3
           AND coalesce(f.added_at, f.modified_at) > (
               SELECT max(p.updated_at) FROM playback_progress p
               JOIN media_files g ON g.id = p.file_id
               WHERE p.user_id = ?1 AND g.series_id = f.series_id)",
    )
    .bind(me)
    .bind(json!(files).to_string())
    .bind(crate::db::unix_now() - LATELY)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();
    Ok(next_up.into_iter().partition(|e| {
        e.episode
            .as_ref()
            .and_then(|ep| ep["file_id"].as_i64())
            .is_some_and(|id| fresh.contains(&id))
    }))
}

/// Titles the user hasn't started, sharing a genre with something they've watched, that TMDB
/// scores highly; the best scored first.
pub(super) async fn highly_rated(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
) -> Result<Vec<Entry>, ApiError> {
    let rows: Vec<(bool, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH mine AS (
             SELECT DISTINCT g.value FROM playback_progress p
             JOIN media_files f ON f.id = p.file_id
             LEFT JOIN movies m ON m.id = f.movie_id
             LEFT JOIN tmdb_movies tm ON tm.tmdb_id = m.match_tmdb_id
             LEFT JOIN series s ON s.id = f.series_id
             LEFT JOIN tmdb_series ts ON ts.tmdb_id = s.match_tmdb_id,
             json_each(coalesce(tm.genres, ts.genres, '[]')) g
             WHERE p.user_id = ?1),
         candidates AS (
             SELECT 0 AS series, m.id, t.score, t.score_votes, t.genres
             FROM movies m JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
             WHERE m.missing_since IS NULL AND {movies}
               AND EXISTS (SELECT 1 FROM media_files f
                           WHERE f.movie_id = m.id AND f.missing_since IS NULL)
               AND NOT EXISTS (SELECT 1 FROM media_files f
                   JOIN playback_progress p ON p.file_id = f.id
                   WHERE f.movie_id = m.id AND p.user_id = ?1)
             UNION ALL
             SELECT 1, s.id, t.score, t.score_votes, t.genres
             FROM series s JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
             WHERE s.missing_since IS NULL AND {series}
               AND EXISTS (SELECT 1 FROM media_files f
                           WHERE f.series_id = s.id AND f.missing_since IS NULL)
               AND NOT EXISTS (SELECT 1 FROM media_files f
                   JOIN playback_progress p ON p.file_id = f.id
                   WHERE f.series_id = s.id AND p.user_id = ?1))
         SELECT series, id FROM candidates
         WHERE score >= ?3 AND score_votes >= ?4
           AND EXISTS (SELECT 1 FROM json_each(genres) g
                       WHERE g.value IN (SELECT value FROM mine))
         ORDER BY score DESC, score_votes DESC, series, id LIMIT ?2",
        movies = access.movie_sql("m"),
        series = access.series_sql("s"),
    )))
    .bind(me)
    .bind(ROW)
    .bind(HIGH_SCORE)
    .bind(ENOUGH_VOTES)
    .fetch_all(pool)
    .await?;
    Ok(titles(rows))
}

/// Films the user watched, and shows they watched every episode of, that they haven't played
/// in a long while: their favourites first, then the most played.
pub(super) async fn watch_again(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
) -> Result<Vec<Entry>, ApiError> {
    let rows: Vec<(bool, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT series, id FROM (
             SELECT 0 AS series, m.id, max(p.updated_at) AS last, sum(p.play_count) AS plays,
                 EXISTS (SELECT 1 FROM favourites v WHERE v.user_id = ?1 AND v.movie_id = m.id)
                     AS favourite
             FROM movies m JOIN media_files f ON f.movie_id = m.id AND f.missing_since IS NULL
             JOIN playback_progress p ON p.file_id = f.id AND p.user_id = ?1
             WHERE m.missing_since IS NULL AND {movies}
             GROUP BY m.id HAVING max(p.watched) = 1 AND max(p.updated_at) < ?3
             UNION ALL
             SELECT 1, s.id, max(p.updated_at), sum(coalesce(p.play_count, 0)),
                 EXISTS (SELECT 1 FROM favourites v WHERE v.user_id = ?1 AND v.series_id = s.id)
             FROM series s JOIN media_files f ON f.series_id = s.id AND f.missing_since IS NULL
                 AND f.season > 0
             LEFT JOIN playback_progress p ON p.file_id = f.id AND p.user_id = ?1
             WHERE s.missing_since IS NULL AND {series}
             GROUP BY s.id
             HAVING count(*) = sum(coalesce(p.watched, 0)) AND max(p.updated_at) < ?3)
         ORDER BY favourite DESC, plays DESC, last, series, id LIMIT ?2",
        movies = access.movie_sql("m"),
        series = access.series_sql("s"),
    )))
    .bind(me)
    .bind(ROW)
    .bind(crate::db::unix_now() - LONG_AGO)
    .fetch_all(pool)
    .await?;
    Ok(titles(rows))
}
