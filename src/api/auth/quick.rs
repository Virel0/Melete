//! Quick Connect: a device shows a code, a signed-in user approves it, and the device gets a
//! session.

use super::*;

#[derive(Deserialize)]
pub(super) struct QuickStart {
    /// Shown to whoever approves it and in their device list, e.g. "Living room TV".
    pub(super) device_name: String,
}

/// A device asks for a Quick Connect code to show. Nobody is signed in yet; the code waits
/// for someone who is to approve it.
pub(super) async fn quick_start(
    State(state): State<QuickState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    client: Option<Extension<ClientAddr>>,
    opens: Option<Extension<Opens>>,
    Json(body): Json<QuickStart>,
) -> Result<Response, ApiError> {
    let device = auth::validate_label(&body.device_name).map_err(bad_request)?;
    match state
        .quick
        .start(device, &address(client, peer), from_home(opens))?
    {
        Ok(started) => {
            let body = json!({
                "code": started.code,
                "secret": started.secret,
                "expires_in": quick_connect::LIFETIME.as_secs(),
            });
            Ok((StatusCode::CREATED, Json(body)).into_response())
        }
        Err(StartError::Busy) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many Quick Connect codes are waiting; try again in a few minutes",
        )),
    }
}

#[derive(Deserialize)]
pub(super) struct QuickPoll {
    pub(super) secret: String,
}

/// The device asks, with the secret only it was given, whether its code was approved; once
/// it was, it gets its session here, once.
pub(super) async fn quick_poll(
    State(state): State<QuickState>,
    Json(body): Json<QuickPoll>,
) -> Result<Response, ApiError> {
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "no such code, or it ran out; start again",
        )
    };
    match state.quick.poll(&body.secret) {
        Poll::Waiting => {
            Ok((StatusCode::ACCEPTED, Json(json!({ "status": "waiting" }))).into_response())
        }
        Poll::Unknown => Err(gone()),
        Poll::Approved {
            user_id,
            device_name,
        } => {
            // Whoever approved it may have been deleted since.
            let user: Option<User> =
                sqlx::query_as("SELECT id, username, is_admin FROM users WHERE id = ?")
                    .bind(user_id)
                    .fetch_optional(&state.auth.pool)
                    .await?;
            let Some(user) = user else {
                return Err(gone());
            };
            let (_, token) =
                auth::issue_token(&state.auth.pool, user.id, TokenKind::Session, &device_name)
                    .await?;
            Ok(Json(json!({ "token": token, "user": user })).into_response())
        }
    }
}

#[derive(Deserialize)]
pub(super) struct QuickCode {
    pub(super) code: String,
}

/// Someone signed in looks up the code a device shows: which device it is, where it asked
/// from and how long ago, to check before approving it. Each user gets ten lookups an hour,
/// found or not, so codes can't be guessed.
pub(super) async fn quick_lookup(
    State(state): State<QuickState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<QuickCode>,
) -> Result<Response, ApiError> {
    match state.quick.lookup(&body.code, current.user.id) {
        Ok(seen) => Ok(Json(json!({
            "device_name": seen.device_name,
            "address": seen.address,
            "age_seconds": seen.age.as_secs(),
            "expires_in": seen.expires_in.as_secs(),
        }))
        .into_response()),
        Err(LookupError::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no code waiting is that one; check it, or start again on the device",
        )),
        Err(LookupError::Limited) => Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "that's ten codes looked up this hour; try again later",
        )),
    }
}

/// Approves a code the same user has looked up, signing that device in as them.
pub(super) async fn quick_approve(
    State(state): State<QuickState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<QuickCode>,
) -> Result<Response, ApiError> {
    let Some(device_name) = state.quick.approve(&body.code, current.user.id) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no code you've looked up is waiting as that one; look it up again, or start again on the device",
        ));
    };
    Ok(Json(json!({ "device_name": device_name })).into_response())
}
