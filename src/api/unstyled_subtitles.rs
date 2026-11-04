//! A series' anime episodes without styled (ASS/SSA) subtitles, for its Manage page (admins
//! only): each with the release group of the copy in the library, so a search can turn that
//! group down and look for a release that carries them (`subtitles::unstyled`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde::Serialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::subtitles;

/// The routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/series/{id}/unstyled-subtitles", get(list))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// An episode without styled subtitles.
#[derive(Serialize)]
struct Episode {
    file_id: i64,
    season: i64,
    episode: i64,
    group: Option<String>,
}

/// The series' anime episodes without styled subtitles, by season and episode. Empty for a
/// series that isn't anime, or isn't in the library.
async fn list(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Episode>>, ApiError> {
    let found = subtitles::unstyled(&pool, Some(id)).await?;
    Ok(Json(
        found
            .into_iter()
            .map(|u| Episode {
                file_id: u.file_id,
                season: u.season,
                episode: u.episode,
                group: u.group,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests;
