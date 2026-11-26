//! A library's Scan and search (admins only): starting one and following it.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin};
use crate::sweep::{self, Asked, Refused, Sources, Sweep};

#[derive(Clone)]
struct SweepState {
    pool: SqlitePool,
    sources: Sources,
}

/// Starting and following a library's Scan and search, for admins only: its searches made with
/// `search`, its subtitles kept in `subtitles`.
pub fn routes(
    pool: SqlitePool,
    search: crate::search_missing::Sources,
    subtitles: PathBuf,
) -> Router {
    let sources = Sources {
        search,
        subtitles,
        poll: sweep::POLL,
        through_jobs: true,
    };
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route(
            "/api/v1/libraries/{name}/scan-and-search",
            get(latest).post(start),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(SweepState { pool, sources })
}

fn yes() -> bool {
    true
}

/// What to do after the scan; each is done unless turned off.
#[derive(Deserialize)]
struct Start {
    #[serde(default = "yes")]
    missing: bool,
    #[serde(default = "yes")]
    upgrades: bool,
    #[serde(default = "yes")]
    subtitles: bool,
}

async fn start(
    State(state): State<SweepState>,
    Extension(current): Extension<CurrentUser>,
    Path(name): Path<String>,
    Json(body): Json<Start>,
) -> Result<(StatusCode, Json<Sweep>), ApiError> {
    let asked = Asked {
        missing: body.missing,
        upgrades: body.upgrades,
        subtitles: body.subtitles,
    };
    let started = sweep::start(
        &state.pool,
        state.sources.clone(),
        &name,
        asked,
        current.user.id,
    )
    .await?;
    match started {
        Ok(sweep) => Ok((StatusCode::ACCEPTED, Json(sweep))),
        Err(Refused::NoSuchLibrary) => Err(ApiError::new(StatusCode::NOT_FOUND, "no such library")),
        Err(Refused::Music) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "a music library's albums are searched for under Music, Search all missing",
        )),
        Err(Refused::NotResponding(why)) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("{why}; scan and search once it answers"),
        )),
        Err(Refused::Already(id)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("a scan and search of this library is under way already ({id})"),
        )),
    }
}

/// The library's latest Scan and search; null when it has had none.
async fn latest(
    State(state): State<SweepState>,
    Path(name): Path<String>,
) -> Result<Json<Option<Sweep>>, ApiError> {
    let known: Option<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE name = ?")
        .bind(&name)
        .fetch_optional(&state.pool)
        .await?;
    if known.is_none() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such library"));
    }
    Ok(Json(sweep::latest(&state.pool, &name).await?))
}
