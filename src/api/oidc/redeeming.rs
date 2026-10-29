//! An app redeeming its one-time code, with the secret only it holds, for its session.

use super::*;

#[derive(Deserialize)]
pub(super) struct Redeem {
    pub(super) secret: String,
    pub(super) code: String,
}

/// An app redeems the code it was sent back with, and the secret it was given, for its
/// session; once.
pub(super) async fn redeem(
    State(state): State<OidcState>,
    Json(body): Json<Redeem>,
) -> Result<Response, ApiError> {
    let client = client(&state)?;
    let gone = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "no such sign-in, or it ran out; start again",
        )
    };
    let (user_id, device_name) = client.redeem(&body.code, &body.secret).ok_or_else(gone)?;
    let user: Option<User> =
        sqlx::query_as("SELECT id, username, is_admin FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;
    let user = user.ok_or_else(gone)?;
    let (_, token) =
        auth::issue_token(&state.pool, user.id, TokenKind::Session, &device_name).await?;
    Ok(Json(json!({ "token": token, "user": user })).into_response())
}
