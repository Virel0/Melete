//! A fake Wikidata, Commons and upload.wikimedia.org in one, for tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The Wikidata item the fake has an image for (Radiohead's).
pub const ITEM: &str = "Q44190";
/// An item the fake has no image for.
pub const BARE_ITEM: &str = "Q2";
/// An item whose image Commons fails on.
pub const FAILING_ITEM: &str = "Q3";
/// An item whose picture is larger than 10 MiB.
pub const LARGE_ITEM: &str = "Q4";
/// An item Wikidata's answer for redirects to another host.
pub const MOVED_ITEM: &str = "Q5";
/// [`ITEM`]'s image.
pub const FILE: &str = "Radiohead in concert.jpg";
/// The picture the fake serves.
pub const JPEG: &[u8] = b"\xff\xd8\xff\xe0fake portrait";

/// What the fake was asked: `wikidata <item>`, `commons <title>` and `picture <name>`.
pub type Log = Arc<Mutex<Vec<String>>>;

/// The fake's log and its own address, which its answers point pictures at.
type Fake = (Log, String);

/// The fake: [`ITEM`]'s image is [`FILE`], whose picture is [`JPEG`], with an author and a
/// licence; [`BARE_ITEM`] and any other item have none.
pub async fn start() -> (super::Client, Log) {
    start_with(true).await
}

/// The fake, with a client that holds redirects and pictures to Wikimedia's hosts over https as
/// the real one does (`any_host` false), so it refuses the fake's own addresses.
pub async fn start_with(any_host: bool) -> (super::Client, Log) {
    let log: Log = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = axum::Router::new()
        .route("/w/api.php", axum::routing::get(api))
        .route("/thumb/{name}", axum::routing::get(picture))
        .with_state((log.clone(), base.clone()));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = super::Client::with_bases(base.clone(), base, any_host).unwrap();
    (client, log)
}

/// Where the fake behind `client` serves the picture of `file` (500px.jpg, or large.jpg).
pub fn picture_url(client: &super::Client, file: &str) -> String {
    format!("{}/thumb/{file}", client.commons)
}

async fn api(
    State((log, base)): State<Fake>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let get = |k: &str| q.get(k).cloned().unwrap_or_default();
    if get("action") == "wbgetclaims" {
        let item = get("entity");
        log.lock().unwrap().push(format!("wikidata {item}"));
        return claims(&item);
    }
    let title = get("titles");
    log.lock().unwrap().push(format!("commons {title}"));
    file(&base, &title)
}

/// An item's image statements: a deprecated one before the one to use.
fn claims(item: &str) -> Response {
    let file = match item {
        ITEM => FILE,
        FAILING_ITEM => "Failing.jpg",
        LARGE_ITEM => "Large.jpg",
        MOVED_ITEM => {
            let to = [(header::LOCATION, "http://127.0.0.1:9/w/api.php")];
            return (StatusCode::FOUND, to).into_response();
        }
        _ => return json!({"claims": {}}).to_string().into_response(),
    };
    let claim = |file: &str, rank: &str| json!({"mainsnak": {"datavalue": {"value": file, "type": "string"}}, "rank": rank});
    let claims = [claim("Old.jpg", "deprecated"), claim(file, "normal")];
    json!({"claims": {"P18": claims}})
        .to_string()
        .into_response()
}

/// A file's picture at one width, its page, author and licence.
fn file(base: &str, title: &str) -> Response {
    let picture = match title.strip_prefix("File:") {
        Some("Failing.jpg") => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Some("Large.jpg") => "large.jpg",
        Some(name) if name == FILE => "500px.jpg",
        _ => {
            return json!({"query": {"pages": [{"title": title, "missing": true}]}})
                .to_string()
                .into_response();
        }
    };
    json!({"query": {"pages": [{"title": title, "imageinfo": [{
        "thumburl": format!("{base}/thumb/{picture}"),
        "url": format!("{base}/full.jpg"),
        "descriptionurl": format!("{base}/wiki/File:Radiohead_in_concert.jpg"),
        "extmetadata": {
            "Artist": {"value": "<a href=\"//commons.wikimedia.org/wiki/User:Some\">Some &amp; One</a>"},
            "LicenseShortName": {"value": "CC BY-SA 4.0"},
            "LicenseUrl": {"value": "https://creativecommons.org/licenses/by-sa/4.0"},
        },
    }]}]}})
    .to_string()
    .into_response()
}

async fn picture(State((log, _)): State<Fake>, Path(name): Path<String>) -> Response {
    log.lock().unwrap().push(format!("picture {name}"));
    let mut bytes = JPEG.to_vec();
    if name == "large.jpg" {
        bytes.resize(super::MAX_PICTURE + 1, 0);
    }
    ([(header::CONTENT_TYPE, "image/jpeg")], bytes).into_response()
}
