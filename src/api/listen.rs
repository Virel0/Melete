//! Listening together over HTTP (`crate::listen`), for everyone signed in: start a
//! session, join one by its code, change what's playing, leave. Each device in a session hears
//! of every change on the event stream (`listening`); these answers carry the server's clock
//! too, so a device can work out where the music is now.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::auth::LoginLimiter;
use crate::listen::{self, Refused};

#[derive(Clone)]
struct ListenState {
    pool: SqlitePool,
    /// Wrong codes count like wrong passwords.
    limiter: Arc<LoginLimiter>,
}

/// The listening-together routes with a limiter of their own, for tests.
#[cfg(test)]
pub fn routes(pool: SqlitePool) -> Router {
    routes_with(pool, Arc::default())
}

/// The listening-together routes, counting wrong codes in `limiter`, which outlasts the API
/// being set up afresh.
pub fn routes_with(pool: SqlitePool, limiter: Arc<LoginLimiter>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/sessions", get(list).post(start))
        .route("/api/v1/music/sessions/join", post(join))
        .route(
            "/api/v1/music/sessions/{id}",
            get(show).put(change).delete(end),
        )
        .route("/api/v1/music/sessions/{id}/leave", post(leave))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(ListenState { pool, limiter })
}

#[derive(Deserialize)]
struct StateBody {
    queue: Vec<i64>,
    index: i64,
    position_ms: i64,
    playing: bool,
}

impl StateBody {
    fn state(self) -> listen::State {
        listen::State {
            queue: self.queue,
            index: self.index,
            position_ms: self.position_ms,
            playing: self.playing,
        }
    }
}

fn refused(refused: Refused) -> ApiError {
    match refused {
        Refused::NotFound => ApiError::new(StatusCode::NOT_FOUND, "no such session"),
        Refused::Stale => ApiError::new(
            StatusCode::CONFLICT,
            "it changed since; get it again and make the change against its new version",
        ),
        Refused::Invalid(why) => ApiError::new(StatusCode::BAD_REQUEST, why),
    }
}

/// A session with the server's clock beside it.
async fn answer(state: &ListenState, token_id: i64, id: i64) -> Result<Json<Value>, ApiError> {
    let session = listen::one(&state.pool, token_id, id)
        .await?
        .ok_or_else(|| refused(Refused::NotFound))?;
    Ok(Json(
        json!({ "session": session, "server_time": crate::db::now_ms() }),
    ))
}

/// The sessions this device is in.
async fn list(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        listen::for_device(&state.pool, current.token_id).await?
    )))
}

async fn start(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<StateBody>,
) -> Result<Response, ApiError> {
    let id = listen::start(
        &state.pool,
        current.user.id,
        current.token_id,
        &body.state(),
    )
    .await?
    .map_err(refused)?;
    let body = answer(&state, current.token_id, id).await?;
    Ok((StatusCode::CREATED, body).into_response())
}

#[derive(Deserialize)]
struct JoinBody {
    code: String,
}

async fn join(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<JoinBody>,
) -> Result<Json<Value>, ApiError> {
    let key = format!("listen|{}", current.user.id);
    if !state.limiter.begin(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong codes; try again in 15 minutes",
        ));
    }
    let Some(id) = listen::join(&state.pool, current.token_id, &body.code).await? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no session has that code; check it with whoever's listening",
        ));
    };
    state.limiter.succeeded(&key);
    answer(&state, current.token_id, id).await
}

async fn show(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    answer(&state, current.token_id, id).await
}

#[derive(Deserialize)]
struct ChangeBody {
    /// The version the change was made against.
    version: i64,
    #[serde(flatten)]
    state: StateBody,
}

async fn change(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<ChangeBody>,
) -> Result<Json<Value>, ApiError> {
    listen::set(
        &state.pool,
        current.token_id,
        id,
        body.version,
        &body.state.state(),
    )
    .await?
    .map_err(refused)?;
    answer(&state, current.token_id, id).await
}

async fn leave(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if listen::leave(&state.pool, current.token_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(refused(Refused::NotFound))
    }
}

async fn end(
    State(state): State<ListenState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    listen::end(
        &state.pool,
        current.token_id,
        current.user.id,
        current.user.is_admin,
        id,
    )
    .await?
    .map_err(|r| match r {
        Refused::Invalid(why) => ApiError::new(StatusCode::FORBIDDEN, why),
        other => refused(other),
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let request = match body {
            Some(b) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => request.body(Body::empty()),
        }
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
    async fn two_devices_listen_together() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation) VALUES (1, 1, 'k', 'A', 'B', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres)
                 VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'One', '[]'),
                        (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Two', '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tokens = Vec::new();
        for name in ["alice", "bob"] {
            let user = auth::create_user(&pool, name, "a-long-password".into(), false)
                .await
                .unwrap()
                .unwrap();
            tokens.push(
                auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                    .await
                    .unwrap()
                    .1,
            );
        }
        let (alice, bob) = (&tokens[0], &tokens[1]);
        let app = routes(pool);

        let (status, started) = send(
            &app,
            "POST",
            "/api/v1/music/sessions",
            alice,
            Some(json!({ "queue": [1, 2], "index": 0, "position_ms": 0, "playing": true })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{started}");
        assert!(started["server_time"].as_i64().unwrap() > 0);
        let id = started["session"]["id"].as_i64().unwrap();
        let code = started["session"]["code"].as_str().unwrap().to_owned();
        let uri = format!("/api/v1/music/sessions/{id}");
        assert_eq!(
            send(&app, "GET", &uri, bob, None).await.0,
            StatusCode::NOT_FOUND
        );

        let (status, joined) = send(
            &app,
            "POST",
            "/api/v1/music/sessions/join",
            bob,
            Some(json!({ "code": code })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{joined}");
        assert_eq!(joined["session"]["members"].as_array().unwrap().len(), 2);
        let (_, mine) = send(&app, "GET", "/api/v1/music/sessions", bob, None).await;
        assert_eq!(mine.as_array().unwrap().len(), 1);

        // Bob skips to the next track; Alice, a step behind, is told it moved on.
        let skip =
            json!({ "version": 1, "queue": [1, 2], "index": 1, "position_ms": 0, "playing": true });
        let (status, skipped) = send(&app, "PUT", &uri, bob, Some(skip.clone())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(skipped["session"]["index"], 1);
        assert_eq!(
            send(&app, "PUT", &uri, alice, Some(skip)).await.0,
            StatusCode::CONFLICT
        );

        // Only Alice ends it for everyone; Bob can leave.
        assert_eq!(
            send(&app, "DELETE", &uri, bob, None).await.0,
            StatusCode::FORBIDDEN
        );
        let leave = format!("{uri}/leave");
        assert_eq!(
            send(&app, "POST", &leave, bob, None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&app, "DELETE", &uri, alice, None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&app, "GET", &uri, alice, None).await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn wrong_codes_are_limited() {
        let pool = crate::db::open_in_memory().await;
        let user = auth::create_user(&pool, "bob", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        let app = routes(pool);
        for _ in 0..5 {
            let (status, _) = send(
                &app,
                "POST",
                "/api/v1/music/sessions/join",
                &token,
                Some(json!({ "code": "AAAAAA" })),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND);
        }
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/music/sessions/join",
            &token,
            Some(json!({ "code": "AAAAAA" })),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    }
}
