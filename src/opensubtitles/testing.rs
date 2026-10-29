//! A local stand-in for OpenSubtitles' API, so tests never reach the real one.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::Value;

use super::*;

/// What a request to the stand-in carried.
#[derive(Debug, Clone)]
pub struct Seen {
    /// The path asked for.
    pub path: String,
    /// The query string, as sent.
    pub query: String,
    /// Every header, lower-cased names.
    pub headers: Vec<(String, String)>,
    /// The body, as text.
    pub body: String,
}

/// The stand-in's state: what it was asked, and whether downloads are used up.
#[derive(Default)]
pub struct Fake {
    /// Every request, in order.
    pub seen: Mutex<Vec<Seen>>,
    /// Downloads answer 406, as when the day's are used up.
    pub quota_reached: std::sync::atomic::AtomicBool,
    /// Searches find nothing.
    pub nothing: std::sync::atomic::AtomicBool,
    base: Mutex<String>,
}

/// The key the stand-in takes, and the login.
pub const KEY: &str = "good-key";
pub const USER: &str = "alice";
pub const PASSWORD: &str = "alice-password";

fn note(fake: &Fake, path: &str, query: Option<String>, headers: &HeaderMap, body: &str) {
    let headers = headers
        .iter()
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    fake.seen.lock().unwrap().push(Seen {
        path: path.into(),
        query: query.unwrap_or_default(),
        headers,
        body: body.into(),
    });
}

fn keyed(headers: &HeaderMap) -> bool {
    headers.get("api-key").is_some_and(|k| k == KEY)
}

/// Starts the stand-in on a free local port; its address, for [`Client::with_api`].
pub async fn start() -> (String, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .route(
            "/infos/formats",
            get(|State(f): State<Arc<Fake>>, h: HeaderMap| async move {
                note(&f, "/infos/formats", None, &h, "");
                if keyed(&h) {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                }
            }),
        )
        .route(
            "/login",
            post(
                |State(f): State<Arc<Fake>>, h: HeaderMap, body: String| async move {
                    note(&f, "/login", None, &h, &body);
                    let v: Value = serde_json::from_str(&body).unwrap_or_default();
                    if keyed(&h) && v["username"] == USER && v["password"] == PASSWORD {
                        axum::Json(json!({ "token": "login-token", "status": 200 })).into_response()
                    } else {
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                },
            ),
        )
        .route("/subtitles", get(search))
        .route("/download", post(download))
        .route(
            "/files/{name}",
            get(|UrlPath(name): UrlPath<String>| async move {
                format!("1\n00:00:01,000 --> 00:00:04,000\nFetched {name}\n")
            }),
        )
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *fake.base.lock().unwrap() = base.clone();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, fake)
}

async fn search(State(f): State<Arc<Fake>>, RawQuery(query): RawQuery, h: HeaderMap) -> Response {
    note(&f, "/subtitles", query, &h, "");
    if !keyed(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if f.nothing.load(std::sync::atomic::Ordering::Relaxed) {
        return axum::Json(json!({ "total_count": 0, "data": [] })).into_response();
    }
    let item = |file_id: i64, downloads: i64, matched: bool, release: &str| {
        json!({ "id": "1", "type": "subtitle", "attributes": {
            "language": "fr", "download_count": downloads, "hearing_impaired": false,
            "moviehash_match": matched, "release": release,
            "files": [{ "file_id": file_id, "cd_number": 1, "file_name": "x.srt" }] } })
    };
    axum::Json(json!({ "total_count": 3, "data": [
        item(101, 5000, false, "Popular.WEB"),
        item(102, 10, true, "This.Very.File"),
        { "id": "3", "attributes": { "language": "fr", "files": [] } },
    ] }))
    .into_response()
}

async fn download(State(f): State<Arc<Fake>>, h: HeaderMap, body: String) -> Response {
    note(&f, "/download", None, &h, &body);
    if !keyed(&h) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if f.quota_reached.load(std::sync::atomic::Ordering::Relaxed) {
        let used_up = json!({ "remaining": 0, "message": "used up",
            "reset_time_utc": "2030-01-02T03:04:05.000Z" });
        return (StatusCode::NOT_ACCEPTABLE, axum::Json(used_up)).into_response();
    }
    let v: Value = serde_json::from_str(&body).unwrap_or_default();
    let base = f.base.lock().unwrap().clone();
    let link = format!("{base}/files/{}.srt", v["file_id"]);
    axum::Json(json!({ "link": link, "remaining": 4 })).into_response()
}

/// Settings for the stand-in: its key, with the login or without.
pub fn settings(login: bool) -> OpenSubtitles {
    OpenSubtitles {
        key: Secret::for_tests(KEY),
        login: login.then(|| (USER.to_owned(), Secret::for_tests(PASSWORD))),
        imports: Vec::new(),
    }
}
