//! A stand-in for Gluetun's control server (v3.41.3's routes and answers), noting what it's
//! asked. Routes can be left open to requests without a key, as Gluetun's public fallback role
//! leaves them.

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
    /// Routes ("METHOD /path") that answer without a key; "*" for every one.
    pub open: Vec<&'static str>,
    /// Whether the tunnel is up.
    pub running: bool,
    /// Each request, as "METHOD /path" and whether it carried the right key.
    pub asked: Vec<(String, bool)>,
}

impl Default for Fake {
    fn default() -> Fake {
        Fake {
            open: Vec::new(),
            running: true,
            asked: Vec::new(),
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
    let route = format!("{method} {}", uri.path());
    let mut fake = fake.lock().unwrap();
    fake.asked.push((route.clone(), keyed));
    let open = fake.open.iter().any(|o| *o == "*" || *o == route);
    if !keyed && !open {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body: Value = serde_json::from_str(&body).unwrap_or_default();
    match route.as_str() {
        "GET /v1/vpn/status" => {
            let status = if fake.running { "running" } else { "stopped" };
            Json(json!({ "status": status })).into_response()
        }
        // Gluetun takes "running" or "stopped" and turns anything else down, changing nothing.
        "PUT /v1/vpn/status" => match body["status"].as_str() {
            Some(status @ ("running" | "stopped")) => {
                fake.running = status == "running";
                Json(json!({ "outcome": status })).into_response()
            }
            _ => (StatusCode::BAD_REQUEST, "status is not valid").into_response(),
        },
        "GET /v1/publicip/ip" => Json(json!({
            "public_ip": "203.0.113.7", "country": "Netherlands", "city": "Amsterdam"
        }))
        .into_response(),
        "GET /v1/portforward" => Json(json!({ "port": 51413, "ports": [51413] })).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
