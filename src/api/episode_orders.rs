//! Which order a series' episodes follow (admins only): TMDB's own, or one of its episode
//! groups. Listing a series' groups fetches them from TMDB the first time; only the series'
//! TMDB id and the groups' ids leave the server.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use super::matching::tmdb_error;
use crate::episode_order::{self, Chosen, ChosenBy};
use crate::tmdb;

#[derive(Clone)]
struct OrderState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
}

/// The episode order routes, for admins only.
pub fn routes(pool: SqlitePool, tmdb: Option<Arc<tmdb::Client>>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/series/{id}/orders", get(list))
        .route("/api/v1/series/{id}/order", put(set))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(OrderState { pool, tmdb })
}

/// The series' TMDB id: 404 unless it's present, 409 unless it's matched.
async fn tmdb_id_of(pool: &SqlitePool, id: i64) -> Result<i64, ApiError> {
    let found: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT match_tmdb_id FROM series WHERE id = ? AND missing_since IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match found {
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "no such series")),
        Some(None) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "the series isn't matched to TMDB, so it has no episode groups",
        )),
        Some(Some(tmdb_id)) => Ok(tmdb_id),
    }
}

/// The order a series follows, the group suggested, and every group with how it fits.
async fn orders(pool: &SqlitePool, id: i64, tmdb_id: i64) -> Result<Value, ApiError> {
    let fetched: Option<i64> =
        sqlx::query_scalar("SELECT episode_groups_fetched_at FROM tmdb_series WHERE tmdb_id = ?")
            .bind(tmdb_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(json!({
        "series_id": id,
        "tmdb_id": tmdb_id,
        "order": episode_order::current(pool, tmdb_id).await?,
        "suggested": episode_order::suggestion(pool, tmdb_id).await?,
        "groups": episode_order::offered(pool, tmdb_id).await?,
        "groups_fetched_at": fetched,
    }))
}

async fn list(
    State(state): State<OrderState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let tmdb_id = tmdb_id_of(&state.pool, id).await?;
    let asked: Option<Option<i64>> =
        sqlx::query_scalar("SELECT episode_groups_fetched_at FROM tmdb_series WHERE tmdb_id = ?")
            .bind(tmdb_id)
            .fetch_optional(&state.pool)
            .await?;
    // Never asked for: ask TMDB now, when it's set up.
    if let (Some(None), Some(tmdb)) = (asked, state.tmdb.as_deref()) {
        episode_order::fetch(&state.pool, tmdb, tmdb_id)
            .await
            .map_err(tmdb_error)?;
    }
    Ok(Json(orders(&state.pool, id, tmdb_id).await?))
}

#[derive(Deserialize)]
struct OrderRequest {
    /// The group to follow; null for TMDB's own order.
    group_id: Option<String>,
}

async fn set(
    State(state): State<OrderState>,
    Path(id): Path<i64>,
    Json(body): Json<OrderRequest>,
) -> Result<Json<Value>, ApiError> {
    let tmdb_id = tmdb_id_of(&state.pool, id).await?;
    let group = body.group_id.as_deref();
    if group.is_some_and(|g| !tmdb::is_group_id(g)) {
        return Err(super::errors::bad_request(
            "group_id isn't a TMDB episode group id",
        ));
    }
    let now = crate::db::unix_now();
    match episode_order::choose(&state.pool, tmdb_id, group, ChosenBy::Admin, now).await? {
        Chosen::Done => Ok(Json(orders(&state.pool, id, tmdb_id).await?)),
        Chosen::NoSuchGroup => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "the series has no such episode group; list them first",
        )),
        // Only Melete's own choices give way to one made before.
        Chosen::EarlierStands => Err(ApiError::new(StatusCode::CONFLICT, "already chosen")),
    }
}

#[cfg(test)]
mod tests;
