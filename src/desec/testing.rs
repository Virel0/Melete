//! A stand-in for deSEC: its API (the zone a name is in, a TXT record read and written, a token
//! checked, writes throttled when asked) and a nameserver answering the records it holds, both
//! on localhost.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The token the stand-in takes.
pub const TOKEN: &str = "desec-stand-in-token-0123456789";

/// What the stand-in holds, and was asked.
#[derive(Debug, Default)]
pub struct Fake {
    /// The account's zones.
    pub zones: Vec<String>,
    /// TXT values by zone and subname, unquoted.
    pub records: HashMap<(String, String), Vec<String>>,
    /// Each write: the subname and the values it left, and when it came.
    pub writes: Vec<(String, Vec<String>)>,
    pub written_at: Vec<std::time::Instant>,
    /// How many requests from now are turned away as too many.
    pub throttle: u32,
    /// Writes turned down, as a token whose policy doesn't allow them.
    pub read_only: bool,
    /// Whether the nameserver answers what's written; when not, it answers as if nothing is.
    pub serving: bool,
    /// Bytes of padding added to the answer naming a zone; writes taken but not kept.
    pub padding: usize,
    pub lose_writes: bool,
    /// Every Authorization header seen, and every request's path and query.
    pub headers: Vec<String>,
    pub paths: Vec<String>,
}

pub type Shared = Arc<Mutex<Fake>>;

/// Starts the stand-in holding `zones`: its API's address, its nameserver's, and its state.
pub async fn start(zones: &[&str]) -> (String, SocketAddr, Shared) {
    let fake = Shared::new(Mutex::new(Fake {
        zones: zones.iter().map(|z| (*z).to_owned()).collect(),
        serving: true,
        ..Fake::default()
    }));
    let router = axum::Router::new()
        .fallback(answer)
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let dns = socket.local_addr().unwrap();
    tokio::spawn(nameserver(socket, fake.clone()));
    (base, dns, fake)
}

/// The zone holding `name` and the subname in it.
fn place(fake: &Fake, name: &str) -> Option<(String, String)> {
    let zone = fake
        .zones
        .iter()
        .find(|z| name == z.as_str() || name.ends_with(&format!(".{z}")))?;
    let sub = name.strip_suffix(zone.as_str())?.trim_end_matches('.');
    Some((zone.clone(), sub.to_owned()))
}

async fn answer(
    State(fake): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    body: String,
) -> Response {
    let mut fake = fake.lock().unwrap();
    let said = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    fake.headers.push(said.clone());
    fake.paths.push(uri.to_string());
    if fake.throttle > 0 {
        fake.throttle -= 1;
        return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1")]).into_response();
    }
    if said != format!("Token {TOKEN}") {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"detail": "Invalid token."})),
        )
            .into_response();
    }
    let parts: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        (Method::GET, ["domains"]) => {
            let name = query.get("owns_qname").cloned().unwrap_or_default();
            let padding = "x".repeat(fake.padding);
            let found: Vec<Value> = place(&fake, &name)
                .map(|(zone, _)| json!({ "name": zone, "padding": padding }))
                .into_iter()
                .collect();
            axum::Json(found).into_response()
        }
        (Method::GET, ["domains", zone, "rrsets", sub, "TXT"]) => {
            match fake.records.get(&((*zone).to_owned(), (*sub).to_owned())) {
                Some(values) if !values.is_empty() => {
                    let quoted: Vec<String> = values.iter().map(|v| format!("\"{v}\"")).collect();
                    axum::Json(json!({ "records": quoted, "ttl": 3600 })).into_response()
                }
                _ => StatusCode::NOT_FOUND.into_response(),
            }
        }
        (Method::PATCH, ["domains", zone, "rrsets"]) => {
            if fake.read_only {
                return (
                    StatusCode::FORBIDDEN,
                    axum::Json(json!({"detail": "Insufficient token permissions."})),
                )
                    .into_response();
            }
            let sets: Vec<Value> = serde_json::from_str(&body).unwrap();
            for set in sets {
                let sub = set["subname"].as_str().unwrap().to_owned();
                assert_eq!(set["type"], "TXT");
                assert!(set["ttl"].as_u64().unwrap() >= 3600, "deSEC's lowest TTL");
                let values: Vec<String> = set["records"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let r = r.as_str().unwrap();
                        assert!(r.starts_with('"') && r.ends_with('"'), "quoted: {r}");
                        r[1..r.len() - 1].to_owned()
                    })
                    .collect();
                fake.writes.push((sub.clone(), values.clone()));
                fake.written_at.push(std::time::Instant::now());
                if fake.lose_writes {
                    continue;
                }
                fake.records.insert(((*zone).to_owned(), sub), values);
            }
            axum::Json(json!([])).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The name a query asks about.
fn question(packet: &[u8]) -> Option<String> {
    let mut at = 12;
    let mut labels = Vec::new();
    loop {
        let len = usize::from(*packet.get(at)?);
        if len == 0 {
            return Some(labels.join("."));
        }
        labels.push(String::from_utf8_lossy(packet.get(at + 1..at + 1 + len)?).into_owned());
        at += 1 + len;
    }
}

/// An answer to `query` holding `values`, as a nameserver sends it.
pub fn dns_answer(query: &[u8], values: &[&str]) -> Vec<u8> {
    let mut out = query.to_vec();
    out[2] = 0x84;
    out[7] = u8::try_from(values.len()).unwrap();
    for value in values {
        out.extend([0xc0, 12, 0, 16, 0, 1, 0, 0, 14, 16]);
        let data: Vec<u8> = std::iter::once(u8::try_from(value.len()).unwrap())
            .chain(value.bytes())
            .collect();
        out.extend(u16::try_from(data.len()).unwrap().to_be_bytes());
        out.extend(data);
    }
    out
}

async fn nameserver(socket: tokio::net::UdpSocket, fake: Shared) {
    let mut buffer = [0u8; 512];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buffer).await else {
            return;
        };
        let asked = &buffer[..n];
        let values: Vec<String> = {
            let fake = fake.lock().unwrap();
            question(asked)
                .filter(|_| fake.serving)
                .and_then(|name| place(&fake, &name))
                .and_then(|key| fake.records.get(&key).cloned())
                .unwrap_or_default()
        };
        let values: Vec<&str> = values.iter().map(String::as_str).collect();
        let _ = socket.send_to(&dns_answer(asked, &values), from).await;
    }
}
