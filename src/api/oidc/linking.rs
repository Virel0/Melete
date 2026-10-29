//! Linking and unlinking the caller's account, and whether it's linked.

use super::*;

/// Whether the caller's account is linked to an identity at the provider set up now.
pub(super) async fn link_status(
    State(state): State<OidcState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let linked_at: Option<i64> = match &state.client {
        None => None,
        Some(_) => {
            sqlx::query_scalar(
                "SELECT linked_at FROM oidc_identities WHERE user_id = ? AND issuer = ?",
            )
            .bind(current.user.id)
            .bind(current_issuer(&state))
            .fetch_optional(&state.pool)
            .await?
        }
    };
    Ok(Json(
        json!({ "linked": linked_at.is_some(), "linked_at": linked_at }),
    ))
}

pub(super) fn current_issuer(state: &OidcState) -> String {
    state
        .client
        .as_ref()
        .map(|c| c.issuer().to_owned())
        .unwrap_or_default()
}

#[derive(Deserialize)]
pub(super) struct LinkBody {
    /// The caller's password, entered again: a session alone can't link a way in to the account.
    pub(super) password: String,
}

/// Starts linking the caller's account to their identity at the provider, in this browser,
/// once they've entered their password again.
pub(super) async fn link(
    State(state): State<OidcState>,
    Extension(current): Extension<CurrentUser>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    client_addr: Option<Extension<ClientAddr>>,
    opens: Option<Extension<Opens>>,
    Json(body): Json<LinkBody>,
) -> Result<Response, ApiError> {
    let client = client(&state)?;
    // The key a password change uses, so guesses here and there add up.
    let key = format!("password|{}", current.user.id);
    if !state.limiter.begin(&key) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too many wrong passwords; try again in 15 minutes",
        ));
    }
    if auth::check_password(&state.pool, &current.user.username, body.password)
        .await?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "that isn't your password",
        ));
    }
    state.limiter.succeeded(&key);
    let bound = oidc::random(32)?;
    let purpose = Purpose::Link {
        user_id: current.user.id,
        binding: token_hash(&bound),
    };
    let url = begin(
        client,
        purpose,
        "",
        &address(client_addr, peer),
        from_home(opens),
    )
    .await?;
    let cookie = binding_cookie(Some(&bound), client.https());
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "authorize_url": url, "secret": null })),
    )
        .into_response())
}

/// Unlinks the caller's account from every identity provider.
pub(super) async fn unlink(
    State(state): State<OidcState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<StatusCode, ApiError> {
    let gone = sqlx::query("DELETE FROM oidc_identities WHERE user_id = ?")
        .bind(current.user.id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "your account isn't linked",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}
