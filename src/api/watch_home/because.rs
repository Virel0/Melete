//! "Because you watched …": for each of the viewer's most recently finished titles (an episode
//! counts as its series), newest first, titles in the library like it that they may see and
//! haven't watched, most alike first.
//!
//! Likeness comes from what Melete stores of each title, with nothing leaving the server: a
//! film's collection, shared genres, shared cast and crew, the decade it came out in, and its
//! own language. A title has to share a collection, a genre or a person to count; the decade
//! and the language only order. With
//! trending and similar titles on, TMDB's recommendations for the title are blended in, kept to
//! what's in the library: only its TMDB id leaves, and the answer is kept a day.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::*;
use crate::tmdb;

/// The most rows, one per title finished.
const SEEDS: i64 = 3;
/// The most titles in a row.
const PER_ROW: i64 = 12;
/// How long TMDB's recommendations for a title are kept.
const KEEP_RECOMMENDED: Duration = Duration::from_secs(24 * 3600);
/// How long the home waits for TMDB's recommendations before going without.
const WAIT_RECOMMENDED: Duration = Duration::from_secs(5);

/// TMDB's recommendations by title (whether it's a series, its TMDB id): the TMDB ids it
/// recommends, and when they were fetched.
pub(super) type Recommended = Arc<Mutex<HashMap<(bool, i64), (Instant, Arc<Vec<i64>>)>>>;

/// What the rows need beyond the database: TMDB, when it's set up, and what it recommended;
/// and OpenRouter, for AI picks.
#[derive(Clone, Default)]
pub(super) struct Outside {
    pub tmdb: Option<Arc<tmdb::Client>>,
    pub recommended: Recommended,
    pub ai: Option<Arc<crate::ai::Client>>,
}

/// A title finished, and what likeness is measured by.
#[derive(Debug, Default)]
struct Seed {
    tmdb_id: Option<i64>,
    /// A JSON array of genre names.
    genres: String,
    year: Option<i64>,
    collection: Option<i64>,
    /// A JSON array of TMDB person ids.
    people: String,
    /// Its own language as TMDB gives it (`ja`), when known.
    language: Option<String>,
}

/// The rows: each title the user finished most recently, with the titles like it.
pub(super) async fn rows(
    pool: &SqlitePool,
    outside: &Outside,
    me: i64,
    access: &Access,
) -> Result<Vec<(TitleRef, Vec<Entry>)>, ApiError> {
    let finished = finished(pool, me, access).await?;
    let discovery = outside.tmdb.is_some() && crate::settings::discovery(pool).await?;
    let alike = join_all(
        finished
            .iter()
            .map(|title| like(pool, outside, discovery, me, access, *title)),
    )
    .await;
    let mut rows = Vec::new();
    for (title, alike) in finished.into_iter().zip(alike) {
        let entries = alike?.into_iter().map(|(t, _)| Entry::plain(t)).collect();
        rows.push((title, entries));
    }
    Ok(rows)
}

/// The present titles the user may see and hasn't watched that are like `title`, most alike
/// first, each with the AI's reason when it chose them: with AI picks on, the AI's once they're
/// made; otherwise by Melete's own likeness, TMDB's recommendations blended in when
/// `discovery` is on.
pub(super) async fn like(
    pool: &SqlitePool,
    outside: &Outside,
    discovery: bool,
    me: i64,
    access: &Access,
    title: TitleRef,
) -> Result<Vec<(TitleRef, Option<String>)>, ApiError> {
    if let (_, Some(picks)) = super::picks::like(pool, outside, me, access, title).await? {
        return Ok(picks.into_iter().map(|(t, r)| (t, Some(r))).collect());
    }
    own_like(pool, outside, discovery, me, access, title).await
}

