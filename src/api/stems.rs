//! How far splitting the music library into parts has got, for admins.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::stems::{self, Coverage};

/// How far splitting has got, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/stems/status", get(status))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

async fn status(State(pool): State<SqlitePool>) -> Result<Json<Coverage>, ApiError> {
    Ok(Json(stems::coverage(&pool).await?))
}
