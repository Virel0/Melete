//! The calendar: upcoming and recent episodes and releases of wanted titles.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::calendar::{self, Entry};

/// The calendar route, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/calendar", get(between))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

#[derive(Deserialize)]
struct Range {
    start: Option<String>,
    end: Option<String>,
}

#[derive(Serialize)]
struct Calendar {
    start: String,
    end: String,
    entries: Vec<Entry>,
}

async fn between(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(range): Query<Range>,
) -> Result<Json<Calendar>, ApiError> {
    let (start, end) =
        match calendar::range(&pool, range.start.as_deref(), range.end.as_deref()).await? {
            Ok(range) => range,
            Err(why) => return Err(ApiError::new(StatusCode::BAD_REQUEST, why)),
        };
    let mut entries = calendar::between(&pool, &start, &end).await?;
    // Someone who doesn't see everything sees only titles in the library that they may see: a
    // title not on disk yet has neither a library nor a known rating.
    let access = &current.access;
    if !access.is_everything() {
        let mut seen = Vec::with_capacity(entries.len());
        for entry in entries {
            let visible = match (entry.movie_id, entry.series_id) {
                (Some(id), _) => access.sees_movie(&pool, id).await?,
                (_, Some(id)) => access.sees_series(&pool, id).await?,
                _ => false,
            };
            if visible {
                seen.push(entry);
            }
        }
        entries = seen;
    }
    Ok(Json(Calendar {
        start,
        end,
        entries,
    }))
}