/// Like [`like`], by Melete's own likeness alone (and TMDB's with `discovery`).
pub(super) async fn own_like(
    pool: &SqlitePool,
    outside: &Outside,
    discovery: bool,
    me: i64,
    access: &Access,
    title: TitleRef,
) -> Result<Vec<(TitleRef, Option<String>)>, ApiError> {
    let seed = seed(pool, title).await?;
    let recommended = recommendations(outside, discovery, title.series, seed.tmdb_id).await;
    let alike = alike(pool, me, access, title, &seed, &recommended).await?;
    Ok(alike.into_iter().map(|t| (t, None)).collect())
}

/// The titles the user finished most recently, newest first, each once.
async fn finished(pool: &SqlitePool, me: i64, access: &Access) -> Result<Vec<TitleRef>, ApiError> {
    let rows: Vec<(Option<i64>, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.movie_id, f.series_id FROM playback_progress p
         JOIN media_files f ON f.id = p.file_id
         WHERE p.user_id = ?1 AND p.watched = 1 AND {}
           AND (f.movie_id IS NOT NULL OR f.series_id IS NOT NULL)
         GROUP BY f.movie_id, f.series_id
         ORDER BY max(p.updated_at) DESC LIMIT ?2",
        access.file_sql("f")
    )))
    .bind(me)
    .bind(SEEDS)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| match row {
            (Some(id), _) => Some(TitleRef::movie(id)),
            (None, Some(id)) => Some(TitleRef::series(id)),
            (None, None) => None,
        })
        .collect())
}

/// What a title's likeness is measured by; nothing for one not matched on TMDB.
async fn seed(pool: &SqlitePool, title: TitleRef) -> Result<Seed, ApiError> {
    type Found = (i64, String, Option<i64>, Option<i64>, Option<String>);
    let found: Option<Found> = if title.series {
        sqlx::query_as(
            "SELECT t.tmdb_id, t.genres, t.year, NULL, t.original_language FROM series s
             JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id WHERE s.id = ?",
        )
    } else {
        sqlx::query_as(
            "SELECT t.tmdb_id, t.genres, t.year, t.collection_id, t.original_language FROM movies m
             JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id WHERE m.id = ?",
        )
    }
    .bind(title.id)
    .fetch_optional(pool)
    .await?;
    let Some((tmdb_id, genres, year, collection, language)) = found else {
        return Ok(Seed::default());
    };
    let people: Vec<i64> = sqlx::query_scalar(if title.series {
        "SELECT DISTINCT person_id FROM tmdb_credits WHERE series_tmdb_id = ?"
    } else {
        "SELECT DISTINCT person_id FROM tmdb_credits WHERE movie_tmdb_id = ?"
    })
    .bind(tmdb_id)
    .fetch_all(pool)
    .await?;
    Ok(Seed {
        tmdb_id: Some(tmdb_id),
        genres,
        year,
        collection,
        people: serde_json::to_string(&people)?,
        // TMDB's "xx" is no language: two silent films aren't alike for it.
        language: language.filter(|l| l != "xx"),
    })
}

/// The TMDB ids TMDB recommends with a title, kept a day; none while trending and similar
/// titles are off, for a title not matched on TMDB, or when TMDB doesn't answer in time.
async fn recommendations(
    outside: &Outside,
    discovery: bool,
    series: bool,
    tmdb_id: Option<i64>,
) -> Arc<Vec<i64>> {
    let (Some(tmdb), true, Some(tmdb_id)) = (&outside.tmdb, discovery, tmdb_id) else {
        return Arc::default();
    };
    let key = (series, tmdb_id);
    {
        let kept = outside.recommended.lock().expect("never poisoned");
        if let Some((at, ids)) = kept.get(&key)
            && at.elapsed() < KEEP_RECOMMENDED
        {
            return ids.clone();
        }
    }
    let asked = tokio::time::timeout(WAIT_RECOMMENDED, tmdb.recommendations(series, tmdb_id));
    let ids: Arc<Vec<i64>> = match asked.await {
        Ok(Ok(listed)) => Arc::new(listed.unwrap_or_default().iter().map(|l| l.id).collect()),
        Ok(Err(err)) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "TMDB's recommendations for a title couldn't be read"
            );
            return Arc::default();
        }
        Err(_) => return Arc::default(),
    };
    let mut kept = outside.recommended.lock().expect("never poisoned");
    kept.retain(|_, (at, _)| at.elapsed() < KEEP_RECOMMENDED);
    kept.insert(key, (Instant::now(), ids.clone()));
    ids
}

