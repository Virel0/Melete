//! Telling admins what needs them, by webhook or email as set up under Outside services: for
//! now, a library that stops responding and when it answers again. Each is off until set, and
//! only the alert itself leaves the server: the library's name and what happened.
//!
//! Settings are read when an alert is sent, so what's saved in the web UI applies at once.

use std::time::Duration;

use sqlx::SqlitePool;

use crate::config::Alerts;
use crate::library::health::Change;

mod email;
mod webhook;

/// How long sending one alert may take.
const LIMIT: Duration = Duration::from_secs(20);

/// One alert: a line to show on its own, and a few sentences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub title: String,
    pub message: String,
}

impl Alert {
    /// What's said when a library stops or starts answering.
    pub fn library(change: &Change, now: i64) -> Alert {
        match change {
            Change::Stopped(down) => Alert {
                title: format!("The {} library isn't responding", down.name),
                message: format!(
                    "Melete can't read the {} library's folder: {}. Scans leave it alone, copies into it wait, and players say it isn't responding, until it answers again.",
                    down.name, down.why
                ),
            },
            Change::Back { name, since } => Alert {
                title: format!("The {name} library answers again"),
                message: format!(
                    "The {name} library is answering again after {} min. Copies into it go ahead, and it plays again.",
                    (now - since).max(0) / 60
                ),
            },
        }
    }
}

/// Sends `alert` each way set up; answers how each went, by way ("webhook", "email").
pub async fn send_with(alerts: &Alerts, alert: &Alert) -> Vec<(&'static str, Result<(), String>)> {
    let mut sent = Vec::new();
    if let Some(hook) = &alerts.webhook {
        sent.push(("webhook", within(webhook::send(hook, alert)).await));
    }
    if let Some(mail) = &alerts.email {
        sent.push(("email", within(email::send(mail, alert)).await));
    }
    sent
}

async fn within(sending: impl Future<Output = Result<(), String>>) -> Result<(), String> {
    tokio::time::timeout(LIMIT, sending)
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {} seconds", LIMIT.as_secs())))
}

/// Sends `alert` each way set up as the settings stand now, noting in the log any way it
/// couldn't go.
pub async fn send(pool: &SqlitePool, alert: &Alert) {
    let alerts = match crate::settings::load(pool).await {
        Ok(config) => config.alerts,
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't read how to send alerts"
            );
            return;
        }
    };
    for (way, sent) in send_with(&alerts, alert).await {
        match sent {
            Ok(()) => tracing::info!(way, title = alert.title, "alert sent"),
            Err(why) => tracing::warn!(way, why, title = alert.title, "couldn't send an alert"),
        }
    }
}

/// A Test of the webhook or email alerts: sends a test alert that way, saying how it went.
pub async fn test(way: &str, alerts: &Alerts) -> (bool, String) {
    let alert = Alert {
        title: "Melete test alert".into(),
        message: "This is a test. Melete sends alerts this way when a library stops responding, and again when it answers.".into(),
    };
    let only = match way {
        "webhook" => Alerts {
            webhook: alerts.webhook.clone(),
            email: None,
        },
        _ => Alerts {
            webhook: None,
            email: alerts.email.clone(),
        },
    };
    match send_with(&only, &alert).await.pop() {
        None if way == "webhook" => (false, "Set the webhook's address first.".into()),
        None => (false, "Set the mail server and the addresses first.".into()),
        Some((_, Ok(()))) if way == "webhook" => (true, "Sent a test alert to the webhook.".into()),
        Some((_, Ok(()))) => (true, "Sent a test email.".into()),
        Some((_, Err(why))) => (false, format!("Couldn't send it: {why}")),
    }
}

#[cfg(test)]
mod tests;
