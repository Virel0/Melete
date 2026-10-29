//! A title's trailer online (`MELETE_TRAILERS_ONLINE`, off by default): the one chosen from
//! TMDB's video list, for an app to play in YouTube's privacy-enhanced player. Only for a title
//! the user may see, and only while the setting is on; the page's content security policy lets
//! that player's frame in only then too (`web::with_trailer_frames`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::metadata::{Trailer, chosen};

/// Where the player is loaded from: YouTube's privacy-enhanced mode, no cookies before playback.
pub const PLAYER: &str = "https://www.youtube-nocookie.com";

/// A movie's and a series' trailer, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/movies/{id}/trailer", get(movie_trailer))
        .route("/api/v1/series/{id}/trailer", get(series_trailer))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

async fn movie_trailer(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let sql = format!(
        "SELECT t.videos, coalesce(t.language, 'en-US') FROM movies x
         LEFT JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
         WHERE x.id = ? AND x.missing_since IS NULL AND {}",
        current.access.movie_sql("x")
    );
    trailer(&pool, &sql, id, "movie").await
}

async fn series_trailer(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let sql = format!(
        "SELECT t.videos, coalesce(t.language, 'en-US') FROM series x
         LEFT JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
         WHERE x.id = ? AND x.missing_since IS NULL AND {}",
        current.access.series_sql("x")
    );
    trailer(&pool, &sql, id, "series").await
}

/// The trailer of the title `sql` finds (its kept videos and their language), or 404: when
/// trailers online is off, the title isn't there or isn't the user's to see, or it has none.
async fn trailer(
    pool: &SqlitePool,
    sql: &str,
    id: i64,
    what: &str,
) -> Result<Json<Value>, ApiError> {
    let not_found = |why: String| ApiError::new(StatusCode::NOT_FOUND, why);
    if !crate::settings::trailers_online(pool).await? {
        return Err(not_found("trailers online are off".into()));
    }
    let row: Option<(Option<String>, String)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some((videos, language)) = row else {
        return Err(not_found(format!("no such {what}")));
    };
    let trailers: Vec<Trailer> = videos
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let Some(trailer) = chosen(&trailers, &language) else {
        return Err(not_found(format!("no trailer online for this {what}")));
    };
    Ok(Json(json!({
        "site": "youtube",
        "key": trailer.key,
        "name": trailer.name,
        "language": trailer.language,
        "embed_url": format!("{PLAYER}/embed/{}", trailer.key),
    })))
}

#[cfg(test)]
mod tests;
