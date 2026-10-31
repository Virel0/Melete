//! slskd's API, for searching Soulseek and downloading from it. Melete talks only to slskd,
//! never to peers, and never changes what slskd shares; it
//! refuses to use an slskd that shares anything. The API key goes in a header and never
//! into a log, a message or a URL; errors leave the URL out. Off unless `MELETE_SLSKD_URL` is
//! set.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Secret;

/// slskd waits this long after the last response before calling a search done.
const QUIET_SECONDS: u32 = 10;
/// The longest a search may take, however long responses keep coming.
const DEADLINE: Duration = Duration::from_secs(45);
/// How often a running search is looked at.
const POLL: Duration = Duration::from_millis(1500);
/// Users' answers kept per search, as slskd counts them.
const RESPONSE_LIMIT: u32 = 200;
/// Search answers are large but bounded; anything bigger is refused.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// A client for slskd's API, holding its key; it never connects to peers itself.
pub struct Client {
    http: reqwest::Client,
    base: String,
    key: Secret,
    poll: Duration,
    deadline: Duration,
}

/// One user's answer.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// The Soulseek user who answered.
    pub username: String,
    /// Files the user lets anyone download; locked ones are left out.
    #[serde(default)]
    pub files: Vec<File>,
    /// Whether the user has an upload slot free now.
    #[serde(default)]
    pub has_free_upload_slot: bool,
    /// How many downloads the user has queued.
    #[serde(default)]
    pub queue_length: i64,
    /// Bytes a second.
    #[serde(default)]
    pub upload_speed: i64,
}

/// A file in a user's answer.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct File {
    /// The path on the user's machine, with its separators ("\" or "/").
    pub filename: String,
    /// Its size in bytes.
    pub size: i64,
    /// Its bit rate in kbit/s, when the user's client reported one.
    pub bit_rate: Option<i64>,
    /// Its bit depth (16, 24), for lossless files that report one.
    pub bit_depth: Option<i64>,
    /// Its sample rate in Hz, when reported.
    pub sample_rate: Option<i64>,
    /// Seconds.
    pub length: Option<i64>,
    /// Whether its bit rate varies, when reported.
    pub is_variable_bit_rate: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchState {
    id: String,
    #[serde(default)]
    is_complete: Option<bool>,
    #[serde(default)]
    state: Option<String>,
}

impl SearchState {
    fn done(&self) -> bool {
        self.is_complete.unwrap_or_else(|| {
            self.state
                .as_deref()
                .is_some_and(|s| s.contains("Completed"))
        })
    }
}

impl Client {
    /// A client for the slskd at `base` (its address, without `/api/v0`), using the API key `key`.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new(base: &str, key: Secret) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            key,
            poll: POLL,
            deadline: DEADLINE,
        })
    }

    /// The same client with a short poll and deadline, for tests.
    #[cfg(test)]
    pub fn quick(mut self) -> Self {
        self.poll = Duration::from_millis(20);
        self.deadline = Duration::from_millis(600);
        self
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/api/v0{path}", self.base))
            .header("X-API-Key", self.key.expose())
    }

    /// Searches Soulseek for `text` and returns every user's answer. The search is removed
    /// from slskd afterwards, finished or not.
    pub async fn search(&self, text: &str) -> anyhow::Result<Vec<Response>> {
        self.ensure_not_sharing().await?;
        let body = serde_json::json!({
            "searchText": text,
            "searchTimeout": QUIET_SECONDS,
            "responseLimit": RESPONSE_LIMIT,
            "filterResponses": true,
        });
        let response = self
            .request(reqwest::Method::POST, "/searches")
            .json(&body)
            .send()
            .await
            .map_err(failed)?;
        let started: SearchState = match response.status() {
            status if status.is_success() => read(response).await?,
            status @ (reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN) => {
                return Err(turned_down(status, "search"));
            }
            reqwest::StatusCode::TOO_MANY_REQUESTS => {
                anyhow::bail!("slskd is busy with another search; try again shortly")
            }
            // Its answer while it isn't signed in, as when it's waiting for the VPN.
            reqwest::StatusCode::CONFLICT => {
                anyhow::bail!("slskd isn't signed in to Soulseek; is its VPN up?")
            }
            status => anyhow::bail!("slskd answered {status}"),
        };
        // Only an id slskd made: it goes into URLs.
        anyhow::ensure!(
            !started.id.is_empty()
                && started
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() || b == b'-'),
            "slskd gave a search id that isn't one"
        );
        let found = self.wait_and_read(&started).await;
        // Cancelled first if still running, then removed; neither failing matters.
        let path = format!("/searches/{}", started.id);
        let _ = self.request(reqwest::Method::PUT, &path).send().await;
        let _ = self.request(reqwest::Method::DELETE, &path).send().await;
        found
    }

    async fn wait_and_read(&self, started: &SearchState) -> anyhow::Result<Vec<Response>> {
        let path = format!("/searches/{}", started.id);
        let until = tokio::time::Instant::now() + self.deadline;
        let mut done = started.done();
        while !done && tokio::time::Instant::now() < until {
            tokio::time::sleep(self.poll).await;
            let response = self
                .request(reqwest::Method::GET, &path)
                .send()
                .await
                .map_err(failed)?;
            anyhow::ensure!(
                response.status().is_success(),
                "slskd answered {}",
                response.status()
            );
            done = read::<SearchState>(response).await?.done();
        }
        let response = self
            .request(reqwest::Method::GET, &format!("{path}/responses"))
            .send()
            .await
            .map_err(failed)?;
        anyhow::ensure!(
            response.status().is_success(),
            "slskd answered {}",
            response.status()
        );
        read(response).await
    }
}

