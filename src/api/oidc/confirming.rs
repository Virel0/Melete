//! The page asking the person in the browser whether to sign an app in, and their answer.

use super::*;

pub(super) fn confirm_cookie(value: Option<&str>, https: bool) -> String {
    let (value, max_age) = match value {
        Some(v) => (v, 300),
        None => ("", 0),
    };
    format!(
        "{CONFIRM_COOKIE}={value}; Path=/api/v1/auth/oidc; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if https { "; Secure" } else { "" }
    )
}

pub(super) fn cookie_named<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(name)?.strip_prefix('='))
        .filter(|v| !v.is_empty())
}

pub(super) fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

/// The page asking the person in the browser whether to sign the app in, with its form token.
pub(super) fn confirm_page(confirm: &Confirm, form: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Sign in an app – Melete</title></head><body><main>\
         <h1>Sign in {device} as {user}?</h1>\
         <p>An app asked to sign in to Melete as you, as <strong>{device}</strong>, and to go back to \
         <strong>{returns}</strong>. Continue only if you started this on that device just now.</p>\
         <form method=\"post\" action=\"/api/v1/auth/oidc/confirm\">\
         <input type=\"hidden\" name=\"form\" value=\"{form}\">\
         <button type=\"submit\" name=\"action\" value=\"continue\">Continue</button> \
         <button type=\"submit\" name=\"action\" value=\"cancel\">Cancel</button>\
         </form></main></body></html>\n",
        device = escape(&confirm.device_name),
        user = escape(&confirm.username),
        returns = escape(&confirm.return_to),
        form = escape(form),
    )
}

/// A page of Melete's own: no scripts or anything from elsewhere, never framed or cached.
pub(super) fn confirmation(page: String, cookies: &[String]) -> Response {
    let mut response = (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
            ),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        page,
    )
        .into_response();
    for cookie in cookies {
        if let Ok(value) = cookie.parse() {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

#[derive(Deserialize)]
pub(super) struct ConfirmForm {
    pub(super) form: String,
    pub(super) action: String,
}

/// The confirmation page's answer: Continue sends the app its code, Cancel sends it `denied`.
/// Needs this browser's cookie and the page's own form token, once.
pub(super) async fn confirm(
    State(state): State<OidcState>,
    headers: HeaderMap,
    Form(body): Form<ConfirmForm>,
) -> Result<Response, ApiError> {
    let client = client(&state)?;
    let clear = confirm_cookie(None, client.https());
    let confirmed = cookie_named(&headers, CONFIRM_COOKIE)
        .and_then(|cookie| client.confirmed(cookie, &body.form));
    let Some(confirmed) = confirmed else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "this sign-in has run out or wasn't started in this browser; start again in the app",
        ));
    };
    let purpose = Purpose::App {
        secret: Vec::new(),
        return_to: confirmed.return_to.clone(),
    };
    let to = if body.action == "continue" {
        let code = client.finished(confirmed.secret, confirmed.user_id, confirmed.device_name)?;
        back(&purpose, "", Some(("code", &code)))
    } else {
        back(&purpose, "denied", None)
    };
    Ok(redirect(&to, &[clear]))
}