/// The present titles the user may see and hasn't watched that are like `title`, most alike
/// first: a shared collection counts 6, each shared genre 2 (up to three), each shared person 2
/// (up to four), TMDB recommending it 6, the same decade 1 and the same language 1. One sharing
/// no collection, genre or person, and not recommended, isn't like it.
async fn alike(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
    title: TitleRef,
    seed: &Seed,
    recommended: &[i64],
) -> Result<Vec<TitleRef>, ApiError> {
    if seed.tmdb_id.is_none() {
        return Ok(Vec::new());
    }
    let rows: Vec<(bool, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH candidates AS (
             SELECT 0 AS series, m.id, t.tmdb_id, t.genres, t.year, t.collection_id,
                 t.original_language,
                 (SELECT count(DISTINCT c.person_id) FROM tmdb_credits c
                  WHERE c.movie_tmdb_id = t.tmdb_id
                    AND c.person_id IN (SELECT value FROM json_each(?2))) AS people
             FROM movies m JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
             WHERE m.missing_since IS NULL AND {movies}
               AND EXISTS (SELECT 1 FROM media_files f
                           WHERE f.movie_id = m.id AND f.missing_since IS NULL)
               AND NOT EXISTS (SELECT 1 FROM media_files f
                   JOIN playback_progress p ON p.file_id = f.id
                   WHERE f.movie_id = m.id AND p.user_id = ?1 AND p.watched = 1)
             UNION ALL
             SELECT 1, s.id, t.tmdb_id, t.genres, t.year, NULL, t.original_language,
                 (SELECT count(DISTINCT c.person_id) FROM tmdb_credits c
                  WHERE c.series_tmdb_id = t.tmdb_id
                    AND c.person_id IN (SELECT value FROM json_each(?2)))
             FROM series s JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
             WHERE s.missing_since IS NULL AND {series}
               AND EXISTS (SELECT 1 FROM media_files f
                           WHERE f.series_id = s.id AND f.missing_since IS NULL)
               AND NOT EXISTS (SELECT 1 FROM media_files f
                   JOIN playback_progress p ON p.file_id = f.id
                   WHERE f.series_id = s.id AND p.user_id = ?1 AND p.watched = 1)
         ),
         measured AS (
             SELECT series, id, year,
                 coalesce(collection_id = ?4, 0) AS collection,
                 (SELECT count(*) FROM json_each(genres) g
                  WHERE g.value IN (SELECT value FROM json_each(?3))) AS genres,
                 people,
                 series = ?6 AND tmdb_id IN (SELECT value FROM json_each(?7)) AS recommended,
                 coalesce(year / 10 = ?5 / 10, 0) AS decade,
                 coalesce(original_language = ?10, 0) AS language
             FROM candidates WHERE NOT (series = ?6 AND id = ?8)
         )
         SELECT series, id FROM measured
         WHERE collection OR genres > 0 OR people > 0 OR recommended
         ORDER BY 6 * collection + 2 * min(genres, 3) + 2 * min(people, 4) + 6 * recommended
                  + decade + language DESC, year DESC, series, id
         LIMIT ?9",
        movies = access.movie_sql("m"),
        series = access.series_sql("s"),
    )))
    .bind(me)
    .bind(&seed.people)
    .bind(&seed.genres)
    .bind(seed.collection)
    .bind(seed.year)
    .bind(title.series)
    .bind(serde_json::to_string(recommended)?)
    .bind(title.id)
    .bind(PER_ROW)
    .bind(&seed.language)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(series, id)| TitleRef { series, id })
        .collect())
}
