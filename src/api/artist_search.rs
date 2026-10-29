//! Searching a whole artist (admins only): starting a run over every wanted album
//! of an artist, and watching it.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::artist_search::{self, ArtistSearch, NotStarted, Sources};

#[derive(Clone)]
struct RunState {
    pool: SqlitePool,
    sources: Sources,
}

/// Starting and watching artist searches, for admins only.
pub fn routes(pool: SqlitePool, sources: Sources) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/wanted/artists/{id}/search", post(start))
        .route("/api/v1/artist-searches/{id}", get(one))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(RunState { pool, sources })
}

#[derive(Deserialize)]
struct StartQuery {
    /// Whether the best accepted result of each album's search is grabbed (default yes).
    grab: Option<bool>,
}

async fn start(
    State(state): State<RunState>,
    Path(id): Path<i64>,
    Query(query): Query<StartQuery>,
) -> Result<(StatusCode, Json<ArtistSearch>), ApiError> {
    let started = artist_search::start(
        &state.pool,
        state.sources.clone(),
        id,
        query.grab.unwrap_or(true),
    )
    .await?;
    match started {
        Ok(run) => Ok((StatusCode::ACCEPTED, Json(run))),
        Err(NotStarted::NoArtist) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such wanted artist",
        )),
        Err(NotStarted::Already(run)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("a search of this artist is already under way (artist search {run})"),
        )),
    }
}

async fn one(
    State(state): State<RunState>,
    Path(id): Path<i64>,
) -> Result<Json<ArtistSearch>, ApiError> {
    artist_search::get(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such artist search"))
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

    async fn send(app: &Router, method: &str, uri: &str, token: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn starts_a_run_and_shows_how_it_went() {
        let pool = db::open_in_memory().await;
        let user = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        sqlx::raw_sql(
            "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
                 VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let sources = Sources {
            slskd: None,
            pipeline: None,
        };
        let app = routes(pool, sources);
        let (status, _) = send(&app, "POST", "/api/v1/wanted/artists/99/search", &token).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, run) = send(
            &app,
            "POST",
            "/api/v1/wanted/artists/1/search?grab=false",
            &token,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        assert_eq!(run["grab"], false);
        let uri = format!("/api/v1/artist-searches/{}", run["id"]);
        let mut shown = Value::Null;
        for _ in 0..200 {
            shown = send(&app, "GET", &uri, &token).await.1;
            if shown["state"] == "done" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(shown["state"], "done", "{shown}");
        // With no profile and nothing to search, it says why for the album.
        assert_eq!(shown["albums"][0]["title"], "OK Computer");
        assert!(shown["albums"][0]["error"].is_string(), "{shown}");
        let (status, _) = send(&app, "GET", "/api/v1/artist-searches/999", &token).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
