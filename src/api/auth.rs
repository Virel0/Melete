//! Sign-in, accounts and tokens over HTTP. Every request after login carries
//! `Authorization: Bearer <token>`, from either a login session or an API key. The web UI
//! signs in to a cookie instead, since `<img>` can't send a header.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
pub(super) use super::bad_request;
use crate::auth::{self, LoginLimiter, Scope, TokenKind, User};
use crate::quick_connect::{self, LookupError, Poll, QuickConnect, StartError};
use crate::two_factor;
use crate::ways_in::{ClientAddr, Opens, Part};

mod account;
mod quick;
mod second_factor;
mod signing_in;
mod users;

use account::*;
use quick::*;
pub(super) use signing_in::*;
use users::*;

/// What the sign-in routes and the auth layers share.
#[derive(Clone)]
pub struct AppState {
    /// The database.
    pub pool: SqlitePool,
    /// Password attempts, shared by every route that checks a password.
    pub limiter: Arc<LoginLimiter>,
}

/// The signed-in user and the token they used, set by [`require_user`].
#[derive(Clone)]
pub struct CurrentUser {
    /// Who signed in.
    pub user: User,
    /// The id of the session or API key the request came with.
    pub token_id: i64,
    /// What that token may reach.
    pub scope: Scope,
    /// What they may see, read afresh on every request as their admin status is.
    pub access: crate::access::Access,
}

/// Quick Connect's codes, beside what signing in needs.
#[derive(Clone)]
struct QuickState {
    auth: AppState,
    quick: Arc<QuickConnect>,
}

/// Sign-in and accounts, with Quick Connect's codes waiting in `quick`, which outlasts the API
/// being set up afresh.
pub fn routes(state: AppState, quick: Arc<QuickConnect>) -> Router {
    let public = Router::new()
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/web-login", post(web_login))
        .route(
            "/api/v1/auth/login/second-factor",
            post(second_factor::login_code),
        )
        .route(
            "/api/v1/auth/web-login/second-factor",
            post(second_factor::web_login_code),
        );
    let quick = QuickState {
        auth: state.clone(),
        quick,
    };
    let quick = Router::new()
        .route("/api/v1/auth/quick-connect", post(quick_start))
        .route("/api/v1/auth/quick-connect/poll", post(quick_poll))
        .merge(
            Router::new()
                .route("/api/v1/auth/quick-connect/lookup", post(quick_lookup))
                .route("/api/v1/auth/quick-connect/approve", post(quick_approve))
                .route_layer(middleware::from_fn_with_state(state.clone(), require_user)),
        )
        .with_state(quick);
    let user = Router::new()
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/me", get(me))
        .route("/api/v1/me/password", put(change_password))
        .route("/api/v1/me/two-factor", get(second_factor::status))
        .route("/api/v1/me/two-factor/setup", post(second_factor::setup))
        .route(
            "/api/v1/me/two-factor/confirm",
            post(second_factor::confirm),
        )
        .route("/api/v1/me/two-factor/off", post(second_factor::turn_off))
        .route("/api/v1/auth/tokens", get(list_tokens))
        .route("/api/v1/auth/tokens/{id}", delete(revoke_token))
        .route("/api/v1/auth/keys", post(create_key))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_user));
    let admin = Router::new()
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/{id}", delete(delete_user).put(change_user))
        .route("/api/v1/users/{id}/password", post(reset_password))
        .route("/api/v1/users/{id}/keys", get(list_user_keys))
        .route("/api/v1/users/{id}/keys/{key_id}", delete(revoke_user_key))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_admin));
    public
        .merge(user)
        .merge(admin)
        .with_state(state)
        .merge(quick)
}

/// A layer letting through only requests from a signed-in user, by bearer token or (for the web
/// UI) the session cookie, and handing the handler their [`CurrentUser`]. An API key that isn't
/// full may read (GET and HEAD) if it's for reading or playback, and change nothing.
///
/// # Errors
/// 401 without a token or with one that isn't valid; 403 for a cookie on a request that changes
/// something without the [`WEB_HEADER`], and for an API key whose scope doesn't reach the route.
pub async fn require_user(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let needed = if request.method().is_safe() {
        Scope::Read
    } else {
        Scope::Full
    };
    #[cfg(test)]
    note_scope(&request, needed);
    let current = signed_in(&state, request.method(), request.headers(), needed).await?;
    Ok(run(current, request, next).await)
}

/// [`require_user`] for routes that play and report progress: an API key for playback may also
/// use the ones that change something.
///
/// # Errors
/// As [`require_user`].
pub async fn require_playback(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let needed = if request.method().is_safe() {
        Scope::Read
    } else {
        Scope::Playback
    };
    #[cfg(test)]
    note_scope(&request, needed);
    let current = signed_in(&state, request.method(), request.headers(), needed).await?;
    Ok(run(current, request, next).await)
}

