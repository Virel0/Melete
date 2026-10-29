//! FlareSolverr: a browser in its own container that gets past Cloudflare's check and
//! hands back the page. Used only for trackers behind that check (Uindex), and only when
//! `MELETE_FLARESOLVERR_URL` is set. Only the page's HTML is taken; its cookies and the
//! browser's identity stay there.

use std::time::Duration;

use serde_json::{Value, json};

/// How long FlareSolverr may take over one page; solving a challenge takes seconds.
const MAX_TIMEOUT_MS: u64 = 60_000;

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

    /// Whether FlareSolverr answers, asked for its sessions: nothing is fetched.
    pub async fn ready(&self) -> Result<(), String> {
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
        let body: Value = response
            .json()
            .await
            .map_err(|err| format!("FlareSolverr's answer: {}", err.without_url()))?;
        match body["status"].as_str() {
            Some("ok") => Ok(()),
            _ => Err(format!(
                "FlareSolverr: {}",
                body["message"]
                    .as_str()
                    .unwrap_or("it didn't say it's ready")
            )),
        }
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
    use serde_json::{Value, json};

    /// A fake FlareSolverr answering every page with `html`; the URLs asked for are logged.
    pub async fn start(html: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let router =
            axum::Router::new()
                .route(
                    "/v1",
                    axum::routing::post(
                        move |State(log): State<Arc<Mutex<Vec<String>>>>,
                              Json(body): Json<Value>| async move {
                            log.lock()
                                .unwrap()
                                .push(body["url"].as_str().unwrap_or_default().to_owned());
                            if body["cmd"] == "sessions.list" {
                                return Json(
                                    json!({ "status": "ok", "message": "", "sessions": [] }),
                                );
                            }
                            if body["cmd"] != "request.get" {
                                return Json(
                                    json!({ "status": "error", "message": "unknown cmd" }),
                                );
                            }
                            Json(json!({ "status": "ok", "message": "Challenge solved!",
                                     "solution": { "status": 200, "response": html } }))
                        },
                    ),
                )
                .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, log)
    }
}