/// One file of a download batch: where it is on the peer's machine and its size.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Wanted {
    /// The path on the peer's machine, with its separators, as slskd listed it.
    pub filename: String,
    /// Its size in bytes.
    pub size: i64,
}

/// A file slskd is fetching, as its batch lists it.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    /// slskd's id for the transfer.
    pub id: String,
    /// The path on the peer's machine.
    pub filename: String,
    /// Its size in bytes.
    pub size: i64,
    /// slskd's flags, e.g. "Queued, Remotely", "InProgress", "Completed, Succeeded" or
    /// "Completed, Rejected".
    pub state: String,
    /// Bytes fetched so far.
    #[serde(default)]
    pub bytes_transferred: i64,
    /// Its place in the peer's upload queue, while queued.
    pub place_in_queue: Option<i64>,
    /// What went wrong, as slskd says, when it failed.
    pub exception: Option<String>,
}

impl Transfer {
    /// Whether the file arrived whole.
    pub fn succeeded(&self) -> bool {
        self.state.contains("Succeeded")
    }

    /// Ended without the file: rejected, timed out, cancelled or errored.
    pub fn failed(&self) -> bool {
        self.state.contains("Completed") && !self.succeeded()
    }
}

#[derive(Deserialize)]
struct Batch {
    #[serde(default)]
    transfers: Option<Vec<Transfer>>,
}

#[derive(Deserialize)]
struct Enqueued {
    #[serde(default)]
    failures: Vec<Failure>,
}

#[derive(Debug, Deserialize)]
struct Failure {
    filename: String,
    message: String,
}

/// A new batch's id: slskd takes any UUID, and Melete picks it so a download can be followed
/// even if the answer to enqueueing it is lost.
pub fn new_batch_id() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    // Version 4, variant 1.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

impl Client {
    /// How slskd says it is, from its application state: the route that tells whether it's
    /// signed in to Soulseek and that a key with the ReadWrite role may read (`/server` needs
    /// an administrator's key).
    pub async fn status(&self) -> anyhow::Result<Status> {
        let response = self
            .request(reqwest::Method::GET, "/application")
            .send()
            .await
            .map_err(failed)?;
        if !response.status().is_success() {
            return Err(turned_down(response.status(), "read how it is"));
        }
        let state: serde_json::Value = read(response).await?;
        Ok(Status {
            version: state["version"]["current"].as_str().map(str::to_owned),
            signed_in: state["server"]["isLoggedIn"] == true,
        })
    }

    /// Fails unless slskd shares nothing: nothing is ever shared from this server. Its list of
    /// shares is readable with a ReadWrite key.
    pub async fn ensure_not_sharing(&self) -> anyhow::Result<()> {
        let response = self
            .request(reqwest::Method::GET, "/shares")
            .send()
            .await
            .map_err(failed)?;
        if !response.status().is_success() {
            return Err(turned_down(response.status(), "read what it shares"));
        }
        // Each host's shares: {"local": [...]}.
        let hosts: std::collections::HashMap<String, Vec<serde_json::Value>> =
            read(response).await?;
        let shared: usize = hosts.values().map(Vec::len).sum();
        anyhow::ensure!(
            shared == 0,
            "slskd shares {shared} folder(s); Melete won't use an slskd that shares anything"
        );
        Ok(())
    }

