//! Favourite movies and series: a list of each user's own, the same on every device, newest
//! first. Kept apart from browse answers, which are the same for everyone, as progress is; a
//! change reaches the user's other devices through the event stream's `favourites`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::artwork_url;
use crate::access::Access;
use crate::db::unix_now;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

/// A kind of title a favourite can be.
#[derive(Clone, Copy)]
enum Kind {
    Movie,
    Series,
}

impl Kind {
    /// Its name in the API.
    fn name(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Series => "series",
        }
    }

    /// Its table, and its column in `favourites`. Constants only, never user input.
    fn columns(self) -> (&'static str, &'static str) {
        match self {
            Kind::Movie => ("movies", "movie_id"),
            Kind::Series => ("series", "series_id"),
        }
    }
}

/// What the routes for one favourite take: the pool, the user, and the title's id.
type Args = (State<SqlitePool>, Extension<CurrentUser>, Path<i64>);

/// The routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let one = |kind: Kind| {
        get(move |s, c, p| show((s, c, p), kind))
            .put(move |s, c, p| add((s, c, p), kind))
            .delete(move |s, c, p| remove((s, c, p), kind))
    };
    Router::new()
        .route("/api/v1/me/favourites", get(list))
        .route("/api/v1/me/favourites/movies/{id}", one(Kind::Movie))
        .route("/api/v1/me/favourites/series/{id}", one(Kind::Series))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// The version of a user's favourites, raised by each change; 0 for one who has never had any.
pub async fn version(pool: &SqlitePool, user_id: i64) -> sqlx::Result<i64> {
    let version: Option<i64> =
        sqlx::query_scalar("SELECT version FROM favourite_versions WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(version.unwrap_or(0))
}

/// What the event stream sends as `favourites`: none for a user who has never had any.
pub async fn event(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Option<String>> {
    let version = version(pool, user_id).await?;
    if version == 0 {
        return Ok(None);
    }
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM favourites WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(Some(
        json!({ "version": version, "total": total }).to_string(),
    ))
}

async fn bump(pool: &SqlitePool, user_id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO favourite_versions (user_id, version) VALUES (?, 1)
         ON CONFLICT (user_id) DO UPDATE SET version = version + 1",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Deserialize)]
struct ListQuery {
    /// `movie` or `series`; both without.
    kind: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct Row {
    movie_id: Option<i64>,
    series_id: Option<i64>,
    added_at: i64,
    title: String,
    year: Option<i64>,
    poster_path: Option<String>,
    match_status: Option<String>,
    present: bool,
}

/// What a favourite is, from its title's row, its TMDB match, and the favourite; only those
/// the user may see now.
fn from(access: &Access) -> String {
    format!(
        "FROM favourites f
         LEFT JOIN movies m ON m.id = f.movie_id
         LEFT JOIN tmdb_movies tm ON tm.tmdb_id = m.match_tmdb_id
         LEFT JOIN series s ON s.id = f.series_id
         LEFT JOIN tmdb_series ts ON ts.tmdb_id = s.match_tmdb_id
         WHERE f.user_id = ?1 AND (?2 IS NULL OR (?2 = 'movie') = (f.movie_id IS NOT NULL))
             AND ((f.movie_id IS NOT NULL AND {}) OR (f.series_id IS NOT NULL AND {}))",
        access.movie_sql("m"),
        access.series_sql("s"),
    )
}

async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if !(1..=MAX_LIMIT).contains(&limit) || offset < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "limit is 1 to 500 and offset is 0 or more",
        ));
    }
    if let Some(kind) = query.kind.as_deref()
        && kind != "movie"
        && kind != "series"
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "kind is movie or series",
        ));
    }
    let from = from(&current.access);
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(current.user.id)
        .bind(&query.kind)
        .fetch_one(&pool)
        .await?;
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT f.movie_id, f.series_id, f.added_at,
             COALESCE(tm.title, ts.name, m.parsed_title, s.parsed_title) AS title,
             COALESCE(tm.year, ts.year, m.parsed_year, s.parsed_year) AS year,
             COALESCE(tm.poster_path, ts.poster_path) AS poster_path,
             COALESCE(m.match_status, s.match_status) AS match_status,
             COALESCE(m.missing_since, s.missing_since) IS NULL AS present
         {from} ORDER BY f.added_at DESC, f.id DESC LIMIT ?3 OFFSET ?4"
    )))
    .bind(current.user.id)
    .bind(&query.kind)
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let (kind, id) = match (row.movie_id, row.series_id) {
                (Some(id), _) => (Kind::Movie, id),
                (_, id) => (Kind::Series, id.unwrap_or_default()),
            };
            json!({
                "kind": kind.name(),
                "id": id,
                "added_at": row.added_at,
                "title": row.title,
                "year": row.year,
                "poster": artwork_url("w342", row.poster_path),
                "matched": row.match_status.as_deref() == Some("matched"),
                "present": row.present,
            })
        })
        .collect();
    let version = version(&pool, current.user.id).await?;
    Ok(Json(
        json!({ "total": total, "version": version, "items": items }),
    ))
}

