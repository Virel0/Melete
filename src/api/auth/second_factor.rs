//! The second factor over HTTP: finishing a sign-in with a code, and a user setting it up,
//! confirming it and turning it off for their own account.

use super::*;
use crate::db::unix_now;
use crate::two_factor::{self, Confirmed, MadeFrom, Stage};

/// `reason` on a 403 or 401 whose code was wrong, for an app to point at the code field.
const WRONG_CODE: &str = "wrong_code";
/// `reason` on a 403 whose password was wrong, for an app to point at the password field.
const WRONG_PASSWORD: &str = "wrong_password";

/// The limiter's key for a user's codes, wherever they're typed. Per user rather than per
/// address: whoever is guessing already has the password, and more addresses mustn't buy
/// more guesses.
fn code_key(user_id: i64) -> String {
    format!("second-factor|{user_id}")
}

fn too_many_codes() -> ApiError {
    ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too many wrong codes; try again in 15 minutes",
    )
}

#[derive(Deserialize)]
pub(super) struct CodeRequest {
    /// What the password sign-in answered with.
    pub(super) challenge: String,
    /// The six digits the authenticator app shows.
    pub(super) code: String,
}

/// Finishes an app's sign-in with its code: a session, as the password alone would have given.
pub(super) async fn login_code(
    State(state): State<AppState>,
    Json(body): Json<CodeRequest>,
) -> Result<Response, ApiError> {
    let (user, token) = finish(&state, &body, false).await?;
    Ok(Json(json!({ "token": token, "user": user })).into_response())
}

/// Finishes the web UI's sign-in with its code, to the cookie.
pub(super) async fn web_login_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CodeRequest>,
) -> Result<Response, ApiError> {
    let (user, token) = finish(&state, &body, true).await?;
    Ok(web_session(&user, &token, &headers))
}

/// Checks the code against the challenge's user and, when it's right, uses the challenge up
/// and issues the session. A wrong code leaves the challenge for another try while it lasts.
async fn finish(
    state: &AppState,
    body: &CodeRequest,
    for_web: bool,
) -> Result<(User, String), ApiError> {
    let now = unix_now();
    let gone = || ApiError::new(StatusCode::GONE, "this sign-in has run out; sign in again");
    let Some(user_id) = two_factor::challenged(&state.pool, &body.challenge, for_web, now).await?
    else {
        return Err(gone());
    };
    let key = code_key(user_id);
    if !state.limiter.begin(&key) {
        return Err(too_many_codes());
    }
    if !two_factor::take_code(&state.pool, user_id, &body.code, now).await? {
        // In the user's own record, so someone whose password got out can tell.
        two_factor::record_wrong_code(&state.pool, user_id, now).await?;
        return Err(ApiError::reasoned(
            StatusCode::UNAUTHORIZED,
            WRONG_CODE,
            "wrong code",
        ));
    }
    state.limiter.succeeded(&key);
    let Some(device) = two_factor::finish_challenge(&state.pool, &body.challenge, now).await?
    else {
        return Err(gone());
    };
    let user = sqlx::query_as("SELECT id, username, is_admin FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(gone)?;
    let (_, token) = auth::issue_token(&state.pool, user_id, TokenKind::Session, &device).await?;
    Ok((user, token))
}

/// Whether the caller has a second factor on, with their record of turning it on and off.
pub(super) async fn status(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let status = two_factor::status(&state.pool, current.user.id).await?;
    let changes = two_factor::changes(&state.pool, current.user.id).await?;
    Ok(Json(json!({
        "on": status.on,
        "on_since": status.on_since,
        "changes": changes,
    })))
}

#[derive(Deserialize)]
pub(super) struct PasswordOnly {
    pub(super) password: String,
}

/// Checks the caller's password, counted with every other password check of theirs.
async fn check_own_password(
    state: &AppState,
    current: &CurrentUser,
    password: String,
) -> Result<(), ApiError> {
    let key = format!("password|{}", current.user.id);
    if !state.limiter.begin(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong passwords; try again in 15 minutes",
        ));
    }
    if auth::check_password(&state.pool, &current.user.username, password)
        .await?
        .is_none()
    {
        return Err(ApiError::reasoned(
            StatusCode::FORBIDDEN,
            WRONG_PASSWORD,
            "password is wrong",
        ));
    }
    state.limiter.succeeded(&key);
    Ok(())
}

