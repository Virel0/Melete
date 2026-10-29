//! Admins managing users: adding, removing, changing them and resetting passwords.

use super::*;

pub(super) async fn list_users(State(state): State<AppState>) -> Result<Json<Vec<User>>, ApiError> {
    Ok(Json(
        sqlx::query_as("SELECT id, username, is_admin FROM users ORDER BY username")
            .fetch_all(&state.pool)
            .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct NewUser {
    pub(super) username: String,
    pub(super) password: String,
    #[serde(default)]
    pub(super) is_admin: bool,
}

pub(super) async fn create_user(
    State(state): State<AppState>,
    Json(body): Json<NewUser>,
) -> Result<Response, ApiError> {
    auth::validate_username(&body.username).map_err(bad_request)?;
    auth::validate_password(&body.password).map_err(bad_request)?;
    match auth::create_user(&state.pool, &body.username, body.password, body.is_admin).await? {
        Some(user) => Ok((StatusCode::CREATED, Json(user)).into_response()),
        None => Err(ApiError::new(
            StatusCode::CONFLICT,
            "that username is taken",
        )),
    }
}

pub(super) async fn delete_user(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if id == current.user.id {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "you can't delete your own account",
        ));
    }
    match auth::delete_user(&state.pool, id).await? {
        auth::Deleted::Done => Ok(StatusCode::NO_CONTENT),
        auth::Deleted::NoSuchUser => Err(ApiError::new(StatusCode::NOT_FOUND, "no such user")),
        auth::Deleted::LastAdmin => Err(ApiError::new(
            StatusCode::CONFLICT,
            "that's the last admin; make someone else an admin first",
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UserChange {
    pub(super) username: String,
    pub(super) is_admin: bool,
}

/// Renames a user, or makes them an admin or takes it away; never the last admin's,
/// and never your own (another admin does that, so nobody locks themselves out by a slip).
pub(super) async fn change_user(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<UserChange>,
) -> Result<Json<User>, ApiError> {
    auth::validate_username(&body.username).map_err(bad_request)?;
    if id == current.user.id && !body.is_admin {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "you can't take away your own admin; ask another admin",
        ));
    }
    match auth::change_user(&state.pool, id, Some(&body.username), Some(body.is_admin)).await? {
        auth::Changed::Done(user) => Ok(Json(user)),
        auth::Changed::NoSuchUser => Err(ApiError::new(StatusCode::NOT_FOUND, "no such user")),
        auth::Changed::Taken => Err(ApiError::new(
            StatusCode::CONFLICT,
            "that username is taken",
        )),
        auth::Changed::LastAdmin => Err(ApiError::new(
            StatusCode::CONFLICT,
            "that's the last admin; make someone else an admin first",
        )),
    }
}

/// One of a user's API keys, as an admin sees it: never the key itself.
#[derive(Serialize, sqlx::FromRow)]
pub(super) struct KeyInfo {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) created_at: i64,
    pub(super) last_used_at: Option<i64>,
    pub(super) scope: String,
}

/// A user's API keys and what each may reach, for an admin; their sessions stay their own.
pub(super) async fn list_user_keys(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Vec<KeyInfo>>, ApiError> {
    let known: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    if known.is_none() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such user"));
    }
    Ok(Json(
        sqlx::query_as(
            "SELECT id, name, created_at, last_used_at, scope FROM tokens
             WHERE user_id = ? AND kind = 'api_key' ORDER BY id",
        )
        .bind(id)
        .fetch_all(&state.pool)
        .await?,
    ))
}

/// Revokes one of a user's API keys for an admin, the same way its owner would; never a
/// session, which stays the user's own.
pub(super) async fn revoke_user_key(
    State(state): State<AppState>,
    Path((id, key_id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    if !revoke(&state.pool, id, key_id, Revocable::KeysOnly).await? {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "that user has no such API key",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
pub(super) struct PasswordReset {
    pub(super) username: String,
    /// Shown only here.
    pub(super) password: String,
}

/// Sets a generated password, as `melete user reset-password` does, and signs the user out
/// everywhere. Admins change their own with PUT /api/v1/me/password.
pub(super) async fn reset_password(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<PasswordReset>, ApiError> {
    if id == current.user.id {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "change your own password with PUT /api/v1/me/password",
        ));
    }
    let username: Option<String> = sqlx::query_scalar("SELECT username FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    let Some(username) = username else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such user"));
    };
    let password = auth::generate_password()?;
    auth::set_password(&state.pool, id, password.clone(), None).await?;
    Ok(Json(PasswordReset { username, password }))
}
