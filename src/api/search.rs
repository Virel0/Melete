//! Searching indexers for wanted titles (admins only), and reading past searches. Results come
//! with their decisions and reasons; download links never leave the server.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::search::{self, Refused, SearchView, Target};
use crate::torznab;

#[derive(Clone)]
struct SearchState {
    pool: SqlitePool,
    /// For checking a movie named with a year one off.
    tmdb: Option<Arc<crate::tmdb::Client>>,
    /// Soulseek, asked beside the indexers for albums.
    slskd: Option<Arc<crate::slskd::Client>>,
}

/// The indexer search routes, for admins only.
pub fn routes(
    pool: SqlitePool,
    tmdb: Option<Arc<crate::tmdb::Client>>,
    slskd: Option<Arc<crate::slskd::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/wanted/movies/{id}/search", post(search_movie))
        .route("/api/v1/wanted/series/{id}/search", post(search_series))
        .route("/api/v1/wanted/albums/{id}/search", post(search_album))
        .route("/api/v1/wanted/songs/{id}/search", post(search_song))
        .route("/api/v1/searches/{id}", get(one))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(SearchState { pool, tmdb, slskd })
}

async fn run(state: &SearchState, target: Target) -> Result<Json<SearchView>, ApiError> {
    let client = torznab::Client::new()?;
    let searched = search::search(&state.pool, &client, state.tmdb.as_deref(), target).await?;
    view(&state.pool, searched).await
}

/// A finished search as shown, or why it couldn't run.
async fn view(
    pool: &SqlitePool,
    searched: Result<i64, Refused>,
) -> Result<Json<SearchView>, ApiError> {
    let id = match searched {
        Ok(id) => id,
        Err(refused @ Refused::NotWanted) => {
            return Err(ApiError::new(StatusCode::NOT_FOUND, refused.to_string()));
        }
        Err(refused) => return Err(ApiError::new(StatusCode::CONFLICT, refused.to_string())),
    };
    let view = search::get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("search {id} vanished"))?;
    Ok(Json(view))
}

/// Searches every indexer's music categories, and Soulseek when slskd is set up, for a wanted
/// album.
async fn search_album(
    State(state): State<SearchState>,
    Path(id): Path<i64>,
) -> Result<Json<SearchView>, ApiError> {
    let client = torznab::Client::new()?;
    let searched =
        crate::album_search::search(&state.pool, &client, state.slskd.as_deref(), id).await?;
    view(&state.pool, searched).await
}

/// Searches Soulseek for a wanted song; songs are found only there.
async fn search_song(
    State(state): State<SearchState>,
    Path(id): Path<i64>,
) -> Result<Json<SearchView>, ApiError> {
    let Some(slskd) = state.slskd.as_deref() else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "songs are searched for on Soulseek, and slskd isn't set up (MELETE_SLSKD_URL)",
        ));
    };
    let searched = crate::song_search::search(&state.pool, slskd, id).await?;
    view(&state.pool, searched).await
}

async fn search_movie(
    State(state): State<SearchState>,
    Path(id): Path<i64>,
) -> Result<Json<SearchView>, ApiError> {
    run(&state, Target::Movie(id)).await
}

#[derive(Deserialize)]
struct SeriesSearch {
    season: i64,
    /// Leave out to search for the whole season.
    episode: Option<i64>,
}

async fn search_series(
    State(state): State<SearchState>,
    Path(id): Path<i64>,
    Json(request): Json<SeriesSearch>,
) -> Result<Json<SearchView>, ApiError> {
    if !(0..=10_000).contains(&request.season)
        || request.episode.is_some_and(|e| !(0..=100_000).contains(&e))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "season or episode is out of range",
        ));
    }
    let target = Target::Series {
        id,
        season: request.season,
        episode: request.episode,
    };
    run(&state, target).await
}

async fn one(
    State(state): State<SearchState>,
    Path(id): Path<i64>,
) -> Result<Json<SearchView>, ApiError> {
    search::get(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such search"))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    #[tokio::test]
    async fn songs_are_searched_for_only_through_slskd() {
        let pool = db::open_in_memory().await;
        let user = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/wanted/songs/1/search")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = routes(pool, None, None).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("slskd isn't set up"),
            "{body}"
        );
    }
}
