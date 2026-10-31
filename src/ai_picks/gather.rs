//! What picks are made from: what a viewer watched, and the candidates, each as the model is
//! told about it; and which kept picks are still there to serve.

use crate::ai::Told;

use super::*;

/// How many titles watched picks are made from.
const HISTORY: i64 = 15;
/// How many candidates the model chooses from.
const CANDIDATES: i64 = 60;

/// A title watched, as the model is told about it.
pub(super) struct Watched {
    pub title: Title,
    pub told: Told,
}

/// A candidate as the database gives it: series, id, title, year, genres, overview.
type CandidateRow = (bool, i64, String, Option<i64>, String, Option<String>);

/// A candidate, with the id it's told by.
pub(super) struct Candidate {
    pub title: Title,
    pub told: Told,
}

#[derive(sqlx::FromRow)]
struct Row {
    series: bool,
    id: i64,
    title: String,
    year: Option<i64>,
    genres: Option<String>,
}

fn genres(json: Option<&str>) -> Vec<String> {
    json.and_then(|g| serde_json::from_str(g).ok())
        .unwrap_or_default()
}

/// A title's name, year and genres, from TMDB's details only: a name read from a folder or a
/// file never leaves the server. `tm` and `ts` stand for the movie's and the series' details.
const DETAILS: &str = "coalesce(tm.title, ts.name) AS title, coalesce(tm.year, ts.year) AS year,
     coalesce(tm.genres, ts.genres) AS genres";

/// The titles matched on TMDB that a viewer finished or is watching and may see, most recent
/// first, an episode counting as its series. One not matched is left out: all there is to
/// tell of it is its folder's or file's name, which never leaves the server.
pub(super) async fn history(
    pool: &SqlitePool,
    user: i64,
    access: &Access,
) -> sqlx::Result<Vec<Watched>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.series_id IS NOT NULL AS series, coalesce(f.movie_id, f.series_id) AS id,
             {DETAILS}
         FROM playback_progress p
         JOIN media_files f ON f.id = p.file_id
         LEFT JOIN movies m ON m.id = f.movie_id
         LEFT JOIN series s ON s.id = f.series_id
         LEFT JOIN tmdb_movies tm ON tm.tmdb_id = m.match_tmdb_id
         LEFT JOIN tmdb_series ts ON ts.tmdb_id = s.match_tmdb_id
         WHERE p.user_id = ?1 AND (p.watched = 1 OR p.position_seconds > 0) AND {}
           AND (tm.tmdb_id IS NOT NULL OR ts.tmdb_id IS NOT NULL)
         GROUP BY f.movie_id, f.series_id
         ORDER BY max(p.updated_at) DESC LIMIT ?2",
        access.file_sql("f")
    )))
    .bind(user)
    .bind(HISTORY)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Watched {
            title: Title {
                series: r.series,
                id: r.id,
            },
            told: Told::new(None, &r.title, r.year, &genres(r.genres.as_deref()), None),
        })
        .collect())
}

/// The titles watched as one text, the same whatever their order: picks are made again when it
/// changes.
pub(super) fn history_key(watched: &[Watched]) -> String {
    let titles: Vec<Title> = watched.iter().map(|w| w.title).collect();
    let mut ids = title_ids(&titles);
    ids.sort();
    ids.join(",")
}

