//! FlareSolverr: a browser in its own container that gets past Cloudflare's check and
//! hands back the page, or a solver that copies its API (Byparr). Used only for trackers behind
//! that check (Uindex), and only when `MELETE_FLARESOLVERR_URL` is set. Only the page's HTML is
//! taken; its cookies and the browser's identity stay there.

use std::time::Duration;

use serde_json::{Value, json};

/// How long FlareSolverr may take over one page; solving a challenge takes seconds.
const MAX_TIMEOUT_MS: u64 = 60_000;

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
    http: reqwest::Client,
    /// Its API, e.g. http://flaresolverr:8191/v1.
    endpoint: String,
}

impl Client {
    /// A client for the FlareSolverr at `base` (its address, without `/v1`). Nothing is sent
    /// yet.
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be built (no TLS backend).
    pub fn new(base: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(MAX_TIMEOUT_MS + 15_000))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            endpoint: format!("{}/v1", base.trim_end_matches('/')),
        })
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
        let request = json!({ "cmd": "request.get", "url": url, "maxTimeout": MAX_TIMEOUT_MS });
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
    /// logged.
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
