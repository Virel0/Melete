//! Import lists (admins only): lists of movies or shows whose titles join the wanted list.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::lists::{self, List, ListInput, Refused, Sources, SyncResult};

#[derive(Clone)]
struct ListState {
    pool: SqlitePool,
    /// None without a TMDB token: lists can be kept but not synced.
    sources: Option<Sources>,
}

/// The import list routes, for admins only. Without `sources`, lists can't be synced.
pub fn routes(pool: SqlitePool, sources: Option<Sources>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/lists", get(all).post(create))
        .route("/api/v1/lists/{id}", put(update).delete(delete))
        .route("/api/v1/lists/{id}/sync", post(sync))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(ListState { pool, sources })
}

fn refused(r: Refused) -> ApiError {
    match r {
        Refused::Invalid(why) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why),
        Refused::NotFound => not_found(),
        Refused::Taken => ApiError::new(StatusCode::CONFLICT, "a list has that name already"),
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such import list")
}

async fn all(State(state): State<ListState>) -> Result<Json<Vec<List>>, ApiError> {
    Ok(Json(lists::all(&state.pool).await?))
}

async fn create(
    State(state): State<ListState>,
    Json(input): Json<ListInput>,
) -> Result<(StatusCode, Json<List>), ApiError> {
    let id = lists::create(&state.pool, &input).await?.map_err(refused)?;
    let list = lists::get(&state.pool, id).await?.ok_or_else(not_found)?;
    Ok((StatusCode::CREATED, Json(list)))
}

async fn update(
    State(state): State<ListState>,
    Path(id): Path<i64>,
    Json(input): Json<ListInput>,
) -> Result<Json<List>, ApiError> {
    lists::update(&state.pool, id, &input)
        .await?
        .map_err(refused)?;
    lists::get(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(not_found)
}

async fn delete(
    State(state): State<ListState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if lists::delete(&state.pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

async fn sync(
    State(state): State<ListState>,
    Path(id): Path<i64>,
) -> Result<Json<SyncResult>, ApiError> {
    let Some(sources) = &state.sources else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "lists need TMDB to tell which titles they name; set MELETE_TMDB_TOKEN",
        ));
    };
    match lists::sync(&state.pool, sources, id).await {
        Ok(Some(result)) => Ok(Json(result)),
        Ok(None) => Err(not_found()),
        // Recorded as the list's last_result too.
        Err(err) => Err(ApiError::new(StatusCode::BAD_GATEWAY, format!("{err:#}"))),
    }
}
