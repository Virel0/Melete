//! A service's history, a title's own, and a title's past searches.

use axum::extract::Path;

use super::*;
use crate::history::{self, Service};

#[derive(Deserialize)]
pub(super) struct HistoryQuery {
    kind: Option<String>,
    wanted_id: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// Events, newest first.
#[derive(Serialize)]
pub(super) struct HistoryPage {
    total: i64,
    items: Vec<history::Event>,
}

fn service(path: &str) -> Result<Service, ApiError> {
    Service::parse(path).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "the service is movies, series, anime or artists",
        )
    })
}

fn no_title() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "no such wanted title in this service",
    )
}

pub(super) async fn history(
    State(pool): State<SqlitePool>,
    Path(path): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<HistoryPage>, ApiError> {
    let service = service(&path)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT as i64);
    let offset = query.offset.unwrap_or(0);
    if !(1..=MAX_LIMIT as i64).contains(&limit) || offset < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "limit is 1 to 500 and offset is 0 or more",
        ));
    }
    if query
        .kind
        .as_deref()
        .is_some_and(|k| !history::KINDS.contains(&k))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("kind is one of {}", history::KINDS.join(", ")),
        ));
    }
    let filter = history::Filter {
        kind: query.kind,
        wanted_id: query.wanted_id,
    };
    let (total, items) = history::list(&pool, service, &filter, limit, offset)
        .await?
        .ok_or_else(no_title)?;
    Ok(Json(HistoryPage { total, items }))
}

#[derive(Deserialize)]
pub(super) struct SearchesQuery {
    wanted_id: i64,
}

pub(super) async fn searches(
    State(pool): State<SqlitePool>,
    Path(path): Path<String>,
    Query(query): Query<SearchesQuery>,
) -> Result<Json<Vec<history::PastSearch>>, ApiError> {
    let service = service(&path)?;
    history::searches(&pool, service, query.wanted_id)
        .await?
        .map(Json)
        .ok_or_else(no_title)
}