    /// Asks slskd to fetch `files` from `username` into `destination` (relative to its
    /// downloads folder), as batch `id`. Fails if any file couldn't be queued.
    pub async fn enqueue(
        &self,
        id: &str,
        username: &str,
        destination: &str,
        files: &[Wanted],
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "id": id,
            "username": username,
            "files": files,
            "options": {"destination": destination},
        });
        // slskd queues one request at a time and turns away the rest for a moment.
        let mut tries = 0;
        let response = loop {
            let response = self
                .request(reqwest::Method::POST, "/transfers/downloads/batches")
                .json(&body)
                .send()
                .await
                .map_err(failed)?;
            tries += 1;
            if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || tries == 5 {
                break response;
            }
            tokio::time::sleep(self.poll).await;
        };
        let status = response.status();
        if status == reqwest::StatusCode::CONFLICT {
            anyhow::bail!("slskd isn't signed in to Soulseek; is its VPN up?");
        }
        if !status.is_success() {
            // slskd says why as a JSON string ("User x appears to be offline").
            let why: Option<String> = read(response).await.ok();
            match why {
                Some(why) if why.contains("must be connected and logged in") => {
                    anyhow::bail!("slskd isn't signed in to Soulseek; is its VPN up?")
                }
                Some(why) => anyhow::bail!("slskd couldn't queue it: {why}"),
                None => anyhow::bail!("slskd answered {status}"),
            }
        }
        let enqueued: Enqueued = read(response).await?;
        if let Some(first) = enqueued.failures.first() {
            anyhow::bail!(
                "slskd couldn't queue {} of the files ({}: {})",
                enqueued.failures.len(),
                first.filename,
                first.message
            );
        }
        Ok(())
    }

    /// The files of batch `id` and how each is doing; None when slskd doesn't know the batch.
    pub async fn batch(&self, id: &str) -> anyhow::Result<Option<Vec<Transfer>>> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/transfers/downloads/batches/{id}"),
            )
            .send()
            .await
            .map_err(failed)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        anyhow::ensure!(
            response.status().is_success(),
            "slskd answered {}",
            response.status()
        );
        Ok(Some(
            read::<Batch>(response).await?.transfers.unwrap_or_default(),
        ))
    }

    /// Stops a file slskd is fetching and forgets it.
    pub async fn cancel(&self, username: &str, transfer_id: &str) -> anyhow::Result<()> {
        let mut url = reqwest::Url::parse(&format!("{}/api/v0/transfers/downloads/", self.base))?;
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("slskd's address can't take a path"))?
            .pop_if_empty()
            .push(username)
            .push(transfer_id);
        url.query_pairs_mut().append_pair("remove", "true");
        let response = self
            .http
            .delete(url)
            .header("X-API-Key", self.key.expose())
            .send()
            .await
            .map_err(failed)?;
        anyhow::ensure!(
            response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
            "slskd answered {}",
            response.status()
        );
        Ok(())
    }
}

/// What slskd says of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Its version, e.g. "0.26.2", when it says.
    pub version: Option<String>,
    /// Whether it's signed in to the Soulseek network.
    pub signed_in: bool,
}

/// slskd answered but turned the key away: one it doesn't know, or one whose role is too low.
/// Says which, and the role the key needs.
#[derive(Debug)]
pub struct TurnedDown(String);

impl std::fmt::Display for TurnedDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TurnedDown {}

/// Why slskd didn't do what Melete asked (`asked`), in words an admin can act on.
fn turned_down(status: reqwest::StatusCode, asked: &str) -> anyhow::Error {
    match status {
        reqwest::StatusCode::UNAUTHORIZED => {
            TurnedDown(format!("slskd refused the API key ({status})")).into()
        }
        reqwest::StatusCode::FORBIDDEN => TurnedDown(format!(
            "slskd won't let this key {asked} ({status}); Melete's key needs slskd's ReadWrite role"
        ))
        .into(),
        status => anyhow::anyhow!("slskd answered {status} when asked to {asked}"),
    }
}

/// A request that didn't get an answer, without its URL.
fn failed(err: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("slskd: {}", err.without_url())
}