/// [`require_user`] for Aoide's routes, which an API key for music sync reaches and no other
/// key that isn't full.
///
/// # Errors
/// As [`require_user`].
pub async fn require_music_sync(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    #[cfg(test)]
    note_scope(&request, Scope::MusicSync);
    let current = signed_in(
        &state,
        request.method(),
        request.headers(),
        Scope::MusicSync,
    )
    .await?;
    Ok(run(current, request, next).await)
}

/// [`require_user`], for admins only, and only with a full token.
///
/// # Errors
/// As [`require_user`], and 403 for a signed-in user who isn't an admin.
pub async fn require_admin(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    #[cfg(test)]
    note_admin_route(&request);
    #[cfg(test)]
    note_scope(&request, Scope::Full);
    let current = signed_in(&state, request.method(), request.headers(), Scope::Full).await?;
    if !current.user.is_admin {
        return Err(ApiError::new(StatusCode::FORBIDDEN, "admins only"));
    }
    Ok(run(current, request, next).await)
}

/// Who's signed in for a request, when their token reaches what the route needs.
async fn signed_in(
    state: &AppState,
    method: &Method,
    headers: &HeaderMap,
    needed: Scope,
) -> Result<CurrentUser, ApiError> {
    let current = authenticate(&state.pool, method, headers).await?;
    if !current.scope.reaches(needed) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            format!(
                "this API key is for {}, which doesn't reach this",
                current.scope.as_str()
            ),
        ));
    }
    Ok(current)
}

async fn run(current: CurrentUser, mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(current);
    next.run(request).await
}

#[cfg(test)]
thread_local! {
    /// Every route [`require_admin`] has guarded on this thread, as "METHOD /path/{pattern}", so
    /// a test can learn which routes are admins' from where the code puts the guard rather than
    /// from a list kept by hand. Per thread, so tests running side by side don't mix.
    pub(super) static ADMIN_GUARDED: std::cell::RefCell<std::collections::BTreeSet<String>> =
        const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

#[cfg(test)]
thread_local! {
    /// What each route guarded on this thread needs of a token's scope, by "METHOD /path".
    pub(super) static SCOPE_NEEDED: std::cell::RefCell<std::collections::BTreeMap<String, Scope>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

#[cfg(test)]
fn note_scope(request: &Request, needed: Scope) {
    if let Some(path) = request.extensions().get::<axum::extract::MatchedPath>() {
        let route = format!("{} {}", request.method(), path.as_str());
        SCOPE_NEEDED.with(|seen| seen.borrow_mut().insert(route, needed));
    }
}

#[cfg(test)]
fn note_admin_route(request: &Request) {
    if let Some(path) = request.extensions().get::<axum::extract::MatchedPath>() {
        let route = format!("{} {}", request.method(), path.as_str());
        ADMIN_GUARDED.with(|guarded| guarded.borrow_mut().insert(route));
    }
}

/// The web UI's session cookie.
const COOKIE: &str = "melete_session";
/// The header the web UI sends with every request. A page on another site can't add it
/// without the browser asking first (and Melete never says yes), so a cookie only counts for
/// requests that change something when it's there.
pub const WEB_HEADER: &str = "x-melete-web";

/// The bearer token, or else the session cookie.
async fn authenticate(
    pool: &SqlitePool,
    method: &Method,
    headers: &HeaderMap,
) -> Result<CurrentUser, ApiError> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim());
    let token = match bearer {
        Some(token) => token,
        None => {
            let Some(token) = session_cookie(headers) else {
                return Err(ApiError::new(StatusCode::UNAUTHORIZED, "sign in first"));
            };
            if !method.is_safe() && !headers.contains_key(WEB_HEADER) {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "requests signed in by cookie that change something need the X-Melete-Web header",
                ));
            }
            token
        }
    };
    match auth::token_holder(pool, token).await? {
        Some(holder) => {
            let user = holder.user;
            let access = crate::access::Access::of(pool, user.id, user.is_admin).await?;
            Ok(CurrentUser {
                user,
                token_id: holder.token_id,
                scope: holder.scope,
                access,
            })
        }
        None => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "this token is not valid; sign in again",
        )),
    }
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .filter(|token| !token.is_empty())
}

/// The cookie holding `token`, or clearing it when there's none. It lasts as long as a session
/// can go unused; `Secure` when the page was loaded over https (a plain-http cookie marked
/// `Secure` would never be sent back).
fn cookie(token: Option<&str>, headers: &HeaderMap) -> String {
    let https = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin.starts_with("https://"));
    cookie_for(token, https)
}

/// The session cookie, for a page known to be on https or not.
pub(super) fn cookie_for(token: Option<&str>, https: bool) -> String {
    let (value, max_age) = match token {
        Some(token) => (token, 90 * 24 * 60 * 60),
        None => ("", 0),
    };
    format!(
        "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if https { "; Secure" } else { "" }
    )
}

#[cfg(test)]
mod tests;
