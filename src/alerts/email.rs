//! An alert by email, through the mail server set up, always encrypted (STARTTLS or TLS).

use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::Alert;
use crate::config::Email;

/// The email for `alert`, from and to the addresses set up.
pub(super) fn message(mail: &Email, alert: &Alert) -> Result<Message, String> {
    let from: Mailbox = mail
        .from
        .parse()
        .map_err(|err| format!("the From address: {err}"))?;
    let mut builder = Message::builder().from(from).subject(alert.title.clone());
    for to in &mail.to {
        let to: Mailbox = to
            .parse()
            .map_err(|err| format!("the To address {to}: {err}"))?;
        builder = builder.to(to);
    }
    builder
        .header(ContentType::TEXT_PLAIN)
        .body(alert.message.clone())
        .map_err(|err| err.to_string())
}

/// Sends `alert` by email.
pub(super) async fn send(mail: &Email, alert: &Alert) -> Result<(), String> {
    let message = message(mail, alert)?;
    let relay = if mail.tls {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&mail.host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&mail.host)
    }
    .map_err(|err| err.to_string())?;
    // The greeting names no machine.
    let mut relay = relay
        .port(mail.port)
        .timeout(Some(super::LIMIT))
        .hello_name(ClientId::Domain("localhost".into()));
    if let Some((user, password)) = &mail.login {
        relay = relay.credentials(Credentials::new(user.clone(), password.expose().to_owned()));
    }
    relay
        .build()
        .send(message)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())
}