async fn read<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> anyhow::Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(failed)? {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= MAX_BYTES,
            "slskd's answer is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|err| anyhow::anyhow!("slskd's answer: {err}"))
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post};

    use crate::config::Secret;

    pub const KEY: &str = "test-key";
    pub const SEARCH_ID: &str = "0d0f4f9e-6a1b-4f5e-9f1c-1b2c3d4e5f60";

    /// What the fake was asked (search requests as "METHOD path", the search texts, batches
    /// enqueued, transfers cancelled), and how it's set to answer.
    #[derive(Default)]
    pub struct Seen {
        pub requests: Vec<String>,
        pub texts: Vec<String>,
        /// Times it was asked what it shares.
        pub shares_asked: usize,
        /// Folders it says it shares.
        pub shared: usize,
        /// Whether it says it isn't signed in to Soulseek.
        pub signed_out: bool,
        /// Whether it turns the key away from every route, as from a key whose role is too low.
        pub forbidden: bool,
        /// Times an administrator's route was asked for.
        pub admin_asked: usize,
        /// Its address.
        pub base: String,
        /// Enqueue requests' bodies, as sent.
        pub enqueued: Vec<serde_json::Value>,
        /// Answers enqueueing with this error, as slskd says it.
        pub refuse_enqueue: Option<String>,
        /// Every transfer's state; "Queued, Remotely" when empty.
        pub transfer_state: String,
        /// Batches it no longer knows.
        pub forgotten: bool,
        /// "username/transfer id" of each transfer cancelled.
        pub cancelled: Vec<String>,
    }

    #[derive(Clone)]
    struct Fake {
        seen: Arc<Mutex<Seen>>,
        responses: serde_json::Value,
        polls_until_done: Arc<Mutex<u32>>,
    }

    fn authorized(headers: &HeaderMap) -> bool {
        headers.get("x-api-key").and_then(|v| v.to_str().ok()) == Some(KEY)
    }

    /// As slskd answers a route any role may read: 401 to a key it doesn't know, 403 when the
    /// key's role is set too low for everything.
    fn allowed(headers: &HeaderMap, seen: &Seen) -> Result<(), StatusCode> {
        match (authorized(headers), seen.forbidden) {
            (false, _) => Err(StatusCode::UNAUTHORIZED),
            (true, true) => Err(StatusCode::FORBIDDEN),
            (true, false) => Ok(()),
        }
    }

    /// As slskd answers an administrator's route to a ReadWrite key.
    async fn admin_only(State(fake): State<Fake>, headers: HeaderMap) -> StatusCode {
        let mut seen = fake.seen.lock().unwrap();
        seen.admin_asked += 1;
        if authorized(&headers) {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::UNAUTHORIZED
        }
    }

    /// A fake slskd whose searches finish after `polls` looks and answer with `responses`.
    pub async fn start(
        responses: serde_json::Value,
        polls: u32,
    ) -> (super::Client, Arc<Mutex<Seen>>) {
        let fake = Fake {
            seen: Arc::default(),
            responses,
            polls_until_done: Arc::new(Mutex::new(polls)),
        };
        let seen = fake.seen.clone();
        let router = axum::Router::new()
            .route(
                "/api/v0/searches",
                post(
                    |State(fake): State<Fake>,
                     headers: HeaderMap,
                     axum::Json(body): axum::Json<serde_json::Value>| async move {
                        if !authorized(&headers) {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        let mut seen = fake.seen.lock().unwrap();
                        seen.requests.push("POST /searches".into());
                        // As slskd answers before it's signed in.
                        if body["searchText"] == "signed out" {
                            return (
                                StatusCode::CONFLICT,
                                axum::Json("The server connection must be connected and logged in to perform a search (currently: Disconnected)"),
                            )
                                .into_response();
                        }
                        seen.texts
                            .push(body["searchText"].as_str().unwrap_or_default().into());
                        axum::Json(serde_json::json!({
                            "id": SEARCH_ID, "searchText": body["searchText"],
                            "state": "InProgress", "isComplete": false,
                        }))
                        .into_response()
                    },
                ),
            )
            .route(
                "/api/v0/searches/{id}",
                get(
                    |State(fake): State<Fake>, Path(id): Path<String>| async move {
                        fake.seen
                            .lock()
                            .unwrap()
                            .requests
                            .push(format!("GET /searches/{id}"));
                        let mut left = fake.polls_until_done.lock().unwrap();
                        *left = left.saturating_sub(1);
                        let done = *left == 0;
                        axum::Json(serde_json::json!({
                            "id": id,
                            "state": if done { "Completed, TimedOut" } else { "InProgress" },
                            "isComplete": done,
                        }))
                    },
                )
                .put(
                    |State(fake): State<Fake>, Path(id): Path<String>| async move {
                        fake.seen
                            .lock()
                            .unwrap()
                            .requests
                            .push(format!("PUT /searches/{id}"));
                        StatusCode::OK
                    },
                )
                .delete(
                    |State(fake): State<Fake>, Path(id): Path<String>| async move {
                        fake.seen
                            .lock()
                            .unwrap()
                            .requests
                            .push(format!("DELETE /searches/{id}"));
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            // An administrator's routes, which the ReadWrite key Melete is given can't read.
            .route("/api/v0/server", get(admin_only))
            .route("/api/v0/options", get(admin_only))
            .route(
                "/api/v0/application",
                get(|State(fake): State<Fake>, headers: HeaderMap| async move {
                    let seen = fake.seen.lock().unwrap();
                    if let Err(status) = allowed(&headers, &seen) {
                        return status.into_response();
                    }
                    let signed_in = !seen.signed_out;
                    axum::Json(serde_json::json!({
                        "version": {"full": "0.26.2.0", "current": "0.26.2"},
                        "server": {
                            "isConnected": signed_in, "isLoggedIn": signed_in,
                            "state": if signed_in { "Connected, LoggedIn" } else { "Disconnected" },
                        },
                        "shares": {"directories": seen.shared, "files": seen.shared * 10},
                    }))
                    .into_response()
                }),
            )
            .route(
                "/api/v0/shares",
                get(|State(fake): State<Fake>, headers: HeaderMap| async move {
                    let mut seen = fake.seen.lock().unwrap();
                    if let Err(status) = allowed(&headers, &seen) {
                        return status.into_response();
                    }
                    seen.shares_asked += 1;
                    let shares: Vec<serde_json::Value> = (0..seen.shared)
                        .map(|n| serde_json::json!({"id": format!("share{n}")}))
                        .collect();
                    axum::Json(serde_json::json!({ "local": shares })).into_response()
                }),
            )
            .route(
                "/api/v0/transfers/downloads/batches",
                post(
                    |State(fake): State<Fake>,
                     headers: HeaderMap,
                     axum::Json(body): axum::Json<serde_json::Value>| async move {
                        if !authorized(&headers) {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        let mut seen = fake.seen.lock().unwrap();
                        if let Some(why) = seen.refuse_enqueue.clone() {
                            return (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(why))
                                .into_response();
                        }
                        seen.enqueued.push(body.clone());
                        axum::Json(serde_json::json!({
                            "batch": {"id": body["id"], "username": body["username"]},
                            "failures": [],
                        }))
                        .into_response()
                    },
                ),
            )
            .route(
                "/api/v0/transfers/downloads/batches/{id}",
                get(
                    |State(fake): State<Fake>, Path(id): Path<String>| async move {
                        let seen = fake.seen.lock().unwrap();
                        let found = seen.enqueued.iter().find(|b| b["id"] == id.as_str());
                        let (Some(batch), false) = (found, seen.forgotten) else {
                            return StatusCode::NOT_FOUND.into_response();
                        };
                        let state = match seen.transfer_state.as_str() {
                            "" => "Queued, Remotely",
                            state => state,
                        };
                        let transfers: Vec<serde_json::Value> = batch["files"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .enumerate()
                            .map(|(n, f)| {
                                let size = f["size"].as_i64().unwrap_or(0);
                                serde_json::json!({
                                    "id": format!("{id}-{n}"), "filename": f["filename"],
                                    "size": size, "state": state, "placeInQueue": 3,
                                    "bytesTransferred": match state {
                                        "Completed, Succeeded" => size,
                                        "InProgress" => size / 2,
                                        _ => 0,
                                    },
                                })
                            })
                            .collect();
                        axum::Json(serde_json::json!({
                            "id": id, "username": batch["username"], "transfers": transfers,
                        }))
                        .into_response()
                    },
                ),
            )
            .route(
                "/api/v0/transfers/downloads/{username}/{id}",
                axum::routing::delete(
                    |State(fake): State<Fake>, Path((username, id)): Path<(String, String)>| async move {
                        fake.seen.lock().unwrap().cancelled.push(format!("{username}/{id}"));
                        StatusCode::NO_CONTENT
                    },
                ),
            )
            .route(
                "/api/v0/searches/{id}/responses",
                get(
                    |State(fake): State<Fake>, Path(id): Path<String>| async move {
                        fake.seen
                            .lock()
                            .unwrap()
                            .requests
                            .push(format!("GET /searches/{id}/responses"));
                        axum::Json(fake.responses.clone())
                    },
                ),
            )
            .with_state(fake);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        seen.lock().unwrap().base = base.clone();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = super::Client::new(&base, Secret::for_tests(KEY))
            .unwrap()
            .quick();
        (client, seen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer() -> serde_json::Value {
        serde_json::json!([{
            "username": "alice", "fileCount": 2, "hasFreeUploadSlot": true, "queueLength": 0,
            "uploadSpeed": 1048576, "token": 1, "lockedFileCount": 0, "lockedFiles": [],
            "files": [
                {"filename": "@@music\\Radiohead\\OK Computer\\01 Airbag.flac", "size": 30000000,
                 "bitDepth": 16, "sampleRate": 44100, "length": 284, "extension": "flac",
                 "code": 1, "isLocked": false},
                {"filename": "@@music\\Radiohead\\OK Computer\\cover.jpg", "size": 100000,
                 "extension": "jpg", "code": 1, "isLocked": false}
            ]
        }])
    }

    #[tokio::test]
    async fn searches_waits_reads_and_cleans_up() {
        let (client, seen) = testing::start(answer(), 3).await;
        let found = client.search("Radiohead OK Computer").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].username, "alice");
        assert!(found[0].has_free_upload_slot);
        assert_eq!(found[0].files[0].bit_depth, Some(16));
        assert_eq!(found[0].files[0].length, Some(284));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.texts, ["Radiohead OK Computer"]);
        let id = testing::SEARCH_ID;
        assert_eq!(
            seen.requests,
            [
                "POST /searches".to_owned(),
                format!("GET /searches/{id}"),
                format!("GET /searches/{id}"),
                format!("GET /searches/{id}"),
                format!("GET /searches/{id}/responses"),
                format!("PUT /searches/{id}"),
                format!("DELETE /searches/{id}"),
            ]
        );
    }

    #[tokio::test]
    async fn a_search_that_runs_long_is_read_as_it_stands() {
        let (client, seen) = testing::start(answer(), 1_000).await;
        let found = client.search("Radiohead OK Computer").await.unwrap();
        assert_eq!(found.len(), 1, "what came in before the deadline");
        assert!(
            seen.lock()
                .unwrap()
                .requests
                .iter()
                .any(|r| r.starts_with("DELETE"))
        );
    }

    /// slskd's administrator routes turn a ReadWrite key away; nothing Melete asks needs them.
    /// A key whose role is too low is told apart from a wrong one, and nothing goes ahead.
    #[tokio::test]
    async fn a_read_write_key_is_enough_and_a_lower_role_is_named() {
        let (client, seen) = testing::start(answer(), 1).await;
        let status = client.status().await.unwrap();
        assert_eq!(
            status,
            Status {
                version: Some("0.26.2".into()),
                signed_in: true
            }
        );
        client.ensure_not_sharing().await.unwrap();
        client.search("Radiohead OK Computer").await.unwrap();
        assert_eq!(seen.lock().unwrap().admin_asked, 0);

        seen.lock().unwrap().forbidden = true;
        let refused = [
            client.status().await.unwrap_err(),
            client.ensure_not_sharing().await.unwrap_err(),
            client.search("Radiohead OK Computer").await.unwrap_err(),
        ];
        for err in refused {
            assert!(err.is::<TurnedDown>(), "{err}");
            assert!(
                err.to_string()
                    .ends_with("(403 Forbidden); Melete's key needs slskd's ReadWrite role"),
                "{err}"
            );
        }
        assert_eq!(seen.lock().unwrap().texts, ["Radiohead OK Computer"]);
    }

    #[tokio::test]
    async fn a_wrong_key_or_no_slskd_is_an_error_without_the_key() {
        let (client, _) = testing::start(answer(), 1).await;
        let wrong = Client::new(&client.base, Secret::for_tests("nope")).unwrap();
        let err = wrong.search("x").await.unwrap_err().to_string();
        assert!(err.contains("refused the API key"), "{err}");
        let err = client.search("signed out").await.unwrap_err().to_string();
        assert_eq!(err, "slskd isn't signed in to Soulseek; is its VPN up?");
        let away = Client::new("http://127.0.0.1:9", Secret::for_tests("s3cret")).unwrap();
        let err = format!("{:#}", away.search("x").await.unwrap_err());
        assert!(err.starts_with("slskd: "), "{err}");
        assert!(
            !err.contains("s3cret") && !err.contains("127.0.0.1"),
            "{err}"
        );
    }
}
