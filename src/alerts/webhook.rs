//! An alert as a POST to a webhook: plain text with a `Title` header (ntfy), or JSON with
//! `title`, `message` and `content` (Gotify, Home Assistant, Discord).

use super::Alert;
use crate::config::Webhook;

/// What the request is sent with, naming no one.
const USER_AGENT: &str = "Melete";

/// Posts `alert` to the webhook. An error never shows the address, which is often its key.
pub(super) async fn send(hook: &Webhook, alert: &Alert) -> Result<(), String> {
    // Direct, never through the proxy: an alert has no switch of its own there.
    let client = crate::proxy::Http::through(false, |b| {
        b.user_agent(USER_AGENT)
            .timeout(super::LIMIT)
            .redirect(reqwest::redirect::Policy::none())
    })
    .map_err(|err| format!("{err:#}"))?;
    let request = client.post(hook.url.expose());
    let request = if hook.json {
        request.json(&serde_json::json!({
            "title": alert.title,
            "message": alert.message,
            "content": format!("**{}**\n{}", alert.title, alert.message),
        }))
    } else {
        // A header can't hold a line break, nor (for ntfy) more than plain text.
        let title: String = alert.title.chars().filter(|c| !c.is_control()).collect();
        request
            .header("Title", title)
            .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(alert.message.clone())
    };
    let answer = request
        .send()
        .await
        .map_err(|err| err.without_url().to_string())?;
    if answer.status().is_success() {
        Ok(())
    } else {
        Err(format!("the webhook answered {}", answer.status()))
    }
}
