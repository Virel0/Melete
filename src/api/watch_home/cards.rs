//! What the home shows of each title: its art, facts and overview, when it came in, and the
//! user's progress on it, looked up for every title on the page at once.

use super::*;
use crate::api::media::{BACKDROP_SIZES, LOGO_SIZES, artwork_url, unwatched_episodes};

/// A movie or a series, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct TitleRef {
    /// Whether it's a series; else a movie.
    pub series: bool,
    /// The movie's or series' id.
    pub id: i64,
}

impl TitleRef {
    pub(super) fn movie(id: i64) -> Self {
        Self { series: false, id }
    }

    pub(super) fn series(id: i64) -> Self {
        Self { series: true, id }
    }
}

#[derive(sqlx::FromRow)]
struct CardRow {
    id: i64,
    title: String,
    year: Option<i64>,
    overview: Option<String>,
    runtime_minutes: Option<i64>,
    genres: Option<String>,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    textless_backdrop_path: Option<String>,
    logo_path: Option<String>,
    backdrop_x: Option<f64>,
    backdrop_y: Option<f64>,
    textless_x: Option<f64>,
    textless_y: Option<f64>,
    added_at: Option<i64>,
}

fn focus(x: Option<f64>, y: Option<f64>) -> Value {
    match (x, y) {
        (Some(x), Some(y)) => json!({ "x": x, "y": y }),
        _ => Value::Null,
    }
}

/// The cards of the titles on the page, and the user's progress on its movies.
pub(super) struct Cards {
    cards: ByTitle,
    /// The movie's file the user last had progress on, by movie.
    movie_progress: HashMap<i64, Value>,
}

impl Cards {
    /// Looks up every title in `titles` that's still present and `access` sees, with `me`'s
    /// progress.
    pub(super) async fn load(
        pool: &SqlitePool,
        me: i64,
        access: &Access,
        titles: &[TitleRef],
    ) -> Result<Self, ApiError> {
        let mut cards = ByTitle::new();
        for series in [false, true] {
            let ids = ids_of(titles, series);
            if ids.is_empty() {
                continue;
            }
            let unwatched = if series {
                unwatched_episodes(pool, me, &ids).await?
            } else {
                HashMap::new()
            };
            for row in card_rows(pool, access, series, &ids).await? {
                let mut card = card_json(series, &row)?;
                card["unwatched_episodes"] = if series {
                    json!(unwatched.get(&row.id).copied().unwrap_or(0))
                } else {
                    Value::Null
                };
                let title = TitleRef { series, id: row.id };
                cards.insert(title, card);
            }
        }
        let movies = ids_of(titles, false);
        Ok(Self {
            cards,
            movie_progress: movie_progress(pool, me, &movies).await?,
        })
    }

    /// Whether `title` is on the page: present, and the user may see it.
    pub(super) fn has(&self, title: TitleRef) -> bool {
        self.cards.contains_key(&title)
    }

    /// Replaces a card's backdrops, focal points and logo with `found`, as `/artwork` gives them.
    pub(super) fn overlay_artwork(&mut self, title: TitleRef, found: &Value) {
        if let Some(card) = self.cards.get_mut(&title) {
            for field in [
                "backdrop",
                "backdrop_focus",
                "textless_backdrop",
                "textless_backdrop_focus",
                "logo",
            ] {
                card[field] = found[field].clone();
            }
        }
    }

    /// An entry as the home lists it; none when its title has gone.
    pub(super) fn item(&self, entry: &Entry) -> Option<Value> {
        let mut item = self.cards.get(&entry.title)?.clone();
        item["episode"] = entry.episode.clone().unwrap_or(Value::Null);
        item["progress"] = entry
            .progress
            .clone()
            .or_else(|| {
                (!entry.title.series)
                    .then(|| self.movie_progress.get(&entry.title.id).cloned())
                    .flatten()
            })
            .unwrap_or(Value::Null);
        Some(item)
    }
}

