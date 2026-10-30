//! "Wrong file?" (admins only): setting aside a movie or episode file Melete brought in from a
//! download as the wrong one for its title, then searching again while it's monitored.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router, middleware};
use serde::Serialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::fulfil::Sources;
use crate::upgrades::{self, NotSetAside};

#[derive(Clone)]
struct WrongState {
    pool: SqlitePool,
    sources: Arc<Sources>,
}

/// What setting a file aside did.
#[derive(Debug, Serialize)]
struct SetAsideView {
    /// The trash entry, to put it back with `POST /api/v1/trash/{id}/restore`.
    trash_id: i64,
    /// The download it came from, now failed.
    download_id: i64,
    /// The release, now blocklisted.
    release: String,
    /// What searching again did, in words.
    search: String,
}

/// The route, for admins only.
pub fn routes(pool: SqlitePool, sources: Sources) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/files/{id}/wrong", post(set_aside))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(WrongState {
            pool,
            sources: Arc::new(sources),
        })
}

async fn set_aside(
    State(state): State<WrongState>,
    Path(id): Path<i64>,
) -> Result<Json<SetAsideView>, ApiError> {
    let aside = match upgrades::set_aside(&state.pool, id).await? {
        Ok(aside) => aside,
        Err(NotSetAside::NotFound) => {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "no such file"));
        }
        Err(NotSetAside::Adopted) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "Melete didn't bring this file in from a download: it's part of the library as it was, which Melete never moves",
            ));
        }
        Err(NotSetAside::Failed(why)) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("it couldn't be moved to the trash: {why}"),
            ));
        }
    };
    let search = match aside.target {
        Some(target) => crate::transfer::search_again(&state.pool, &state.sources, target).await?,
        None => "it isn't for a wanted title, so it wasn't searched for again".into(),
    };
    Ok(Json(SetAsideView {
        trash_id: aside.trash_id,
        download_id: aside.download_id,
        release: aside.release,
        search,
    }))
}

#[cfg(test)]
mod tests;
