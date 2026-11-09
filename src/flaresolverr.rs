//! FlareSolverr: a browser in its own container that gets past Cloudflare's check and
//! hands back the page, or a solver that copies its API (Byparr). Used only for trackers behind
//! that check (Uindex), and only when `MELETE_FLARESOLVERR_URL` is set. Only the page's HTML is
//! taken; its cookies and the browser's identity stay there.

use std::time::Duration;

use serde_json::{Value, json};

/// How long FlareSolverr may take over one page; solving a challenge takes seconds.
const MAX_TIMEOUT_MS: u64 = 60_000;

/// The first FlareSolverr that reads the `proxy` field of a page request: its 3.0.0 rewrite had
/// no proxy support, and 3.2.0 brought it back ("Support "proxy" param in requests and
/// sessions", its changelog). An earlier or unknown one isn't handed the proxy.
pub const PROXY_FROM: &str = "3.2.0";

/// Whether FlareSolverr `version` (such as "v3.3.21" or "3.4.0") is [`PROXY_FROM`] or later.
pub(crate) fn takes_a_proxy(version: &str) -> bool {
    let parts: Vec<u32> = version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map_while(|p| p.parse().ok())
        .collect();
    parts.len() >= 2 && (parts[0], parts[1]) >= (3, 2)
}

/// What answered at FlareSolverr's address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Solver {
    /// FlareSolverr itself, with its version when it says.
    FlareSolverr(Option<String>),
    /// A solver that takes FlareSolverr's page requests but not its other commands, as Byparr
    /// does.
    Compatible,
}

impl std::fmt::Display for Solver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Solver::FlareSolverr(Some(version)) => write!(f, "FlareSolverr {version}"),
            Solver::FlareSolverr(None) => f.write_str("FlareSolverr"),
            Solver::Compatible => f.write_str("a FlareSolverr-compatible solver"),
        }
    }
}

/// Talks to one FlareSolverr.
#[derive(Clone)]
pub struct Client {
    http: crate::proxy::Http,
    /// Its API, e.g. http://flaresolverr:8191/v1.
    endpoint: String,
    /// Whether the pages it fetches are for an indexer set to go through the proxy.
    through: bool,
}

impl Client {
    /// A client for the FlareSolverr at `base` (its address, without `/v1`). Nothing is sent
    /// yet.
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be built (no TLS backend).
    pub fn new(base: &str) -> anyhow::Result<Self> {
        let http = crate::proxy::Http::new(crate::proxy::Service::FlareSolverr, |b| {
            b.timeout(Duration::from_millis(MAX_TIMEOUT_MS + 15_000))
                .redirect(reqwest::redirect::Policy::none())
        })?;
        Ok(Self {
            http,
            endpoint: format!("{}/v1", base.trim_end_matches('/')),
            through: false,
        })
    }

    /// The same solver, its browser fetching through the proxy when `proxy` is true.
    pub fn through(self, proxy: bool) -> Self {
        Self {
            through: proxy,
            ..self
        }
    }

    /// The `proxy` field to send for a page, None to fetch direct. FlareSolverr's browser is
    /// handed an http or socks5 proxy (socks5 has the proxy look names up), by its address alone.
    /// One that can't be handed over is refused, never left out, as the page would then be
    /// fetched from the solver's own address: an HTTPS proxy (the browser takes none), and a
    /// proxy with a login, whose password would otherwise travel in FlareSolverr's request
    /// body, and over plain http on the way there. So is a solver that doesn't say it's
    /// FlareSolverr [`PROXY_FROM`] or later, which may leave the field unread.
    async fn proxy_field(&self) -> Result<Option<Value>, String> {
        use crate::proxy::Kind;
        if !self.through {
            return Ok(None);
        }
        let applied = crate::proxy::current();
        let Some(proxy) = &applied.setup.at else {
            return Err(crate::proxy::Unreachable::NoProxy.to_string());
        };
        let at = format!("{}:{}", proxy.host, proxy.port);
        let refused = |why: &str| {
            Err(format!(
                "{why}: the proxy at {} isn't handed to it, and nothing was sent; it never goes direct",
                proxy.address()
            ))
        };
        let field = match (proxy.kind, &proxy.login) {
            (_, Some(_)) => {
                return refused("FlareSolverr is never given a proxy's password");
            }
            (Kind::Socks5, None) => json!({ "url": format!("socks5://{at}") }),
            (Kind::Http, None) => json!({ "url": format!("http://{at}") }),
            (Kind::Https, None) => {
                return refused("FlareSolverr's browser can't use an HTTPS proxy");
            }
        };
        match self.ready().await {
            Ok(Solver::FlareSolverr(Some(version))) if takes_a_proxy(&version) => Ok(Some(field)),
            Ok(Solver::FlareSolverr(version)) => Err(format!(
                "FlareSolverr {} may not read the proxy it's handed (it does from {PROXY_FROM}), so nothing was sent through it",
                version.as_deref().unwrap_or("of an unknown version")
            )),
            Ok(Solver::Compatible) => Err("the solver at FlareSolverr's address isn't FlareSolverr itself, so it isn't trusted to fetch through the proxy, and nothing was sent".into()),
            Err(why) => Err(why),
        }
    }