fn card_json(series: bool, row: &CardRow) -> Result<Value, ApiError> {
    let [backdrop_size] = BACKDROP_SIZES else {
        unreachable!("one backdrop size")
    };
    let [logo_size] = LOGO_SIZES else {
        unreachable!("one logo size")
    };
    let genres: Value = match &row.genres {
        Some(json) => serde_json::from_str(json)?,
        None => json!([]),
    };
    Ok(json!({
        "kind": if series { "series" } else { "movie" },
        "id": row.id,
        "title": row.title,
        "year": row.year,
        "genres": genres,
        "runtime_minutes": row.runtime_minutes,
        "overview": row.overview,
        "poster": artwork_url("w342", row.poster_path.clone()),
        "backdrop": artwork_url(backdrop_size, row.backdrop_path.clone()),
        "backdrop_focus": focus(row.backdrop_x, row.backdrop_y),
        "textless_backdrop": artwork_url(backdrop_size, row.textless_backdrop_path.clone()),
        "textless_backdrop_focus": focus(row.textless_x, row.textless_y),
        "logo": artwork_url(logo_size, row.logo_path.clone()),
        "added_at": row.added_at,
    }))
}

/// Present movies (or series) among `ids` that `access` sees, with their stored artwork (a
/// title folder's own first) and when the first of their present files came in.
async fn card_rows(
    pool: &SqlitePool,
    access: &Access,
    series: bool,
    ids: &[i64],
) -> sqlx::Result<Vec<CardRow>> {
    let seen = if series {
        access.series_sql("x")
    } else {
        access.movie_sql("x")
    };
    let (titles, metadata, name, runtime, file_column) = if series {
        ("series", "tmdb_series", "name", "NULL", "series_id")
    } else {
        (
            "movies",
            "tmdb_movies",
            "title",
            "t.runtime_minutes",
            "movie_id",
        )
    };
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT x.id, COALESCE(t.{name}, x.parsed_title) AS title,
             COALESCE(t.year, x.parsed_year) AS year, t.overview,
             {runtime} AS runtime_minutes, t.genres,
             COALESCE(x.local_poster_key, t.poster_path) AS poster_path,
             COALESCE(x.local_backdrop_key, t.backdrop_path) AS backdrop_path,
             t.textless_backdrop_path, t.logo_path,
             fb.x AS backdrop_x, fb.y AS backdrop_y, ft.x AS textless_x, ft.y AS textless_y,
             (SELECT min(coalesce(f.added_at, f.modified_at)) FROM media_files f
              WHERE f.{file_column} = x.id AND f.missing_since IS NULL) AS added_at
         FROM {titles} x
         LEFT JOIN {metadata} t ON t.tmdb_id = x.match_tmdb_id
         LEFT JOIN artwork_focus fb ON fb.file = COALESCE(x.local_backdrop_key, t.backdrop_path)
         LEFT JOIN artwork_focus ft ON ft.file = t.textless_backdrop_path
         WHERE x.missing_since IS NULL AND {seen} AND x.id IN (SELECT value FROM json_each(?))"
    )))
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await
}

/// For each movie among `ids`, `me`'s progress on the present file they last played: where
/// they are, or that they've watched it.
async fn movie_progress(
    pool: &SqlitePool,
    me: i64,
    ids: &[i64],
) -> sqlx::Result<HashMap<i64, Value>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(i64, i64, f64, Option<f64>, bool, i64)> = sqlx::query_as(
        "SELECT movie_id, file_id, position_seconds, duration_seconds, watched, updated_at FROM (
             SELECT f.movie_id, p.file_id, p.position_seconds, p.duration_seconds, p.watched,
                 p.updated_at,
                 row_number() OVER (PARTITION BY f.movie_id
                                    ORDER BY p.updated_at DESC, p.file_id DESC) AS n
             FROM playback_progress p JOIN media_files f ON f.id = p.file_id
             WHERE p.user_id = ?1 AND f.missing_since IS NULL
               AND (p.position_seconds > 0 OR p.watched = 1)
               AND f.movie_id IN (SELECT value FROM json_each(?2))
         ) WHERE n = 1",
    )
    .bind(me)
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(movie, file, position, duration, watched, at)| {
            let progress = json!({
                "file_id": file,
                "position_seconds": position,
                "duration_seconds": duration,
                "watched": watched,
                "updated_at": at,
            });
            (movie, progress)
        })
        .collect())
}
