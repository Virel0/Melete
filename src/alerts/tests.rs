use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;

use super::*;
use crate::config::{Email, Secret, Webhook};
use crate::library::health::Down;

type Seen = Arc<Mutex<Vec<(HeaderMap, Bytes)>>>;

/// A webhook on this machine: `/hook` takes anything, `/gone` answers 404.
async fn hook() -> (String, Seen) {
    let seen = Seen::default();
    let kept = seen.clone();
    let app = Router::new()
        .route(
            "/hook/secret-topic",
            post(move |headers: HeaderMap, body: Bytes| async move {
                kept.lock().unwrap().push((headers, body));
                StatusCode::OK
            }),
        )
        .route(
            "/gone/secret-topic",
            post(|| async { StatusCode::NOT_FOUND }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let at = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (at, seen)
}

fn only_webhook(url: String, json: bool) -> Alerts {
    Alerts {
        webhook: Some(Webhook {
            url: Secret::for_tests(&url),
            json,
        }),
        email: None,
    }
}

fn down() -> Change {
    Change::Stopped(Down {
        id: 1,
        name: "movies".into(),
        since: 1_000,
        why: "its folder didn't answer within 15 seconds".into(),
    })
}

#[test]
fn an_alert_says_which_library_and_what_happened() {
    let stopped = Alert::library(&down(), 1_000);
    assert_eq!(stopped.title, "The movies library isn't responding");
    assert!(stopped.message.contains("didn't answer within 15 seconds"));
    let back = Alert::library(
        &Change::Back {
            name: "movies".into(),
            since: 1_000,
        },
        1_000 + 125 * 60,
    );
    assert_eq!(back.title, "The movies library answers again");
    assert!(back.message.contains("after 125 min"), "{}", back.message);
}

#[tokio::test]
async fn a_webhook_gets_plain_text_with_a_title_or_json() {
    let (at, seen) = hook().await;
    let alert = Alert::library(&down(), 1_000);
    let sent = send_with(
        &only_webhook(format!("{at}/hook/secret-topic"), false),
        &alert,
    )
    .await;
    assert_eq!(sent, [("webhook", Ok(()))]);
    let sent = send_with(
        &only_webhook(format!("{at}/hook/secret-topic"), true),
        &alert,
    )
    .await;
    assert_eq!(sent, [("webhook", Ok(()))]);
    let seen = seen.lock().unwrap();
    let (text_headers, text) = &seen[0];
    assert_eq!(text_headers["title"], "The movies library isn't responding");
    assert_eq!(text_headers["user-agent"], "Melete");
    assert_eq!(&text[..], alert.message.as_bytes());
    let json: serde_json::Value = serde_json::from_slice(&seen[1].1).unwrap();
    assert_eq!(json["title"], alert.title);
    assert_eq!(json["message"], alert.message);
    assert!(json["content"].as_str().unwrap().contains(&alert.message));
}

#[tokio::test]
async fn a_failing_webhook_says_so_without_its_address() {
    let (at, _) = hook().await;
    let alert = Alert::library(&down(), 1_000);
    let refused = send_with(
        &only_webhook(format!("{at}/gone/secret-topic"), false),
        &alert,
    )
    .await;
    assert_eq!(
        refused,
        [("webhook", Err("the webhook answered 404 Not Found".into()))]
    );
    // Nothing listens at port 9: the error names no address.
    let away = send_with(
        &only_webhook("http://127.0.0.1:9/secret-topic".into(), false),
        &alert,
    )
    .await;
    let why = away[0].1.clone().unwrap_err();
    assert!(
        !why.contains("secret-topic") && !why.contains("127.0.0.1"),
        "{why}"
    );
    // A Test with nothing set says what's missing.
    assert_eq!(
        test("webhook", &Alerts::default()).await,
        (false, "Set the webhook's address first.".into())
    );
}

#[test]
fn an_email_alert_goes_from_and_to_the_addresses_set_up() {
    let mail = Email {
        host: "smtp.example.com".into(),
        port: 587,
        tls: false,
        login: None,
        from: "melete@example.com".into(),
        to: vec!["alice@example.com".into(), "bob@example.com".into()],
    };
    let alert = Alert::library(&down(), 1_000);
    let formatted = String::from_utf8(email::message(&mail, &alert).unwrap().formatted()).unwrap();
    assert!(
        formatted.contains("From: melete@example.com"),
        "{formatted}"
    );
    assert!(formatted.contains("alice@example.com") && formatted.contains("bob@example.com"));
    assert!(formatted.contains("Subject: The movies library isn't responding"));
    let bad = Email {
        to: vec!["not an address".into()],
        ..mail
    };
    assert!(email::message(&bad, &alert).is_err());
}