/// When the user made the title a favourite, if they have; a 404 for a title that isn't there
/// or that they may not see.
async fn added_at(
    pool: &SqlitePool,
    current: &CurrentUser,
    id: i64,
    kind: Kind,
) -> Result<Option<i64>, ApiError> {
    let (titles, column) = kind.columns();
    let seen = match kind {
        Kind::Movie => current.access.sees_movie(pool, id).await?,
        Kind::Series => current.access.sees_series(pool, id).await?,
    };
    let not_found = || ApiError::new(StatusCode::NOT_FOUND, format!("no such {}", kind.name()));
    if !seen {
        return Err(not_found());
    }
    let row: Option<Option<i64>> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT (SELECT added_at FROM favourites WHERE user_id = ? AND {column} = x.id)
         FROM {titles} x WHERE x.id = ?"
    )))
    .bind(current.user.id)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(not_found)
}

fn state(added_at: Option<i64>) -> Json<Value> {
    Json(json!({ "favourite": added_at.is_some(), "added_at": added_at }))
}

/// Whether the title is one of the user's favourites.
async fn show(
    (State(pool), Extension(current), Path(id)): Args,
    kind: Kind,
) -> Result<Json<Value>, ApiError> {
    Ok(state(added_at(&pool, &current, id, kind).await?))
}

/// Makes a title in the library a favourite; one already is keeps when it was added.
async fn add(
    (State(pool), Extension(current), Path(id)): Args,
    kind: Kind,
) -> Result<Json<Value>, ApiError> {
    let (titles, column) = kind.columns();
    let present: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {titles} WHERE id = ? AND missing_since IS NULL"
    )))
    .bind(id)
    .fetch_optional(&pool)
    .await?;
    let seen = match kind {
        Kind::Movie => current.access.sees_movie(&pool, id).await?,
        Kind::Series => current.access.sees_series(&pool, id).await?,
    };
    if present.is_none() || !seen {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            format!("no such {} in the library", kind.name()),
        ));
    }
    let added = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO favourites (user_id, {column}, added_at) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING"
    )))
    .bind(current.user.id)
    .bind(id)
    .bind(unix_now())
    .execute(&pool)
    .await?;
    if added.rows_affected() > 0 {
        bump(&pool, current.user.id).await?;
    }
    Ok(state(added_at(&pool, &current, id, kind).await?))
}

/// Takes a title off the user's favourites; one that isn't there is left as it is.
async fn remove(
    (State(pool), Extension(current), Path(id)): Args,
    kind: Kind,
) -> Result<Json<Value>, ApiError> {
    let (_, column) = kind.columns();
    let removed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM favourites WHERE user_id = ? AND {column} = ?"
    )))
    .bind(current.user.id)
    .bind(id)
    .execute(&pool)
    .await?;
    if removed.rows_affected() > 0 {
        bump(&pool, current.user.id).await?;
    }
    Ok(state(None))
}

#[cfg(test)]
mod tests;
