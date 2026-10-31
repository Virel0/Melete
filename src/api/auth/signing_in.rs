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
    match sign_in(&state, address(client, peer), body, false).await? {
        SignedIn::Session(user, token) => {
            Ok(Json(json!({ "token": token, "user": user })).into_response())
        }
        SignedIn::NeedsCode(challenge) => Ok(code_needed(&challenge)),
    }
}

/// Signs in to a cookie the page's script can't read.
pub(super) async fn web_login(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    client: Option<Extension<ClientAddr>>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    match sign_in(&state, address(client, peer), body, true).await? {
        SignedIn::Session(user, token) => Ok(web_session(&user, &token, &headers)),
        SignedIn::NeedsCode(challenge) => Ok(code_needed(&challenge)),
    }
}

/// The web UI's answer to a finished sign-in: the session in a cookie, the user in the body.
pub(super) fn web_session(user: &User, token: &str, headers: &HeaderMap) -> Response {
    let cookie = cookie(Some(token), headers);
    (
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "user": user })),
    )
        .into_response()
}

/// A right password from a user with a second factor: no session yet, but a challenge to
/// finish with their code. A 401 with a message, so an app that doesn't know this step shows
/// why it can't sign in.
fn code_needed(challenge: &str) -> Response {
    let body = json!({
        "error": "this account also asks for a code from an authenticator app; an app that can't ask for one signs in with Quick Connect",
        "second_factor": "totp",
        "challenge": challenge,
        "expires_in": two_factor::CHALLENGE_LIFETIME,
    });
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
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

/// Checks a password and issues a session, or a challenge for a user with a second factor
/// on. Lockouts count per username *and* address, so
/// someone elsewhere can't lock a user out; behind a trusted reverse proxy, the address is
/// the client's as the proxy says.
pub(super) async fn sign_in(
    state: &AppState,
    address: String,
    body: LoginRequest,
    for_web: bool,
) -> Result<SignedIn, ApiError> {
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
    // Asked only once the password was right, so a wrong one is answered as it always was.
    if two_factor::is_on(&state.pool, user.id).await? {
        let challenge = two_factor::start_challenge(&state.pool, user.id, device, for_web).await?;
        return Ok(SignedIn::NeedsCode(challenge));
    }
    let (_, token) = auth::issue_token(&state.pool, user.id, TokenKind::Session, device).await?;
    Ok(SignedIn::Session(user, token))
}

/// What a right password gave.
pub(super) enum SignedIn {
    /// A session for the device, and its user.
    Session(User, String),
    /// A challenge to finish with a code from the user's authenticator app.
    NeedsCode(String),
}
