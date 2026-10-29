//! Signing in through an outside identity provider (`crate::oidc`), off unless set up.
//! Each sign-in still ends in a Melete session of the device's own, as a password gives.
//!
//! The web UI starts one and is sent back to its page with its session cookie set. An app
//! starts one with `return_to`, its own URL scheme or a port on the device, and a secret only
//! it holds; it's sent back there with a one-time code, and redeems the code with the secret.
//! A user links their identity at the provider while signed in; only a linked identity signs
//! in, and only as its user.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Json, Router, middleware};
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{
    AppState, CurrentUser, address, bad_request, cookie_for, from_home, require_user,
};
use crate::auth::{self, LoginLimiter, TokenKind, User, token_hash};
use crate::oidc::{self, Attempt, BeginError, Client, Confirm, Identity, Purpose};
use crate::ways_in::{ClientAddr, Opens};

mod confirming;
mod identities;
mod linking;
mod redeeming;
mod returning;
mod starting;

use confirming::*;
use identities::*;
use linking::*;
use redeeming::*;
use returning::*;
use starting::*;

#[derive(Clone)]
struct OidcState {
    pool: SqlitePool,
    client: Option<Arc<Client>>,
    /// Linking asks for the password again; wrong ones count with a password change's.
    limiter: Arc<LoginLimiter>,
}

/// Ties an app's sign-in, finished at the provider, to the browser that will confirm it. Set on
/// the confirmation page and sent only with its own form (SameSite=Strict).
const CONFIRM_COOKIE: &str = "melete_oidc_confirm";

/// Ties a sign-in to the browser that started it; sent back by the provider's redirect, a
/// top-level navigation, so it's SameSite=Lax where the session cookie is Strict.
const BINDING_COOKIE: &str = "melete_oidc";

/// `limiter` is the one sign-in and password changes use: linking checks the password too, and
/// guesses add up across all of them.
pub fn routes(pool: SqlitePool, client: Option<Arc<Client>>, limiter: Arc<LoginLimiter>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let public = Router::new()
        .route("/api/v1/auth/oidc", get(about))
        .route("/api/v1/auth/oidc/start", post(start))
        .route("/api/v1/auth/oidc/callback", get(callback))
        .route("/api/v1/auth/oidc/confirm", post(confirm))
        .route("/api/v1/auth/oidc/redeem", post(redeem));
    let user = Router::new()
        .route(
            "/api/v1/auth/oidc/link",
            get(link_status).post(link).delete(unlink),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_user));
    public.merge(user).with_state(OidcState {
        pool,
        client,
        limiter,
    })
}

fn client(state: &OidcState) -> Result<&Arc<Client>, ApiError> {
    state.client.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "signing in through an identity provider isn't set up",
        )
    })
}

/// Whether the sign-in button shows, and what it calls the provider.
async fn about(State(state): State<OidcState>) -> Json<serde_json::Value> {
    Json(json!({
        "enabled": state.client.is_some(),
        "name": state.client.as_ref().map(|c| c.name()),
    }))
}

fn binding_cookie(value: Option<&str>, https: bool) -> String {
    let (value, max_age) = match value {
        Some(v) => (v, 600),
        None => ("", 0),
    };
    format!(
        "{BINDING_COOKIE}={value}; Path=/api/v1/auth/oidc; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if https { "; Secure" } else { "" }
    )
}

fn binding(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(BINDING_COOKIE)?.strip_prefix('='))
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests;