    /// What answers at the address, asked for FlareSolverr's sessions: nothing is fetched. A
    /// solver that copies only FlareSolverr's page requests (Byparr) turns the command down in
    /// an answer of its own, which still shows a solver is there.
    pub async fn ready(&self) -> Result<Solver, String> {
        let response = self
            .http
            .post(&self.endpoint)
            .json(&json!({ "cmd": "sessions.list" }))
            .send()
            .await
            .map_err(|err| {
                format!(
                    "FlareSolverr didn't answer at that address ({}). Check it's running and the address is as Melete reaches it.",
                    err.without_url()
                )
            })?;
        let failed = !response.status().is_success();
        let body: Value = response
            .json()
            .await
            .map_err(|err| format!("FlareSolverr's answer: {}", err.without_url()))?;
        if body["status"] == "ok" && body["sessions"].is_array() {
            let version = body["version"]
                .as_str()
                .map(|v| v.chars().take(20).collect());
            return Ok(Solver::FlareSolverr(version));
        }
        // FlareSolverr's own refusal ({"status": "error", "message": …}), or the error a solver
        // built on FastAPI gives ({"detail": …}) for a page it can't open.
        let refused = body["status"] == "error" && body["message"].is_string();
        if refused || (failed && body.get("detail").is_some()) {
            return Ok(Solver::Compatible);
        }
        Err("Something answered at that address, but not as FlareSolverr does.".into())
    }

    /// The HTML of a page, as a browser gets it.
    pub async fn get(&self, url: &str) -> Result<String, String> {
        let mut request = json!({ "cmd": "request.get", "url": url, "maxTimeout": MAX_TIMEOUT_MS });
        if let Some(proxy) = self.proxy_field().await? {
            request["proxy"] = proxy;
        }
        let response = self
            .http
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|err| format!("FlareSolverr: {}", err.without_url()))?;
        let body: Value = response
            .json()
            .await
            .map_err(|err| format!("FlareSolverr's answer: {}", err.without_url()))?;
        if body["status"] != "ok" {
            let message = body["message"].as_str().unwrap_or("no reason given");
            return Err(format!(
                "FlareSolverr: {}",
                message.chars().take(200).collect::<String>()
            ));
        }
        let status = body["solution"]["status"].as_i64().unwrap_or(0);
        if status != 200 {
            return Err(format!("the site answered {status} through FlareSolverr"));
        }
        body["solution"]["response"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "FlareSolverr gave no page".to_owned())
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use serde_json::{Value, json};

    /// Which solver a fake plays.
    #[derive(Clone, Copy)]
    pub enum Plays {
        /// FlareSolverr, giving this version with its sessions (none: it leaves it out).
        FlareSolverr(Option<&'static str>),
        /// Byparr, which takes every command as a page request, as its code does: a command
        /// without a page is a page it can't open, refused as FastAPI refuses.
        Byparr,
    }

    /// A fake FlareSolverr answering every page with `html`; the URLs asked for are logged.
    pub async fn start(html: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        start_as(Plays::FlareSolverr(None), html).await
    }

    /// A fake of the solver `plays`, answering every page with `html`; the URLs asked for are
    /// logged, each after `proxy <its proxy field>` when it came with one.
    pub async fn start_as(plays: Plays, html: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let router =
            axum::Router::new()
                .route(
                    "/v1",
                    axum::routing::post(
                        move |State(log): State<Arc<Mutex<Vec<String>>>>,
                              Json(body): Json<Value>| async move {
                            let url = body["url"].as_str().unwrap_or_default().to_owned();
                            if let Some(proxy) = body.get("proxy") {
                                log.lock().unwrap().push(format!("proxy {proxy}"));
                            }
                            log.lock().unwrap().push(url.clone());
                            answer(plays, &body, &url, html)
                        },
                    ),
                )
                .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, log)
    }

    fn answer(plays: Plays, body: &Value, url: &str, html: &str) -> Response {
        let page = json!({ "status": "ok", "message": "Challenge solved!",
                           "solution": { "status": 200, "response": html } });
        match plays {
            Plays::FlareSolverr(version) if body["cmd"] == "sessions.list" => {
                let mut sessions = json!({ "status": "ok", "message": "", "sessions": [] });
                if let Some(version) = version {
                    sessions["version"] = json!(version);
                }
                Json(sessions).into_response()
            }
            Plays::FlareSolverr(_) if body["cmd"] != "request.get" => {
                Json(json!({ "status": "error", "message": "unknown cmd" })).into_response()
            }
            Plays::Byparr if !url.starts_with("http") => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "detail": "Could not reach the target: invalid URL" })),
            )
                .into_response(),
            _ => Json(page).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Plays, start_as};
    use super::*;

    #[tokio::test]
    async fn says_which_solver_answers_and_byparr_still_solves() {
        let (flaresolverr, _) = start_as(Plays::FlareSolverr(Some("3.3.21")), "").await;
        let found = Client::new(&flaresolverr).unwrap().ready().await.unwrap();
        assert_eq!(found, Solver::FlareSolverr(Some("3.3.21".into())));
        assert_eq!(found.to_string(), "FlareSolverr 3.3.21");

        let (byparr, log) = start_as(Plays::Byparr, "<html>page</html>").await;
        let byparr = Client::new(&byparr).unwrap();
        let found = byparr.ready().await.unwrap();
        assert_eq!(found, Solver::Compatible);
        assert_eq!(found.to_string(), "a FlareSolverr-compatible solver");
        assert_eq!(
            byparr.get("https://example.org/").await.unwrap(),
            "<html>page</html>"
        );
        assert_eq!(
            *log.lock().unwrap(),
            ["", "https://example.org/"],
            "nothing fetched to check"
        );
    }

    #[tokio::test]
    async fn something_else_at_the_address_is_no_solver() {
        let router = axum::Router::new().fallback(|| async {
            (
                axum::http::StatusCode::NOT_FOUND,
                axum::Json(json!({ "error": "no such page" })),
            )
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let err = Client::new(&url).unwrap().ready().await.unwrap_err();
        assert_eq!(
            err,
            "Something answered at that address, but not as FlareSolverr does."
        );
    }
}
