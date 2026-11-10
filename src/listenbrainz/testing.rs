//! A fake ListenBrainz, Labs and the API in one, for tests: it answers what it's given, and logs
//! every request as its path and its query's pairs, so a test can check what left the server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// A recording the fake first answers 429 for, asking for a one-second pause, then as one it
/// knows nothing alongside.
pub const PAUSED: &str = "00000000-0000-4000-8000-000000000429";
/// A recording the fake always answers 429 for, asking for an hour's pause.
pub const HELD: &str = "00000000-0000-4000-8000-000000003600";
/// A recording whose answer says no requests are left for one second.
pub const SPENT: &str = "00000000-0000-4000-8000-000000000000";
/// An artist whose top recordings the fake refuses without a token, as ListenBrainz has at
/// times.
pub const REFUSED: &str = "00000000-0000-4000-8000-000000000401";

/// What the fake answers: similar recordings and artists by the id asked about, and each
/// recording's metadata.
#[derive(Debug, Clone, Default)]
pub struct Answers {
    /// Recording id to the recordings alongside it, as Labs answers them.
    pub recordings: HashMap<String, Vec<Value>>,
    /// Artist id to the artists alongside them.
    pub artists: HashMap<String, Vec<Value>>,
    /// Artist id to their top recordings, as the API answers them.
    pub top: HashMap<String, Vec<Value>>,
    /// Recording id to its metadata, as the API answers it.
    pub metadata: HashMap<String, Value>,
}

/// What the fake was asked, in order: each request's path, then its query's pairs as
/// `name=value`, and whether it carried anything to identify a caller (`cookie`,
/// `authorization`).
pub type Log = Arc<Mutex<Vec<String>>>;

type Fake = (Arc<Answers>, Log);

/// The fake, answering `answers`, with a client for it whose turns are a millisecond apart.
pub async fn start(answers: Answers) -> (super::Client, Log) {
    let log: Log = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .route("/similar-recordings/json", axum::routing::get(recordings))
        .route("/similar-artists/json", axum::routing::get(artists))
        .route(
            "/1/metadata/recording/",
            axum::routing::get(recording_metadata),
        )
        .route(
            "/1/popularity/top-recordings-for-artist/{artist}",
            axum::routing::get(top_recordings),
        )
        .with_state((Arc::new(answers), log.clone()));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = super::Client::with_bases(base.clone(), base, true)
        .unwrap()
        .spaced(std::time::Duration::from_millis(1));
    (client, log)
}

/// A similar recording as Labs gives it, without its artists' ids, as Labs leaves them out.
pub fn recording(id: &str, title: &str, artist: &str, release: &str, score: i64) -> Value {
    json!({
        "recording_mbid": id, "recording_name": title, "artist_credit_name": artist,
        "artist_credit_mbids": null, "release_name": release,
        "release_mbid": "11111111-1111-4111-8111-111111111111", "caa_id": 1,
        "caa_release_mbid": "11111111-1111-4111-8111-111111111111", "score": score,
        "reference_mbid": "22222222-2222-4222-8222-222222222222",
    })
}

/// One of an artist's top recordings as the API gives it.
pub fn top(id: &str, title: &str, artist: &str, listens: i64) -> Value {
    json!({
        "artist_mbids": [], "artist_name": artist, "recording_mbid": id, "recording_name": title,
        "release_name": "Their Album", "release_mbid": "33333333-3333-4333-8333-333333333333",
        "caa_id": 1, "caa_release_mbid": "33333333-3333-4333-8333-333333333333", "length": 1000,
        "total_listen_count": listens, "total_user_count": 1, "tags": [],
    })
}

/// A similar artist as Labs gives them.
pub fn artist(id: &str, name: &str, score: i64) -> Value {
    json!({
        "artist_mbid": id, "name": name, "comment": "", "type": "Group", "gender": null,
        "score": score, "reference_mbid": "22222222-2222-4222-8222-222222222222",
    })
}

/// A recording's metadata as the API gives it: its artists by id and name, its release's name
/// and release group.
pub fn metadata(artists: &[(&str, &str)], release: &str, group: &str) -> Value {
    let credits: Vec<Value> = artists
        .iter()
        .map(|(id, name)| json!({"artist_mbid": id, "name": name, "join_phrase": "", "rels": {}}))
        .collect();
    json!({
        "artist": {"artist_credit_id": 1, "artists": credits, "name": artists.first().map(|a| a.1)},
        "recording": {"name": "x", "length": 1000, "rels": []},
        "release": {"name": release, "mbid": "11111111-1111-4111-8111-111111111111",
            "release_group_mbid": group, "caa_id": 1, "year": 2000},
    })
}

/// Notes the request: its path, its query's pairs in order, and any header that could say who
/// asked.
fn note(log: &Log, path: &str, query: &[(String, String)], headers: &HeaderMap) {
    let mut line = path.to_owned();
    for (name, value) in query {
        line.push_str(&format!(" {name}={value}"));
    }
    for name in [header::COOKIE, header::AUTHORIZATION] {
        if headers.contains_key(&name) {
            line.push_str(&format!(" [{name}]"));
        }
    }
    log.lock().unwrap().push(line);
}

/// How many times `line` was asked, this one included.
fn asked(log: &Log, line: &str) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|l| l.as_str() == line)
        .count()
}

fn pause(seconds: &'static str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, seconds)],
    )
        .into_response()
}

async fn recordings(
    State((answers, log)): State<Fake>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    note(&log, "similar-recordings", &query, &headers);
    let id = value(&query, "recording_mbids");
    let line = log.lock().unwrap().last().cloned().unwrap_or_default();
    match id.as_str() {
        PAUSED if asked(&log, &line) == 1 => pause("1"),
        HELD => pause("3600"),
        SPENT => (
            [
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset-in", "1"),
            ],
            "[]",
        )
            .into_response(),
        _ => json!(answers.recordings.get(&id).cloned().unwrap_or_default())
            .to_string()
            .into_response(),
    }
}

async fn artists(
    State((answers, log)): State<Fake>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    note(&log, "similar-artists", &query, &headers);
    let id = value(&query, "artist_mbids");
    json!(answers.artists.get(&id).cloned().unwrap_or_default())
        .to_string()
        .into_response()
}

async fn recording_metadata(
    State((answers, log)): State<Fake>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    note(&log, "metadata", &query, &headers);
    let found: serde_json::Map<String, Value> = value(&query, "recording_mbids")
        .split(',')
        .filter_map(|id| Some((id.to_owned(), answers.metadata.get(id)?.clone())))
        .collect();
    (
        [
            ("x-ratelimit-remaining", "29"),
            ("x-ratelimit-reset-in", "5"),
        ],
        Value::Object(found).to_string(),
    )
        .into_response()
}

async fn top_recordings(
    State((answers, log)): State<Fake>,
    Path(artist): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
) -> Response {
    note(&log, &format!("top-recordings {artist}"), &query, &headers);
    if artist == REFUSED {
        return (
            StatusCode::UNAUTHORIZED,
            r#"{"code":401,"error":"you need to provide an Auth token for this endpoint"}"#,
        )
            .into_response();
    }
    json!(answers.top.get(&artist).cloned().unwrap_or_default())
        .to_string()
        .into_response()
}

fn value(query: &[(String, String)], name: &str) -> String {
    query
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}
