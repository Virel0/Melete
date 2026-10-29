//! A franchise's films, in release order: those in the library always, and with
//! `MELETE_DISCOVERY` on the rest too, from TMDB's collection lookup (only the collection's id
//! leaves the server), each marked playable, requested or requestable.

use super::*;

/// A collection one of the library's films the user may see belongs to; 404 for any other.
pub(super) async fn collection(
    State(state): State<DiscoverState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let found: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT c.name, c.poster_path, c.backdrop_path FROM tmdb_collections c
             WHERE c.id = ? AND EXISTS (SELECT 1 FROM tmdb_movies t
                 JOIN movies m ON m.match_tmdb_id = t.tmdb_id AND m.missing_since IS NULL
                 WHERE t.collection_id = c.id AND {})",
            current.access.movie_sql("m")
        )))
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    let Some((name, poster, backdrop)) = found else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such collection"));
    };
    let looked_up = match (&state.tmdb, crate::settings::discovery(&state.pool).await?) {
        (Some(tmdb), true) => tmdb.collection(id).await.map_err(outside)?.map(|c| c.parts),
        _ => None,
    };
    let all_films = looked_up.is_some();
    let mut films = match looked_up {
        Some(parts) => parts,
        None => in_library(&state.pool, &current.access, id).await?,
    };
    // Release order; films without a date yet come last.
    films.sort_by(|a, b| {
        let date = |l: &Listed| l.release_date.clone().filter(|d| !d.is_empty());
        (date(a).is_none(), date(a), a.id).cmp(&(date(b).is_none(), date(b), b.id))
    });
    Ok(Json(json!({
        "id": id,
        "name": name,
        "poster": artwork_url("w342", poster),
        "backdrop": artwork_url("w1280", backdrop),
        "all_films": all_films,
        "items": items(&state, &current, Kind::Movie, films).await?,
    })))
}

/// A film's TMDB id, title, release date, summary and poster.
type FilmRow = (i64, String, Option<String>, Option<String>, Option<String>);

/// The collection's films in the library that the user may see, as TMDB's details gave them.
async fn in_library(
    pool: &SqlitePool,
    access: &crate::access::Access,
    id: i64,
) -> Result<Vec<Listed>, ApiError> {
    let rows: Vec<FilmRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT DISTINCT t.tmdb_id, t.title, t.release_date, t.overview, t.poster_path
             FROM tmdb_movies t
             JOIN movies m ON m.match_tmdb_id = t.tmdb_id AND m.missing_since IS NULL
             WHERE t.collection_id = ? AND {}",
        access.movie_sql("m")
    )))
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, title, release_date, overview, poster_path)| Listed {
            id,
            title: Some(title),
            release_date,
            overview,
            poster_path,
            ..Listed::default()
        })
        .collect())
}
