//! A user's own account: signing out, who they are, their password, sessions and API keys.

use super::*;

/// Also clears the web UI's cookie, which is harmless for apps.
pub(super) async fn logout(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    sqlx::query("DELETE FROM tokens WHERE id = ?")
        .bind(current.token_id)
        .execute(&state.pool)
        .await?;
    let cookie = cookie(None, &headers);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

/// For the web UI, which asks this on every load, the cookie is set again: it runs out 90 days
/// after it was last set, and the session only after 90 days unused. `sees_all_music` says
/// whether the user may see every music library, which Aoide's sync, radio, likes and playlists
/// need, so an app can leave out what would only be refused.
pub(super) async fn me(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let sees_all_music = current.access.sees_all_music(&state.pool).await?;
    let user = &current.user;
    let body = Json(json!({
        "id": user.id,
        "username": user.username,
        "is_admin": user.is_admin,
        "sees_all_music": sees_all_music,
    }));
    let signed_by_cookie = !headers.contains_key(header::AUTHORIZATION);
    Ok(
        match session_cookie(&headers).filter(|_| signed_by_cookie) {
            Some(token) => {
                let cookie = cookie(Some(token), &headers);
                ([(header::SET_COOKIE, cookie)], body).into_response()
            }
            None => body.into_response(),
        },
    )
}

#[derive(Deserialize)]
pub(super) struct PasswordChange {
    pub(super) current_password: String,
    pub(super) new_password: String,
}

/// Changes the caller's password and signs out their other sessions and API keys.
pub(super) async fn change_password(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<PasswordChange>,
) -> Result<StatusCode, ApiError> {
    auth::validate_password(&body.new_password).map_err(bad_request)?;
    // Its own key, so failed sign-ins by someone else can't block a password change.
    let key = format!("password|{}", current.user.id);
    if !state.limiter.begin(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong passwords; try again in 15 minutes",
        ));
    }
    if auth::check_password(&state.pool, &current.user.username, body.current_password)
        .await?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "current password is wrong",
        ));
    }
    state.limiter.succeeded(&key);
    auth::set_password(
        &state.pool,
        current.user.id,
        body.new_password,
        Some(current.token_id),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
pub(super) struct TokenRow {
    pub(super) id: i64,
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) created_at: i64,
    pub(super) last_used_at: Option<i64>,
    pub(super) scope: String,
}

/// The caller's sessions and API keys (never the tokens themselves).
pub(super) async fn list_tokens(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<TokenRow> = sqlx::query_as(
        "SELECT id, kind, name, created_at, last_used_at, scope FROM tokens WHERE user_id = ?
         ORDER BY id",
    )
    .bind(current.user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "kind": t.kind,
                    "name": t.name,
                    "created_at": t.created_at,
                    "last_used_at": t.last_used_at,
                    "scope": t.scope,
                    "current": t.id == current.token_id,
                })
            })
            .collect(),
    ))
}

pub(super) async fn revoke_token(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if !revoke(&state.pool, current.user.id, id, Revocable::Any).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such token"));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Which of a user's tokens [`revoke`] may reach.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Revocable {
    /// Their sessions and API keys: the owner revoking their own.
    Any,
    /// Only their API keys: an admin's reach, which stops short of their sessions.
    KeysOnly,
}

/// Deletes `user_id`'s token `id`, which stops working on its next request (every request looks
/// its token up afresh). False when they have no such token within `reach`.
pub(super) async fn revoke(
    pool: &SqlitePool,
    user_id: i64,
    id: i64,
    reach: Revocable,
) -> sqlx::Result<bool> {
    let deleted =
        sqlx::query("DELETE FROM tokens WHERE id = ? AND user_id = ? AND (? OR kind = 'api_key')")
            .bind(id)
            .bind(user_id)
            .bind(reach == Revocable::Any)
            .execute(pool)
            .await?
            .rows_affected();
    Ok(deleted > 0)
}

#[derive(Deserialize)]
pub(super) struct NewKey {
    pub(super) name: String,
    /// What it may reach; full, as keys have always been, when not given.
    pub(super) scope: Option<String>,
}

/// Creates an API key. The key is in this response only; it cannot be shown again.
pub(super) async fn create_key(
    State(state): State<AppState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<NewKey>,
) -> Result<Response, ApiError> {
    let name = auth::validate_label(&body.name).map_err(bad_request)?;
    let scope = match body.scope.as_deref() {
        None => Scope::Full,
        Some(named) => Scope::parse(named)
            .ok_or_else(|| bad_request("scope is read, playback, music-sync or full"))?,
    };
    let (id, token) =
        auth::issue_scoped(&state.pool, current.user.id, TokenKind::ApiKey, name, scope).await?;
    let body = Json(json!({ "id": id, "name": name, "token": token, "scope": scope }));
    Ok((StatusCode::CREATED, body).into_response())
}
