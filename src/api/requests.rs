//! Requests: anyone signed in asks for a title or for music and follows it; admins approve or
//! decline, choose where approved titles go, and whose requests are let through.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router, middleware};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin, require_user};
use crate::fulfil::{self, Sources};
use crate::musicbrainz;
use crate::playlist_import::{self, ImportSummary, ImportView};
use crate::request_search::{self, Candidate};
use crate::requests::{self, Choice, NewRequest, Refused, RequestView};

mod admin_settings;
mod asking;
mod deciding;
mod listing;
mod playlists;

use admin_settings::*;
use asking::*;
use deciding::*;
use listing::*;
use playlists::*;

#[derive(Clone)]
struct RequestState {
    pool: SqlitePool,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
    /// TMDB, and what fetches an approved request's title.
    sources: Sources,
}

/// Asking for things and following one's own requests, for everyone signed in; approving,
/// declining and the request settings, for admins only.
pub fn routes(
    pool: SqlitePool,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
    sources: Sources,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let state = RequestState {
        pool,
        musicbrainz,
        sources,
    };
    let users = Router::new()
        .route("/api/v1/requests", get(list).post(create))
        .route("/api/v1/requests/events", get(events))
        .route("/api/v1/requests/search", get(search))
        .route(
            "/api/v1/requests/imports",
            get(imports)
                .post(import)
                .layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            "/api/v1/requests/imports/{id}",
            get(one_import).delete(remove_import),
        )
        .route(
            "/api/v1/requests/{id}",
            get(one).put(change).delete(withdraw),
        )
        .route_layer(middleware::from_fn_with_state(auth.clone(), require_user));
    let admins = Router::new()
        .route("/api/v1/requests/{id}/approve", post(approve))
        .route("/api/v1/requests/{id}/decline", post(decline))
        .route("/api/v1/requests/{id}/reopen", post(reopen))
        .route(
            "/api/v1/requests/imports/{id}/approve",
            post(approve_import),
        )
        .route("/api/v1/requests/settings", get(settings))
        .route(
            "/api/v1/requests/settings/defaults/{kind}",
            put(set_defaults),
        )
        .route("/api/v1/requests/settings/users/{id}", put(set_user))
        .route_layer(middleware::from_fn_with_state(auth, require_admin));
    users.merge(admins).with_state(state)
}

pub(super) fn refused(refused: Refused) -> ApiError {
    let status = match refused {
        Refused::Invalid(_) => StatusCode::BAD_REQUEST,
        Refused::NotFound | Refused::NotYours => StatusCode::NOT_FOUND,
        Refused::Outside(_) => StatusCode::BAD_GATEWAY,
        Refused::NoMetadata
        | Refused::NoMusicBrainz
        | Refused::Already(_)
        | Refused::Decided
        | Refused::NoLibrary(_)
        | Refused::Unavailable(_)
        | Refused::Conflict(_) => StatusCode::CONFLICT,
    };
    // Someone else's request is as good as none, to anyone but an admin.
    let message = match refused {
        Refused::NotYours => Refused::NotFound.to_string(),
        other => other.to_string(),
    };
    ApiError::new(status, message)
}

#[cfg(test)]
mod tests;
