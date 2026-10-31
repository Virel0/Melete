//! Deluge on the seedbox, through its web UI's JSON API: the only code that talks to Deluge.
//! It logs in with the web UI's password, keeps the session cookie, logs in again when the
//! session expires, and connects the web UI to its daemon when it isn't.
//!
//! The password goes only in the login request's body; redirects aren't followed and errors
//! never quote it.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, COOKIE, SET_COOKIE};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::config::Secret;

/// The largest reply read: the status of many torrents with their file lists.
const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Why a call to Deluge failed. None of them quotes the password.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The web UI refused the password.
    Rejected,
    /// The request didn't get an answer: a connection error or timeout, with its URL left out.
    Unreachable(String),
    /// The web UI answered with this HTTP status instead of 200.
    Status(u16),
    /// The web UI has no daemon to connect to.
    NoDaemon,
    /// Deluge reported an error.
    Deluge {
        /// Deluge's error code: 1 is not signed in, 2 an unknown method.
        code: i64,
        /// Deluge's message, often a whole Twisted traceback.
        message: String,
    },
    /// The reply wasn't the JSON expected, or was over 32 MiB.
    Malformed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Rejected => f.write_str("Deluge refused the password"),
            Error::Unreachable(reason) => write!(f, "can't reach Deluge: {reason}"),
            Error::Status(status) => write!(f, "Deluge answered {status}"),
            Error::NoDaemon => f.write_str("Deluge's web UI has no daemon to connect to"),
            Error::Deluge { message, .. } => write!(f, "Deluge: {}", short(message)),
            Error::Malformed(reason) => {
                write!(f, "Deluge's reply isn't what was expected: {reason}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Deluge's errors come wrapped in a Twisted traceback; keep the useful last part.
fn short(message: &str) -> &str {
    let inner = message.rsplit_once(">: ").map_or(message, |(_, rest)| rest);
    inner.trim_end_matches(|c: char| c == ']' || c.is_whitespace())
}

/// One torrent as Deluge sees it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Status {
    /// The torrent's name.
    pub name: String,
    /// "Downloading", "Seeding", "Paused", "Checking", "Queued", "Error"...
    pub state: String,
    /// How much is downloaded, in percent (0 to 100).
    pub progress: f64,
    /// The size of the files it downloads, in bytes.
    pub total_size: i64,
    /// The folder on the seedbox it downloads into.
    pub save_path: String,
    /// Whether every file it downloads is complete.
    pub is_finished: bool,
    /// Its files; empty for a magnet link until Deluge has fetched the metadata.
    pub files: Vec<TorrentFile>,
    /// Deluge's tracker or error message; often empty.
    pub message: String,
    /// How much it has uploaded for each byte downloaded; 0 when Deluge doesn't say.
    #[serde(default)]
    pub ratio: f64,
    /// How long it has seeded, in seconds; 0 when Deluge doesn't say.
    #[serde(default)]
    pub seeding_time: i64,
}

/// A file in a torrent.
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct TorrentFile {
    /// Relative to the save path, starting with the torrent's folder when it has one.
    pub path: String,
    /// Its size in bytes.
    pub size: i64,
}

const STATUS_FIELDS: [&str; 10] = [
    "name",
    "state",
    "progress",
    "total_size",
    "save_path",
    "is_finished",
    "files",
    "message",
    "ratio",
    "seeding_time",
];

/// A client for Deluge's web UI. It holds one session and makes one call at a time; requests
/// give up after 30 seconds.
pub struct Client {
    http: reqwest::Client,
    url: String,
    password: Secret,
    /// The session cookie, once logged in.
    session: Mutex<Option<String>>,
}

impl Client {
    /// `url` is the web UI as Melete reaches it, e.g. https://seedbox.example/deluge.
    pub fn new(url: &str, password: Secret) -> anyhow::Result<Self> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| anyhow::anyhow!("MELETE_DELUGE_URL isn't a URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
            anyhow::bail!("MELETE_DELUGE_URL must be http:// or https:// with a host");
        }
        if !parsed.username().is_empty() || parsed.password().is_some() || parsed.query().is_some()
        {
            anyhow::bail!("MELETE_DELUGE_URL can't hold a user name, password or query");
        }
        // The password and session cross the internet to the seedbox: https, unless Deluge is
        // on this machine or the local network.
        if parsed.scheme() == "http" && !local(&parsed) {
            anyhow::bail!(
                "MELETE_DELUGE_URL must be https:// unless Deluge is on the local network"
            );
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            url: format!("{}/json", url.trim_end_matches('/')),
            password,
            session: Mutex::new(None),
        })
    }

    /// Signs in to the web UI and makes sure it's connected to its daemon: a connection test.
    pub async fn check(&self) -> Result<(), Error> {
        self.call("web.connected", json!([])).await.map(|_| ())
    }

    /// Calls `method`, logging in first if needed, and once more if the session expired.
    async fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        let mut session = self.session.lock().await;
        for attempt in 0..2 {
            let cookie = match session.as_ref() {
                Some(cookie) => cookie.clone(),
                None => {
                    let cookie = self.login().await?;
                    *session = Some(cookie.clone());
                    cookie
                }
            };
            match self.raw(Some(&cookie), method, params.clone()).await {
                // Code 1: not authenticated (the session expired).
                Err(Error::Deluge { code: 1, .. }) if attempt == 0 => *session = None,
                // Code 2 for a daemon method: the web UI lost its daemon (it restarted, say),
                // and doesn't reconnect by itself.
                Err(Error::Deluge { code: 2, .. })
                    if attempt == 0 && method.starts_with("core.") =>
                {
                    if !self.connect(&cookie).await? {
                        return Err(Error::Deluge {
                            code: 2,
                            message: "Unknown method".into(),
                        });
                    }
                }
                result => return result.map(|(value, _)| value),
            }
        }
        Err(Error::Rejected)
    }

    async fn login(&self) -> Result<String, Error> {
        let (ok, cookie) = self
            .raw(None, "auth.login", json!([self.password.expose()]))
            .await?;
        let cookie = match (ok, cookie) {
            (Value::Bool(true), Some(cookie)) => cookie,
            (Value::Bool(true), None) => return Err(Error::Malformed("no session cookie".into())),
            _ => return Err(Error::Rejected),
        };
        self.connect(&cookie).await?;
        Ok(cookie)
    }

    /// Connects the web UI to its daemon unless it is; returns whether it had to.
    async fn connect(&self, cookie: &str) -> Result<bool, Error> {
        let (connected, _) = self.raw(Some(cookie), "web.connected", json!([])).await?;
        if connected == Value::Bool(true) {
            return Ok(false);
        }
        let (hosts, _) = self.raw(Some(cookie), "web.get_hosts", json!([])).await?;
        let host = hosts
            .as_array()
            .and_then(|hosts| hosts.first())
            .and_then(|host| host.get(0))
            .and_then(Value::as_str)
            .ok_or(Error::NoDaemon)?
            .to_owned();
        self.raw(Some(cookie), "web.connect", json!([host])).await?;
        Ok(true)
    }

    /// One JSON-RPC request; returns the result and a new session cookie, if one was set.
    async fn raw(
        &self,
        cookie: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<(Value, Option<String>), Error> {
        let unreachable = |err: reqwest::Error| Error::Unreachable(err.without_url().to_string());
        let mut request = self
            .http
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .body(json!({ "method": method, "params": params, "id": 1 }).to_string());
        if let Some(cookie) = cookie {
            request = request.header(COOKIE, format!("_session_id={cookie}"));
        }
        let mut response = request.send().await.map_err(unreachable)?;
        if response.status() != StatusCode::OK {
            return Err(Error::Status(response.status().as_u16()));
        }
        let session = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| v.strip_prefix("_session_id="))
            .map(|v| v.split(';').next().unwrap_or_default().to_owned());
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(Error::Malformed("the reply is too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Reply {
            result: Value,
            error: Option<DelugeError>,
        }
        #[derive(Deserialize)]
        struct DelugeError {
            code: i64,
            message: String,
        }
        let reply: Reply =
            serde_json::from_slice(&body).map_err(|err| Error::Malformed(err.to_string()))?;
        match reply.error {
            Some(err) => Err(Error::Deluge {
                code: err.code,
                message: err.message,
            }),
            None => Ok((reply.result, session)),
        }
    }

    /// Adds a .torrent file; returns its hash.
    pub async fn add_torrent_file(&self, file_name: &str, torrent: &[u8]) -> Result<String, Error> {
        use base64::Engine;
        let dump = base64::engine::general_purpose::STANDARD.encode(torrent);
        let result = self
            .call("core.add_torrent_file", json!([file_name, dump, {}]))
            .await?;
        hash(result)
    }

    /// Adds a magnet link; returns its hash.
    pub async fn add_magnet(&self, uri: &str) -> Result<String, Error> {
        let result = self
            .call("core.add_torrent_magnet", json!([uri, {}]))
            .await?;
        hash(result)
    }

    /// Removes a torrent from Deluge, keeping what it downloaded on the seedbox unless
    /// `with_data`, when Deluge deletes that too (its own remove-with-data). False when
    /// Deluge doesn't have it.
    pub async fn remove(&self, hash: &str, with_data: bool) -> Result<bool, Error> {
        let result = self
            .call("core.remove_torrent", json!([hash, with_data]))
            .await;
        match result {
            Ok(removed) => Ok(removed.as_bool().unwrap_or(false)),
            // Deluge answers an unknown hash with an InvalidTorrentError.
            Err(Error::Deluge { message, .. }) if message.contains("InvalidTorrentError") => {
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    /// The status of these torrents; ones Deluge doesn't have are left out.
    pub async fn statuses(&self, hashes: &[String]) -> Result<HashMap<String, Status>, Error> {
        if hashes.is_empty() {
            return Ok(HashMap::new());
        }
        let result = self
            .call(
                "core.get_torrents_status",
                json!([{ "id": hashes }, STATUS_FIELDS]),
            )
            .await?;
        serde_json::from_value(result).map_err(|err| Error::Malformed(err.to_string()))
    }
}

/// Whether a URL's host is this machine or a private network address. Host names other than
/// localhost aren't resolved, so they don't count.
pub fn local(url: &reqwest::Url) -> bool {
    use std::net::IpAddr;
    let Some(host) = url.host_str() else {
        return false;
    };
    match host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_private())
        }
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

fn hash(result: Value) -> Result<String, Error> {
    result
        .as_str()
        .filter(|h| is_hash(h))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| Error::Malformed(format!("no torrent hash in {result}")))
}

/// When Deluge refused a torrent because it already has it: the hash Deluge knows it by.
pub fn already_added(err: &Error) -> Option<String> {
    let Error::Deluge { message, .. } = err else {
        return None;
    };
    let (_, rest) = message.split_once("Torrent already in session (")?;
    let hash = rest.split(')').next()?;
    is_hash(hash).then(|| hash.to_ascii_lowercase())
}

/// Whether `h` is a torrent's info hash: 40 hex digits, in either case.
pub fn is_hash(h: &str) -> bool {
    h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The info hash of a .torrent file: the SHA-1 of its bencoded `info` dictionary.
pub fn torrent_hash(torrent: &[u8]) -> Option<String> {
    use sha1::Digest as _;
    if torrent.first() != Some(&b'd') {
        return None;
    }
    let mut at = 1;
    while torrent.get(at)? != &b'e' {
        let (key, after_key) = bencode_string(torrent, at)?;
        let end = bencode_skip(torrent, after_key)?;
        if key == b"info" {
            let digest = sha1::Sha1::digest(&torrent[after_key..end]);
            return Some(digest.iter().map(|b| format!("{b:02x}")).collect());
        }
        at = end;
    }
    None
}

/// A bencoded string at `at`: its bytes and where it ends.
fn bencode_string(data: &[u8], at: usize) -> Option<(&[u8], usize)> {
    let colon = at + data.get(at..)?.iter().position(|b| *b == b':')?;
    let len: usize = std::str::from_utf8(&data[at..colon]).ok()?.parse().ok()?;
    let end = colon.checked_add(1)?.checked_add(len)?;
    Some((data.get(colon + 1..end)?, end))
}

/// Where the bencoded value at `at` ends. Nesting is bounded so a hostile file can't recurse
/// without end.
fn bencode_skip(data: &[u8], at: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut at = at;
    loop {
        match *data.get(at)? {
            b'i' => at += data.get(at..)?.iter().position(|b| *b == b'e')? + 1,
            b'l' | b'd' => {
                depth += 1;
                if depth > 64 {
                    return None;
                }
                at += 1;
                continue;
            }
            b'e' => {
                depth = depth.checked_sub(1)?;
                at += 1;
            }
            b'0'..=b'9' => at = bencode_string(data, at)?.1,
            _ => return None,
        }
        if depth == 0 {
            return Some(at);
        }
    }
}

/// The info hash in a magnet link (hex, or base32 as some trackers write it).
pub fn magnet_hash(uri: &str) -> Option<String> {
    let query = uri.strip_prefix("magnet:?")?;
    // Deluge takes the last one when a link has several.
    let btih = query
        .split('&')
        .rev()
        .find_map(|pair| pair.strip_prefix("xt=urn:btih:"))?
        .split(['&', '%'])
        .next()?;
    if is_hash(btih) {
        return Some(btih.to_ascii_lowercase());
    }
    // Base32, 32 characters for 20 bytes.
    if btih.len() != 32 {
        return None;
    }
    let mut bits: u64 = 0;
    let mut count = 0;
    let mut bytes = Vec::with_capacity(20);
    for c in btih.bytes() {
        let value = match c.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        bits = (bits << 5) | u64::from(value);
        count += 5;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// A fake Deluge web UI for tests, answering as Deluge 2.2 does (shapes recorded from it).
#[cfg(test)]
pub mod testing {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::response::{IntoResponse, Response};
    use serde_json::{Value, json};

    #[derive(Default)]
    pub struct Fake {
        /// Torrents by hash: their status object.
        pub torrents: HashMap<String, Value>,
        pub sessions: Vec<String>,
        /// Whether the web UI is connected to its daemon.
        pub connected: bool,
        /// Methods called, in order.
        pub calls: Vec<String>,
        /// Names the next added torrent by this hash instead of its SHA-1, as libtorrent 2
        /// does for hybrid and v2 torrents.
        pub name_next: Option<String>,
        /// The names given that way, by SHA-1, so the same torrent is recognised again.
        pub named: HashMap<String, String>,
        /// Torrents removed with their data, by hash.
        pub data_removed: Vec<String>,
    }

    pub type Shared = Arc<Mutex<Fake>>;

    /// Starts it with the password "pw"; returns its URL and state.
    pub async fn start() -> (String, Shared) {
        let shared: Shared = Arc::default();
        let router = axum::Router::new()
            .route("/deluge/json", axum::routing::post(handle))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/deluge", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, shared)
    }

    pub fn status(name: &str, state: &str, progress: f64, finished: bool) -> Value {
        json!({ "name": name, "state": state, "progress": progress, "total_size": 301000,
                "save_path": "/home/user/Completed", "is_finished": finished, "message": "OK",
                "files": [{ "index": 0, "path": format!("{name}/{name}.mkv"), "size": 300000, "offset": 0 }] })
    }

    async fn handle(
        State(shared): State<Shared>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> Response {
        let method = request["method"].as_str().unwrap_or_default().to_owned();
        let params = request["params"].clone();
        let mut fake = shared.lock().unwrap();
        fake.calls.push(method.clone());
        let reply = |result: Value| {
            Json(json!({ "result": result, "error": null, "id": 1 })).into_response()
        };
        let error = |code: i64, message: &str| {
            Json(json!({ "result": null, "error": { "code": code, "message": message }, "id": 1 }))
                .into_response()
        };
        if method == "auth.login" {
            if params[0] != "pw" {
                return reply(json!(false));
            }
            let session = format!("s{}", fake.sessions.len());
            fake.sessions.push(session.clone());
            let mut response = reply(json!(true));
            response.headers_mut().insert(
                "set-cookie",
                format!("_session_id={session}; Path=/").parse().unwrap(),
            );
            return response;
        }
        let cookie = headers
            .get("cookie")
            .and_then(|c| c.to_str().ok())
            .unwrap_or_default();
        let session = cookie.strip_prefix("_session_id=").unwrap_or_default();
        if !fake.sessions.iter().any(|s| s == session) {
            return error(1, "Not authenticated");
        }
        match method.as_str() {
            "web.connected" => reply(json!(fake.connected)),
            "web.get_hosts" => reply(json!([["c43373b0", "127.0.0.1", 58846, "localclient"]])),
            "web.connect" => {
                fake.connected = true;
                reply(json!(["core.add_torrent_file"]))
            }
            _ if !fake.connected => error(2, "Unknown method"),
            "core.add_torrent_file" | "core.add_torrent_magnet" => {
                let hash = match method.as_str() {
                    "core.add_torrent_file" => {
                        use base64::Engine;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(params[1].as_str().unwrap_or_default())
                            .unwrap_or_default();
                        super::torrent_hash(&bytes)
                    }
                    _ => super::magnet_hash(params[0].as_str().unwrap_or_default()),
                };
                let Some(hash) = hash else {
                    return error(
                        4,
                        "Failure: [Failure instance: Traceback (failure with no frames): <class 'deluge.error.AddTorrentError'>: Unable to add torrent, decoding filedump failed\n]",
                    );
                };
                let hash = match (fake.named.get(&hash).cloned(), fake.name_next.take()) {
                    (Some(known), _) => known,
                    (None, Some(name)) => {
                        fake.named.insert(hash, name.clone());
                        name
                    }
                    (None, None) => hash,
                };
                if fake.torrents.contains_key(&hash) {
                    return error(
                        4,
                        &format!(
                            "Failure: [Failure instance: Traceback (failure with no frames): <class 'deluge.error.AddTorrentError'>: Torrent already in session ({hash}).\n]"
                        ),
                    );
                }
                fake.torrents
                    .insert(hash.clone(), status("Added", "Downloading", 0.0, false));
                reply(json!(hash))
            }
            "core.remove_torrent" => {
                let hash = params[0].as_str().unwrap_or_default();
                if fake.torrents.remove(hash).is_some() {
                    if params[1] == json!(true) {
                        fake.data_removed.push(hash.to_owned());
                    }
                    reply(json!(true))
                } else {
                    error(
                        4,
                        &format!(
                            "Failure: <class 'deluge.error.InvalidTorrentError'>: torrent_id {hash} not in session."
                        ),
                    )
                }
            }
            "core.get_torrents_status" => {
                let wanted: Vec<String> =
                    serde_json::from_value(params[0]["id"].clone()).unwrap_or_default();
                let found: serde_json::Map<String, Value> = wanted
                    .into_iter()
                    .filter_map(|h| fake.torrents.get(&h).map(|s| (h, s.clone())))
                    .collect();
                reply(Value::Object(found))
            }
            _ => error(2, "Unknown method"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOVIE_HASH: &str = "a11bae70b2fdd9eb95b63e746f8594dce2650b0c";

    #[test]
    fn reads_info_hashes() {
        let torrent = include_bytes!("../tests/fixtures/deluge/movie.torrent");
        assert_eq!(torrent_hash(torrent).as_deref(), Some(MOVIE_HASH));
        assert_eq!(torrent_hash(b"<html>login</html>"), None);
        assert_eq!(torrent_hash(b"d4:info"), None, "truncated");
        let deep = format!("d4:info{}{}e", "l".repeat(100), "e".repeat(100));
        assert_eq!(torrent_hash(deep.as_bytes()), None, "nesting is bounded");
        assert_eq!(
            magnet_hash(&format!(
                "magnet:?xt=urn:btih:{}&dn=x",
                MOVIE_HASH.to_uppercase()
            ))
            .as_deref(),
            Some(MOVIE_HASH)
        );
        assert_eq!(
            magnet_hash("magnet:?dn=x&xt=urn:btih:UEN244FS7XM6XFNWHZ2G7BMU3TRGKCYM").as_deref(),
            Some(MOVIE_HASH)
        );
        assert_eq!(magnet_hash("magnet:?xt=urn:btih:short"), None);
        assert_eq!(magnet_hash("https://x"), None);
    }

    #[test]
    fn keeps_the_useful_part_of_delugess_errors() {
        let err = Error::Deluge {
            code: 4,
            message: "Failure: [Failure instance: Traceback (failure with no frames): <class 'deluge.error.AddTorrentError'>: Torrent already in session (00864F0123456789ABCDEF0123456789ABCDEF01).\n]".into(),
        };
        assert_eq!(
            err.to_string(),
            "Deluge: Torrent already in session (00864F0123456789ABCDEF0123456789ABCDEF01)."
        );
        assert_eq!(
            already_added(&err).as_deref(),
            Some("00864f0123456789abcdef0123456789abcdef01")
        );
        assert_eq!(already_added(&Error::Rejected), None);
    }

    #[tokio::test]
    async fn logs_in_connects_adds_and_reads_status() {
        let (url, fake) = testing::start().await;
        let client = Client::new(&url, Secret::for_tests("pw")).unwrap();
        let hash = client
            .add_torrent_file(
                "movie.torrent",
                include_bytes!("../tests/fixtures/deluge/movie.torrent"),
            )
            .await
            .unwrap();
        assert_eq!(hash, MOVIE_HASH);
        // Not connected to its daemon, so the client connected it.
        assert_eq!(
            fake.lock().unwrap().calls,
            [
                "auth.login",
                "web.connected",
                "web.get_hosts",
                "web.connect",
                "core.add_torrent_file"
            ]
        );
        let again = client
            .add_torrent_file(
                "movie.torrent",
                include_bytes!("../tests/fixtures/deluge/movie.torrent"),
            )
            .await
            .unwrap_err();
        assert_eq!(
            already_added(&again).as_deref(),
            Some(MOVIE_HASH),
            "{again}"
        );
        let junk = client
            .add_torrent_file("x.torrent", b"<html>")
            .await
            .unwrap_err();
        assert!(matches!(junk, Error::Deluge { code: 4, .. }));

        fake.lock().unwrap().torrents.insert(
            MOVIE_HASH.into(),
            testing::status("Some.Movie", "Seeding", 100.0, true),
        );
        let statuses = client
            .statuses(&[MOVIE_HASH.into(), "0".repeat(40)])
            .await
            .unwrap();
        assert_eq!(statuses.len(), 1, "unknown hashes are left out");
        let status = &statuses[MOVIE_HASH];
        assert!(status.is_finished);
        assert_eq!(status.files[0].path, "Some.Movie/Some.Movie.mkv");
        assert_eq!(client.statuses(&[]).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn logs_in_again_when_the_session_expires_and_reports_a_wrong_password() {
        let (url, fake) = testing::start().await;
        let client = Client::new(&url, Secret::for_tests("pw")).unwrap();
        client.statuses(&[MOVIE_HASH.into()]).await.unwrap();
        fake.lock().unwrap().sessions.clear();
        client.statuses(&[MOVIE_HASH.into()]).await.unwrap();
        assert_eq!(
            fake.lock().unwrap().sessions.len(),
            1,
            "a new session after the old one expired"
        );

        let wrong = Client::new(&url, Secret::for_tests("nope")).unwrap();
        let err = wrong.statuses(&[MOVIE_HASH.into()]).await.unwrap_err();
        assert_eq!(err, Error::Rejected);
        assert!(!err.to_string().contains("nope"));
        assert!(Client::new("http://user:pw@host/deluge", Secret::for_tests("pw")).is_err());
        let gone = Client::new("http://127.0.0.1:9/deluge", Secret::for_tests("pw")).unwrap();
        assert!(matches!(
            gone.statuses(&[MOVIE_HASH.into()]).await.unwrap_err(),
            Error::Unreachable(_)
        ));
    }

    #[test]
    fn magnets_with_several_hashes_use_the_last_as_deluge_does() {
        let uri = format!(
            "magnet:?xt=urn:btih:{}&xt=urn:btih:{MOVIE_HASH}",
            "f".repeat(40)
        );
        assert_eq!(magnet_hash(&uri).as_deref(), Some(MOVIE_HASH));
    }

    #[test]
    fn a_remote_deluge_needs_https() {
        let new = |url: &str| Client::new(url, Secret::for_tests("pw"));
        assert!(new("http://seedbox.example/deluge").is_err());
        assert!(new("https://seedbox.example/deluge").is_ok());
        for local in [
            "http://127.0.0.1:8112",
            "http://192.168.1.10:8112",
            "http://localhost:8112",
            "http://[::1]:8112",
        ] {
            assert!(new(local).is_ok(), "{local}");
        }
    }

    #[tokio::test]
    async fn reconnects_when_the_web_ui_loses_its_daemon() {
        let (url, fake) = testing::start().await;
        let client = Client::new(&url, Secret::for_tests("pw")).unwrap();
        client.statuses(&[MOVIE_HASH.into()]).await.unwrap();
        // The daemon restarted: the session is still good, the connection isn't.
        fake.lock().unwrap().connected = false;
        client.statuses(&[MOVIE_HASH.into()]).await.unwrap();
        assert!(fake.lock().unwrap().connected);
        assert_eq!(
            fake.lock().unwrap().sessions.len(),
            1,
            "no new login was needed"
        );
    }
}
