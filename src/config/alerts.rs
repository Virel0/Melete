//! Where admins are told what needs them (a library that stops responding, and when it answers
//! again): a webhook, email, or both. Each is off unless its variables are set.

use super::*;

/// How admins are told.
#[derive(Debug, Clone, Default)]
pub struct Alerts {
    /// `MELETE_ALERT_WEBHOOK_URL`: POSTed to.
    pub webhook: Option<Webhook>,
    /// `MELETE_SMTP_HOST` and the rest: sent through that mail server.
    pub email: Option<Email>,
}

/// A webhook: an ntfy topic, Gotify, Discord, Home Assistant or anything taking a POST.
#[derive(Debug, Clone)]
pub struct Webhook {
    /// Kept like a password: the address is often the only key.
    pub url: Secret,
    /// `MELETE_ALERT_WEBHOOK_FORMAT=json`: a JSON body (`title`, `message`, and `content` for
    /// Discord); otherwise the message as plain text with a `Title` header, as ntfy reads it.
    pub json: bool,
}

/// A mail server to send through.
#[derive(Debug, Clone)]
pub struct Email {
    pub host: String,
    /// `MELETE_SMTP_PORT`: 587 for STARTTLS, 465 for TLS by default.
    pub port: u16,
    /// `MELETE_SMTP_SECURITY=tls`: TLS from the start; otherwise STARTTLS, which is required.
    pub tls: bool,
    /// `MELETE_SMTP_USERNAME` and `MELETE_SMTP_PASSWORD`, for a server that wants a login.
    pub login: Option<(String, Secret)>,
    /// `MELETE_ALERT_EMAIL_FROM`.
    pub from: String,
    /// `MELETE_ALERT_EMAIL_TO`: one address or several, separated by commas or spaces.
    pub to: Vec<String>,
}

/// Every setting of the email alerts, so one set alone says what's missing.
const EMAIL: &[&str] = &[
    "MELETE_SMTP_HOST",
    "MELETE_SMTP_PORT",
    "MELETE_SMTP_SECURITY",
    "MELETE_SMTP_USERNAME",
    "MELETE_SMTP_PASSWORD",
    "MELETE_ALERT_EMAIL_FROM",
    "MELETE_ALERT_EMAIL_TO",
];

/// An address as far as can be told without sending to it: something@a.domain, nothing else.
pub(super) fn address(value: &str) -> bool {
    value.split_once('@').is_some_and(|(user, domain)| {
        !user.is_empty()
            && domain.contains('.')
            && !value
                .chars()
                .any(|c| c.is_whitespace() || "<>,;\"".contains(c))
    })
}

pub(super) fn alerts(get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<Alerts> {
    let value = |key: &str| {
        get(key)
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let webhook = match value("MELETE_ALERT_WEBHOOK_URL") {
        None => None,
        Some(url) => {
            anyhow::ensure!(
                url.starts_with("https://") || url.starts_with("http://"),
                "MELETE_ALERT_WEBHOOK_URL is a web address, starting https:// (or http:// on the local network)"
            );
            let json = match value("MELETE_ALERT_WEBHOOK_FORMAT").as_deref() {
                None | Some("text") => false,
                Some("json") => true,
                Some(other) => bail!("MELETE_ALERT_WEBHOOK_FORMAT is text or json, not {other:?}"),
            };
            Some(Webhook {
                url: Secret(url),
                json,
            })
        }
    };
    let Some(host) = value("MELETE_SMTP_HOST") else {
        if let Some(set) = EMAIL.iter().find(|k| value(k).is_some()) {
            bail!("{set} is set, so MELETE_SMTP_HOST is needed too");
        }
        return Ok(Alerts {
            webhook,
            email: None,
        });
    };
    let tls = match value("MELETE_SMTP_SECURITY").as_deref() {
        None | Some("starttls") => false,
        Some("tls") => true,
        Some(other) => bail!("MELETE_SMTP_SECURITY is starttls or tls, not {other:?}"),
    };
    let port = match value("MELETE_SMTP_PORT") {
        None => {
            if tls {
                465
            } else {
                587
            }
        }
        Some(port) => port
            .parse()
            .ok()
            .filter(|p| *p > 0)
            .context("MELETE_SMTP_PORT is a port number, such as 587")?,
    };
    let login = match (value("MELETE_SMTP_USERNAME"), value("MELETE_SMTP_PASSWORD")) {
        (Some(user), Some(password)) => Some((user, Secret(password))),
        (None, None) => None,
        _ => bail!("MELETE_SMTP_USERNAME and MELETE_SMTP_PASSWORD go together"),
    };
    let from = value("MELETE_ALERT_EMAIL_FROM")
        .filter(|f| address(f))
        .context("MELETE_ALERT_EMAIL_FROM is the address alerts are sent from, such as melete@example.com")?;
    let to: Vec<String> = value("MELETE_ALERT_EMAIL_TO")
        .unwrap_or_default()
        .split([',', ' '])
        .filter(|a| !a.is_empty())
        .map(str::to_owned)
        .collect();
    anyhow::ensure!(
        !to.is_empty() && to.iter().all(|a| address(a)),
        "MELETE_ALERT_EMAIL_TO is the address (or addresses, separated by commas) alerts go to"
    );
    Ok(Alerts {
        webhook,
        email: Some(Email {
            host,
            port,
            tls,
            login,
            from,
            to,
        }),
    })
}
