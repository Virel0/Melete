//! A stand-in for Gluetun's control server (v3.41.3's routes and answers), noting what it's
//! asked; it answers without a key when `open`, as Gluetun's public fallback role does.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The key the stand-in takes.
pub const KEY: &str = "melete-control-key";

/// What the stand-in is, and was asked.
#[derive(Debug)]
pub struct Fake {
    /// Answers requests without a key.
    pub open: bool,
    /// Whether the tunnel is up.
    pub running: bool,
    /// Each request, as "METHOD /path" and whether it carried the right key.
    pub asked: Vec<(String, bool)>,
    /// Each body sent to PUT /v1/vpn/settings.
    pub settings: Vec<Value>,
    /// The countries it knows, as its provider's server list names them.
    pub known: Vec<&'static str>,
}

impl Default for Fake {
    fn default() -> Fake {
        Fake {
            open: false,
            running: true,
            asked: Vec::new(),
            settings: Vec::new(),
            known: vec!["Netherlands", "Sweden"],
        }
    }
}

pub type Shared = Arc<Mutex<Fake>>;

/// Starts the stand-in; its address and state.
pub async fn start() -> (String, Shared) {
    let fake = Shared::default();
    let router = axum::Router::new()
        .fallback(answer)
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, fake)
}

async fn answer(
    State(fake): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: String,
) -> Response {
    let keyed = headers.get("x-api-key").is_some_and(|k| k == KEY);
    let mut fake = fake.lock().unwrap();
    fake.asked.push((format!("{method} {}", uri.path()), keyed));
    if !keyed && !fake.open {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body: Value = serde_json::from_str(&body).unwrap_or_default();
    match (method.as_str(), uri.path()) {
        ("GET", "/v1/vpn/status") => {
            let status = if fake.running { "running" } else { "stopped" };
            Json(json!({ "status": status })).into_response()
        }
        ("PUT", "/v1/vpn/status") => {
            fake.running = body["status"] == "running";
            Json(json!({ "outcome": "stopped" })).into_response()
        }
        ("GET", "/v1/publicip/ip") => Json(json!({
            "public_ip": "203.0.113.7", "country": "Netherlands", "city": "Amsterdam"
        }))
        .into_response(),
        ("GET", "/v1/portforward") => {
            Json(json!({ "port": 51413, "ports": [51413] })).into_response()
        }
        ("PUT", "/v1/vpn/settings") => {
            let countries = &body["provider"]["server_selection"]["countries"];
            let known = countries
                .as_array()
                .is_some_and(|c| c.iter().all(|c| fake.known.iter().any(|k| c == k)));
            fake.settings.push(body.clone());
            if known {
                "settings updated".into_response()
            } else {
                (StatusCode::BAD_REQUEST, "country is not valid").into_response()
            }
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