/// Starts setting up: a new secret, shown this once and again only by starting over, until a
/// first code confirms it. Needs the password, so a session left signed in somewhere can't
/// put someone else's authenticator on the account.
pub(super) async fn setup(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<PasswordOnly>,
) -> Result<Response, ApiError> {
    check_own_password(&state, &current, body.password).await?;
    let Some(setup) = two_factor::start_setup(&state.pool, &current.user).await? else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "two-factor sign-in is on already; turn it off first",
        ));
    };
    // Drawn here, so the secret in the link goes nowhere else to become a picture.
    let qr = two_factor::qr::encode(setup.uri.as_bytes()).map(|code| code.svg());
    let body = Json(json!({ "secret": setup.secret, "uri": setup.uri, "qr": qr }));
    Ok(([(header::CACHE_CONTROL, "no-store")], body).into_response())
}

#[derive(Deserialize)]
pub(super) struct CodeOnly {
    pub(super) code: String,
}

/// Turns the second factor on with a first code from the secret setup gave. Nothing is signed
/// out. Only a code checked against a waiting setup counts against the user's codes: a confirm
/// with nothing to confirm mustn't lock their code step at sign-in.
pub(super) async fn confirm(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<CodeOnly>,
) -> Result<StatusCode, ApiError> {
    let nothing_waiting = || {
        ApiError::new(
            StatusCode::CONFLICT,
            "no setup is waiting for a code; start again",
        )
    };
    let already_on = || ApiError::new(StatusCode::CONFLICT, "two-factor sign-in is on already");
    match two_factor::stage(&state.pool, current.user.id).await? {
        Stage::Off => return Err(nothing_waiting()),
        Stage::On => return Err(already_on()),
        Stage::Waiting => {}
    }
    let key = code_key(current.user.id);
    if !state.limiter.begin(&key) {
        return Err(too_many_codes());
    }
    match two_factor::confirm(&state.pool, current.user.id, &body.code, unix_now()).await? {
        Confirmed::Done => {
            state.limiter.succeeded(&key);
            Ok(StatusCode::NO_CONTENT)
        }
        Confirmed::WrongCode => Err(ApiError::reasoned(
            StatusCode::FORBIDDEN,
            WRONG_CODE,
            "wrong code; check the app shows Melete's newest code and the device's clock is right",
        )),
        // Changed since it was looked at, by another request of theirs.
        Confirmed::NothingWaiting => Err(nothing_waiting()),
        Confirmed::AlreadyOn => Err(already_on()),
    }
}

#[derive(Deserialize)]
pub(super) struct TurnOff {
    pub(super) password: String,
    pub(super) code: String,
}

/// Turns the second factor off, with the password and a current code. Nothing is signed out.
pub(super) async fn turn_off(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<TurnOff>,
) -> Result<StatusCode, ApiError> {
    let user_id = current.user.id;
    if !two_factor::is_on(&state.pool, user_id).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "two-factor sign-in is off already",
        ));
    }
    check_own_password(&state, &current, body.password).await?;
    let key = code_key(user_id);
    if !state.limiter.begin(&key) {
        return Err(too_many_codes());
    }
    if !two_factor::take_code(&state.pool, user_id, &body.code, unix_now()).await? {
        return Err(ApiError::reasoned(
            StatusCode::FORBIDDEN,
            WRONG_CODE,
            "wrong code",
        ));
    }
    state.limiter.succeeded(&key);
    two_factor::remove(&state.pool, user_id, MadeFrom::Account).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
