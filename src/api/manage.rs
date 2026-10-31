//! The managed lists, one per service (admins only): `GET /api/v1/manage/movies`,
//! `/manage/series`, `/manage/anime` and `/manage/artists`. Each joins the library with the
//! wanted list, one row per title with its state, filtered, sorted and paged.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde::Serialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::manage::{self, Filter, Page};

/// Titles on a page unless asked otherwise, and at most.
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;

/// The managed lists' routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/manage/movies", get(movies))
        .route("/api/v1/manage/series", get(series))
        .route("/api/v1/manage/anime", get(anime))
        .route("/api/v1/manage/artists", get(artists))
        .route("/api/v1/manage/{service}/history", get(history::history))
        .route("/api/v1/manage/{service}/searches", get(history::searches))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// A list's query: [`Filter`]'s fields (spelled out, as a query string can't be flattened
/// into numbers and booleans), and the page.
#[derive(Deserialize)]
struct ListQuery {
    state: Option<manage::State>,
    monitored: Option<bool>,
    wanted: Option<bool>,
    library: Option<String>,
    category: Option<String>,
    q: Option<String>,
    cutoff_unmet: Option<bool>,
    wanted_id: Option<i64>,
    folder: Option<i64>,
    sort: Option<manage::Sort>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl ListQuery {
    /// The filter and page asked for, checked.
    fn checked(self) -> Result<(Filter, usize, usize), ApiError> {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "limit is 1 to 500 and offset is 0 or more",
            ));
        }
        if self
            .category
            .as_deref()
            .is_some_and(|c| !matches!(c, "anime" | "kids" | "none"))
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "category is anime, kids or none",
            ));
        }
        let filter = Filter {
            state: self.state,
            monitored: self.monitored,
            wanted: self.wanted,
            library: self.library,
            category: self.category,
            q: self.q.filter(|q| !q.trim().is_empty()),
            cutoff_unmet: self.cutoff_unmet,
            wanted_id: self.wanted_id,
            folder: self.folder,
            sort: self.sort.unwrap_or_default(),
        };
        Ok((filter, limit, self.offset.unwrap_or(0)))
    }
}

fn paged<D: Serialize + manage::Folder>(
    titles: Vec<manage::Title<D>>,
    query: ListQuery,
) -> Result<Json<Page<D>>, ApiError> {
    let (filter, limit, offset) = query.checked()?;
    Ok(Json(manage::page(titles, &filter, limit, offset)))
}

async fn movies(
    State(pool): State<SqlitePool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<manage::MovieDetail>>, ApiError> {
    let Json(mut page) = paged(manage::movies(&pool).await?, query)?;
    manage::qualities(&pool, &mut page.items).await?;
    Ok(Json(page))
}

async fn series(
    State(pool): State<SqlitePool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<manage::SeriesDetail>>, ApiError> {
    paged(manage::series(&pool, false).await?, query)
}

async fn anime(
    State(pool): State<SqlitePool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<manage::SeriesDetail>>, ApiError> {
    paged(manage::series(&pool, true).await?, query)
}

async fn artists(
    State(pool): State<SqlitePool>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<manage::ArtistDetail>>, ApiError> {
    if query.cutoff_unmet.is_some() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "music isn't upgraded, so no album is below a cutoff",
        ));
    }
    paged(manage::artists(&pool).await?, query)
}

mod history;
mod selection;

pub use selection::selection_routes;

#[cfg(test)]
mod tests;
