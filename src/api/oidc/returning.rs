//! Where the provider sends the browser back, and finishing the sign-in there.

use super::*;

#[derive(Deserialize)]
pub(super) struct Callback {
    pub(super) state: Option<String>,
    pub(super) code: Option<String>,
    pub(super) error: Option<String>,
}

/// Where the browser goes when a sign-in ends, with a word on how: back to the app, or to the
/// web UI's page.
pub(super) fn back(purpose: &Purpose, outcome: &str, extra: Option<(&str, &str)>) -> String {
    match purpose {
        Purpose::App { return_to, .. } => {
            // Every App purpose's return_to passed `return_to_allowed`, which parses it.
            let mut url = reqwest::Url::parse(return_to).expect("parsed when the sign-in began");
            {
                let mut pairs = url.query_pairs_mut();
                match extra {
                    Some((k, v)) => pairs.append_pair(k, v),
                    None => pairs.append_pair("error", outcome),
                };
            }
            url.into()
        }
        Purpose::Web { .. } => format!("/#/?oidc={outcome}"),
        Purpose::Link { .. } => format!("/#/settings/account?oidc={outcome}"),
    }
}

pub(super) fn redirect(to: &str, cookies: &[String]) -> Response {
    let mut response = Redirect::to(to).into_response();
    for cookie in cookies {
        if let Ok(value) = cookie.parse() {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

/// The provider sends the browser back here. The web UI is sent back with its session, an app's
/// sign-in shows a page asking whether to sign it in (`confirm`), and anything that went wrong
/// is a redirect saying so.
pub(super) async fn callback(
    State(state): State<OidcState>,
    headers: HeaderMap,
    Query(query): Query<Callback>,
) -> Result<Response, ApiError> {
    let Some(client) = &state.client else {
        return Ok(redirect("/#/?oidc=off", &[]));
    };
    let https = client.https();
    let clear = binding_cookie(None, https);
    let Some(attempt) = query.state.as_deref().and_then(|s| client.take(s)) else {
        return Ok(redirect("/#/?oidc=expired", &[clear]));
    };
    let fail = |outcome: &str| {
        redirect(
            &back(&attempt.purpose, outcome, None),
            std::slice::from_ref(&clear),
        )
    };
    if query.error.is_some() {
        return Ok(fail("denied"));
    }
    let Some(code) = query.code.as_deref().filter(|c| !c.is_empty()) else {
        return Ok(fail("failed"));
    };
    if let Purpose::Web { binding: bound } | Purpose::Link { binding: bound, .. } = &attempt.purpose
        && binding(&headers).map(token_hash).as_ref() != Some(bound)
    {
        return Ok(fail("browser"));
    }
    let identity = match client.finish(&attempt, code).await {
        Ok(identity) => identity,
        Err(err) => {
            tracing::warn!(error = format!("{err:#}"), "an OIDC sign-in failed");
            return Ok(fail("failed"));
        }
    };
    finish(&state.pool, client, attempt, identity, clear).await
}

pub(super) async fn finish(
    pool: &SqlitePool,
    client: &Client,
    attempt: Attempt,
    identity: Identity,
    clear: String,
) -> Result<Response, ApiError> {
    let fail = |outcome: &str| {
        redirect(
            &back(&attempt.purpose, outcome, None),
            std::slice::from_ref(&clear),
        )
    };
    match &attempt.purpose {
        Purpose::Link { user_id, .. } => {
            Ok(match link_identity(pool, *user_id, &identity).await? {
                Linked::Done => fail("linked"),
                Linked::Taken => fail("taken"),
                Linked::NoUser => fail("failed"),
            })
        }
        Purpose::Web { .. } => {
            let Some(user) = linked_user(pool, &identity).await? else {
                return Ok(fail("unlinked"));
            };
            let (_, token) =
                auth::issue_token(pool, user.id, TokenKind::Session, &attempt.device_name).await?;
            Ok(redirect(
                "/#/",
                &[clear.clone(), cookie_for(Some(&token), client.https())],
            ))
        }
        Purpose::App { secret, return_to } => {
            let Some(user) = linked_user(pool, &identity).await? else {
                return Ok(fail("unlinked"));
            };
            // The person in this browser says it's theirs before the app gets its code.
            let confirm = Confirm {
                secret: secret.clone(),
                user_id: user.id,
                username: user.username.clone(),
                device_name: attempt.device_name.clone(),
                return_to: return_to.clone(),
                expires: std::time::Instant::now(),
            };
            let (cookie, form) = client.to_confirm(confirm.clone())?;
            Ok(confirmation(
                confirm_page(&confirm, &form),
                &[clear.clone(), confirm_cookie(Some(&cookie), client.https())],
            ))
        }
    }
}
