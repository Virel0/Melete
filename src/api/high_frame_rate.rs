//! The films high frame rate releases are wanted for (admins only): listed by TMDB id, and
//! switched one by one from a movie's Manage page. A movie's search turns down releases named
//! as converted to a high frame rate for every other film (`search::frame_rate`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::search::frame_rate;

/// The routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/high-frame-rate-films", get(list))
        .route("/api/v1/high-frame-rate-films/{tmdb_id}", put(set))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// The films high frame rate releases are wanted for, by TMDB id.
async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<i64>>, ApiError> {
    Ok(Json(frame_rate::allowed_films(&pool).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wanted {
    allowed: bool,
}

/// Wants high frame rate releases for a film, or stops wanting them.
async fn set(
    State(pool): State<SqlitePool>,
    Path(tmdb_id): Path<i64>,
    Json(wanted): Json<Wanted>,
) -> Result<StatusCode, ApiError> {
    if tmdb_id <= 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "tmdb_id is a TMDB movie id",
        ));
    }
    frame_rate::allow(&pool, tmdb_id, wanted.allowed).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
