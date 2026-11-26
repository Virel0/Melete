//! Watching together over HTTP (`crate::watch_party`), for everyone who can play video: start
//! a party, join one by its code, change what plays, leave. Each device in a party hears of
//! every change on the event stream (`watch_party`); these answers carry the server's clock too,
//! so a device can work out where the film is now.

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
use super::auth::{AppState, CurrentUser, require_playback};
use super::playback::require_playable;
use crate::auth::LoginLimiter;
use crate::watch_party::{self, Refused, guests};

#[derive(Clone)]
struct PartyState {
    pool: SqlitePool,
    /// Wrong codes count like wrong passwords.
    limiter: Arc<LoginLimiter>,
    /// Melete's own tunnel, for guests who come through it.
    tunnel: Arc<crate::tunnel::Slot>,
}

/// The watch party routes with a limiter of their own, for tests.
#[cfg(test)]
pub fn routes(pool: SqlitePool) -> Router {
    routes_with(pool, Arc::default(), Arc::default())
}

/// The watch party routes, counting wrong codes in `limiter`, which outlasts the API being set
/// up afresh, and making guests' peers in `tunnel` once it's started.
pub fn routes_with(
    pool: SqlitePool,
    limiter: Arc<LoginLimiter>,
    tunnel: Arc<crate::tunnel::Slot>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/watch-parties", get(list).post(start))
        .route("/api/v1/watch-parties/join", post(join))
        .route(
            "/api/v1/watch-parties/{id}",
            get(show).put(change).delete(end),
        )
        .route("/api/v1/watch-parties/{id}/leave", post(leave))
        .route("/api/v1/watch-parties/{id}/invites", post(invite))
        .route(
            "/api/v1/watch-parties/{id}/tunnel-invites",
            post(tunnel_invites::make),
        )
        .route(
            "/api/v1/watch-parties/{id}/waiting",
            axum::routing::put(waiting),
        )
        .route(
            "/api/v1/watch-parties/{id}/guests/{guest}",
            axum::routing::delete(send_away),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_playback))
        .with_state(PartyState {
            pool,
            limiter,
            tunnel,
        })
}

#[derive(Deserialize)]
struct StateBody {
    file_id: i64,
    position_ms: i64,
    playing: bool,
}

impl StateBody {
    fn state(&self) -> watch_party::State {
        watch_party::State {
            file_id: self.file_id,
            position_ms: self.position_ms,
            playing: self.playing,
        }
    }
}

fn refused(refused: Refused) -> ApiError {
    match refused {
        Refused::NotFound => ApiError::new(StatusCode::NOT_FOUND, "no such watch party"),
        Refused::Stale => ApiError::new(
            StatusCode::CONFLICT,
            "it changed since; get it again and make the change against its new version",
        ),
        Refused::NotHost(why) => ApiError::new(StatusCode::FORBIDDEN, why),
        Refused::Invalid(why) => ApiError::new(StatusCode::BAD_REQUEST, why),
    }
}

/// A party with the server's clock beside it.
async fn answer(pool: &SqlitePool, token_id: i64, id: i64) -> Result<Json<Value>, ApiError> {
    let party = watch_party::one(pool, token_id, id)
        .await?
        .ok_or_else(|| refused(Refused::NotFound))?;
    Ok(Json(
        json!({ "party": party, "server_time": crate::db::now_ms() }),
    ))
}

/// The parties this device is in.
async fn list(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        watch_party::for_device(&state.pool, current.token_id).await?
    )))
}

#[derive(Deserialize)]
struct StartBody {
    #[serde(flatten)]
    state: StateBody,
    /// Guests' streams held to this many bits a second; null holds them to nothing more than
    /// everyone's.
    guest_max_bitrate: Option<i64>,
}

async fn start(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<StartBody>,
) -> Result<Response, ApiError> {
    require_playable(&state.pool, &current.access, body.state.file_id).await?;
    let id = watch_party::start(
        &state.pool,
        (current.user.id, current.token_id),
        &body.state.state(),
        body.guest_max_bitrate,
    )
    .await?
    .map_err(refused)?;
    let body = answer(&state.pool, current.token_id, id).await?;
    Ok((StatusCode::CREATED, body).into_response())
}

#[derive(Deserialize)]
struct JoinBody {
    code: String,
}

async fn join(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<JoinBody>,
) -> Result<Json<Value>, ApiError> {
    let key = format!("watch-party|{}", current.user.id);
    if !state.limiter.begin(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong codes; try again in 15 minutes",
        ));
    }
    let Some((id, file_id)) = watch_party::by_code(&state.pool, &body.code).await? else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no watch party has that code; check it with whoever started it",
        ));
    };
    state.limiter.succeeded(&key);
    // Only what this account may watch: a party isn't a way round a child's limits.
    require_playable(&state.pool, &current.access, file_id)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "this party plays something your account can't watch",
            )
        })?;
    watch_party::join(&state.pool, current.token_id, id).await?;
    answer(&state.pool, current.token_id, id).await
}

async fn show(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    answer(&state.pool, current.token_id, id).await
}

#[derive(Deserialize)]
struct ChangeBody {
    /// The version the change was made against.
    version: i64,
    #[serde(flatten)]
    state: StateBody,
}

async fn change(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<ChangeBody>,
) -> Result<Json<Value>, ApiError> {
    require_playable(&state.pool, &current.access, body.state.file_id).await?;
    watch_party::set(
        &state.pool,
        (current.user.id, current.token_id),
        id,
        body.version,
        &body.state.state(),
    )
    .await?
    .map_err(refused)?;
    answer(&state.pool, current.token_id, id).await
}

async fn leave(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if watch_party::leave(&state.pool, current.token_id, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(refused(Refused::NotFound))
    }
}

#[derive(Deserialize)]
struct WaitingBody {
    /// Whether this player is loading or buffering.
    waiting: bool,
}

/// This device's player is loading or buffering, or ready again: a playing party pauses for
/// it and plays on once everyone's ready, or after 30 seconds.
async fn waiting(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<WaitingBody>,
) -> Result<Json<Value>, ApiError> {
    if watch_party::one(&state.pool, current.token_id, id)
        .await?
        .is_none()
    {
        return Err(refused(Refused::NotFound));
    }
    let who = (
        watch_party::waits::Waiter::Device(current.token_id),
        current.user.username.as_str(),
    );
    watch_party::waits::set(&state.pool, id, who, body.waiting).await?;
    answer(&state.pool, current.token_id, id).await
}

/// Makes an invite link for someone with no account: it lets one guest into this party once,
/// within a day. The secret goes after `#`, so the link's opening never sends it to a server.
async fn invite(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let (secret, expires_at) = guests::invite(&state.pool, (current.user.id, current.token_id), id)
        .await?
        .map_err(refused)?;
    let body = json!({
        "invite": secret,
        "path": format!("/#/guest/{secret}"),
        "expires_at": expires_at,
    });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// Sends a guest away: their pass and stream links end at once.
async fn send_away(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path((id, guest)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    guests::send_away(&state.pool, current.token_id, id, guest)
        .await?
        .map_err(refused)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn end(
    State(state): State<PartyState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let who = (current.user.id, current.token_id, current.user.is_admin);
    watch_party::end(&state.pool, who, id)
        .await?
        .map_err(refused)?;
    Ok(StatusCode::NO_CONTENT)
}

mod tunnel_invites;

#[cfg(test)]
mod tests;
