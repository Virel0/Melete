//! Signing in with a name and password, from an app or the web UI.

use super::*;

#[derive(Deserialize)]
pub(super) struct LoginRequest {
    pub(super) username: String,
    pub(super) password: String,
    /// Shown in the user's device list, e.g. "Living room iPad".
    pub(super) device_name: String,
}

pub(super) async fn login(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    client: Option<Extension<ClientAddr>>,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let (user, token) = sign_in(&state, address(client, peer), body).await?;
    Ok(Json(json!({ "token": token, "user": user })).into_response())
}

/// Signs in to a cookie the page's script can't read.
pub(super) async fn web_login(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    client: Option<Extension<ClientAddr>>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let (user, token) = sign_in(&state, address(client, peer), body).await?;
    let cookie = cookie(Some(&token), &headers);
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "user": user })),
    )
        .into_response())
}

/// Whether a request came in through the household's own way: one that opens everything,
/// admin included (or no ways in are set). Caps keep places for these.
pub(in super::super) fn from_home(opens: Option<Extension<Opens>>) -> bool {
    opens.is_none_or(|Extension(opens)| opens.has(Part::Admin))
}

/// The client's address: what a trusted reverse proxy says it is (`ways_in`), else the
/// connection's.
pub(in super::super) fn address(
    client: Option<Extension<ClientAddr>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> String {
    match (client, peer) {
        (Some(Extension(ClientAddr(ip))), _) => ip.to_string(),
        (None, Some(Extension(ConnectInfo(a)))) => a.ip().to_string(),
        (None, None) => "unknown".into(),
    }
}

/// Checks a password and issues a session. Lockouts count per username *and* address, so
/// someone elsewhere can't lock a user out; behind a trusted reverse proxy, the address is
/// the client's as the proxy says.
pub(super) async fn sign_in(
    state: &AppState,
    address: String,
    body: LoginRequest,
) -> Result<(User, String), ApiError> {
    let device = auth::validate_label(&body.device_name).map_err(bad_request)?;
    let wrong = || ApiError::new(StatusCode::UNAUTHORIZED, "wrong username or password");
    // A name that can't exist never reaches the limiter or the password check.
    if auth::validate_username(&body.username).is_err() {
        return Err(wrong());
    }
    let key = format!("login|{}|{address}", body.username.to_ascii_lowercase());
    if !state.limiter.begin(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many failed sign-ins; try again in 15 minutes",
        ));
    }
    let Some(user) = auth::check_password(&state.pool, &body.username, body.password).await? else {
        return Err(wrong());
    };
    state.limiter.succeeded(&key);
    let (_, token) = auth::issue_token(&state.pool, user.id, TokenKind::Session, device).await?;
    Ok((user, token))
}
