//! RSS sync (admins only): its latest runs, with the searches they stored and what they
//! grabbed, and running one now.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::downloads::Pipeline;
use crate::rss::{self, SyncView};
use crate::torznab;

#[derive(Clone)]
struct RssState {
    pool: SqlitePool,
    tmdb: Option<Arc<crate::tmdb::Client>>,
    pipeline: Option<Arc<Pipeline>>,
}

/// The RSS sync routes, for admins only.
pub fn routes(
    pool: SqlitePool,
    tmdb: Option<Arc<crate::tmdb::Client>>,
    pipeline: Option<Arc<Pipeline>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/rss/sync", post(sync_now))
        .route("/api/v1/rss/syncs", get(list))
        .route("/api/v1/rss/syncs/{id}", get(one))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(RssState {
            pool,
            tmdb,
            pipeline,
        })
}

async fn sync_now(State(state): State<RssState>) -> Result<Json<SyncView>, ApiError> {
    let client = torznab::Client::new()?;
    let view = rss::sync(
        &state.pool,
        &client,
        state.tmdb.as_deref(),
        state.pipeline.as_deref(),
    )
    .await?;
    view.map(Json).ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "no indexers are turned on; import them from Prowlarr",
        )
    })
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
}

async fn list(
    State(state): State<RssState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<SyncView>>, ApiError> {
    let limit = query.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, "limit is 1 to 100"));
    }
    Ok(Json(rss::recent(&state.pool, limit).await?))
}

async fn one(
    State(state): State<RssState>,
    Path(id): Path<i64>,
) -> Result<Json<SyncView>, ApiError> {
    rss::get(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such RSS sync"))
}