/// Present titles matched on TMDB that `access` sees, except `except`, and except those
/// `user` watched when there's one, most like `genres_liked` first (by how many they share),
/// then newest; each told by an id of its own.
async fn candidates(
    pool: &SqlitePool,
    user: Option<i64>,
    access: &Access,
    genres_liked: &[String],
    except: &[Title],
) -> anyhow::Result<Vec<Candidate>> {
    // Without a user (?1 is NULL), nobody's watching leaves anything out.
    let unwatched = |kind: &str| {
        format!(
            "NOT EXISTS (SELECT 1 FROM media_files w JOIN playback_progress p ON p.file_id = w.id
                 WHERE w.{kind}_id = x.id AND p.user_id = ?1 AND p.watched = 1)"
        )
    };
    let rows: Vec<CandidateRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT series, id, title, year, genres, overview FROM (
                 SELECT 0 AS series, x.id, t.title, t.year, t.genres, t.overview,
                     (SELECT max(coalesce(f.added_at, f.modified_at)) FROM media_files f
                      WHERE f.movie_id = x.id AND f.missing_since IS NULL) AS added
                 FROM movies x JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
                 WHERE x.missing_since IS NULL AND {movies} AND {movies_unwatched}
                 UNION ALL
                 SELECT 1, x.id, t.name, t.year, t.genres, t.overview,
                     (SELECT max(coalesce(f.added_at, f.modified_at)) FROM media_files f
                      WHERE f.series_id = x.id AND f.missing_since IS NULL)
                 FROM series x JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
                 WHERE x.missing_since IS NULL AND {series} AND {series_unwatched}
             ) WHERE added IS NOT NULL
               AND (CASE series WHEN 1 THEN 's' ELSE 'm' END || id)
                   NOT IN (SELECT value FROM json_each(?3))
             ORDER BY (SELECT count(*) FROM json_each(genres) g
                       WHERE g.value IN (SELECT value FROM json_each(?2))) DESC,
                      added DESC, series, id
             LIMIT ?4",
        movies = access.movie_sql("x"),
        series = access.series_sql("x"),
        movies_unwatched = unwatched("movie"),
        series_unwatched = unwatched("series"),
    )))
    .bind(user)
    .bind(serde_json::to_string(genres_liked)?)
    .bind(serde_json::to_string(&title_ids(except))?)
    .bind(CANDIDATES)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(
            |(n, (series, id, title, year, genres_json, overview))| Candidate {
                title: Title { series, id },
                told: Told::new(
                    Some(format!("c{}", n + 1)),
                    &title,
                    year,
                    &genres(Some(&genres_json)),
                    overview.as_deref(),
                ),
            },
        )
        .collect())
}

/// Titles as `m1`, `s4`.
fn title_ids(titles: &[Title]) -> Vec<String> {
    titles
        .iter()
        .map(|t| format!("{}{}", if t.series { 's' } else { 'm' }, t.id))
        .collect()
}

/// The candidates for a viewer's picks: titles they may see and haven't watched, most like what
/// they watched first.
pub(super) async fn candidates_for_viewer(
    pool: &SqlitePool,
    user: i64,
    access: &Access,
    watched: &[Watched],
) -> anyhow::Result<Vec<Candidate>> {
    let liked: Vec<String> = watched
        .iter()
        .flat_map(|w| w.told.genres.iter().cloned())
        .collect();
    let except: Vec<Title> = watched.iter().map(|w| w.title).collect();
    candidates(pool, Some(user), access, &liked, &except).await
}

/// The candidates like `title`: the titles `access` sees but it, most like its genres first.
pub(super) async fn candidates_like(
    pool: &SqlitePool,
    access: &Access,
    title: Title,
) -> anyhow::Result<Vec<Candidate>> {
    let liked = told_of(pool, title)
        .await?
        .map(|t| t.genres)
        .unwrap_or_default();
    candidates(pool, None, access, &liked, &[title]).await
}

/// A title as the model is told about it, with its premise; None for one not matched on TMDB.
pub(super) async fn told_of(pool: &SqlitePool, title: Title) -> sqlx::Result<Option<Told>> {
    let row: Option<(String, Option<i64>, String, Option<String>)> = if title.series {
        sqlx::query_as(
            "SELECT t.name, t.year, t.genres, t.overview FROM series x
             JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id WHERE x.id = ?",
        )
    } else {
        sqlx::query_as(
            "SELECT t.title, t.year, t.genres, t.overview FROM movies x
             JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id WHERE x.id = ?",
        )
    }
    .bind(title.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(name, year, genres_json, overview)| {
        Told::new(
            None,
            &name,
            year,
            &genres(Some(&genres_json)),
            overview.as_deref(),
        )
    }))
}

/// The kept picks still present that `access` sees, and that `user` hasn't watched since when
/// there's one, in order.
pub(super) async fn still_there(
    pool: &SqlitePool,
    user: Option<i64>,
    access: &Access,
    picks: &[Pick],
) -> sqlx::Result<Vec<Pick>> {
    let mut out = Vec::new();
    for pick in picks {
        let (table, kind, seen) = if pick.title.series {
            ("series", "series", access.series_sql("x"))
        } else {
            ("movies", "movie", access.movie_sql("x"))
        };
        let there: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS (SELECT 1 FROM {table} x WHERE x.id = ?1 AND x.missing_since IS NULL
                 AND {seen}
                 AND NOT EXISTS (SELECT 1 FROM media_files w
                     JOIN playback_progress p ON p.file_id = w.id
                     WHERE w.{kind}_id = x.id AND p.user_id = ?2 AND p.watched = 1))"
        )))
        .bind(pick.title.id)
        .bind(user)
        .fetch_one(pool)
        .await?;
        if there {
            out.push(pick.clone());
        }
    }
    Ok(out)
}
