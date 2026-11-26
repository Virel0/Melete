//! Telling admins what needs them: by webhook, and by email.

use super::*;

/// What both say leaves the server.
#[cfg(test)]
const WHAT: &str = "Only the library's name and what happened (it stopped responding, or answers again) leaves the server";

pub(super) const WEBHOOK: Service = Service {
    key: "webhook",
    name: "Alerts by webhook",
    about: "Telling admins when a library stops responding and when it answers again, by a POST to a webhook: an ntfy topic (with its phone app), Gotify, Discord, Home Assistant or anything that takes one. Off until its address is set. Only the library's name and what happened (it stopped responding, or answers again) leaves the server, to that address. Test sends a test message there.",
    fields: &[
        field(
            "MELETE_ALERT_WEBHOOK_URL",
            "Address",
            "e.g. https://ntfy.sh/a-long-random-topic, or a Gotify, Discord or Home Assistant webhook. Kept like a password: the address is often the only key.",
            Input::Secret,
        ),
        field(
            "MELETE_ALERT_WEBHOOK_FORMAT",
            "Format",
            "Plain text with a Title header, as ntfy takes it; or JSON with title, message and content (Gotify, Discord, Home Assistant).",
            Input::Choice {
                options: &[
                    Choice::new("text", "Plain text (ntfy)"),
                    Choice::new("json", "JSON (Gotify, Discord, Home Assistant)"),
                ],
            },
        ),
    ],
};

pub(super) const EMAIL: Service = Service {
    key: "email",
    name: "Alerts by email",
    about: "Telling admins when a library stops responding and when it answers again, by email through a mail server you choose. Off until its server is set. The connection is always encrypted (STARTTLS or TLS). Only the library's name and what happened (it stopped responding, or answers again) leaves the server, to that mail server and the addresses below. Test sends a test email.",
    fields: &[
        field(
            "MELETE_SMTP_HOST",
            "Mail server",
            "Its name alone, e.g. smtp.example.com.",
            Input::Text,
        ),
        field(
            "MELETE_SMTP_SECURITY",
            "Encryption",
            "",
            Input::Choice {
                options: &[
                    Choice::new("starttls", "STARTTLS (usually port 587)"),
                    Choice::new("tls", "TLS (usually port 465)"),
                ],
            },
        ),
        field(
            "MELETE_SMTP_PORT",
            "Port",
            "587 for STARTTLS or 465 for TLS when empty.",
            Input::Number,
        ),
        field(
            "MELETE_SMTP_USERNAME",
            "Username",
            "Only for a server that wants a login.",
            Input::Text,
        ),
        field("MELETE_SMTP_PASSWORD", "Password", "", Input::Secret),
        field(
            "MELETE_ALERT_EMAIL_FROM",
            "From",
            "The address alerts are sent from, e.g. melete@example.com.",
            Input::Text,
        ),
        field(
            "MELETE_ALERT_EMAIL_TO",
            "To",
            "The admin addresses they go to, separated by commas.",
            Input::Text,
        ),
    ],
};

#[cfg(test)]
#[test]
fn both_say_what_leaves_the_server() {
    assert!(WEBHOOK.about.contains(WHAT) && EMAIL.about.contains(WHAT));
}
