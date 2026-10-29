//! LibreTranslate, for translating lyrics, off unless `MELETE_LIBRETRANSLATE_URL` is
//! set: a server the household runs itself, or one it has a key for. What leaves the server:
//! the lines of the lyrics being translated and the language asked for; nothing about who
//! asked. The key, when there is one, goes in the request's body, never its address.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::config::Secret;

/// Lyrics are short; an answer this big isn't a translation of them.
const MAX_BYTES: usize = 1024 * 1024;

/// A LibreTranslate client, holding its key when there is one.
pub struct Client {
    http: reqwest::Client,
    base: String,
    api_key: Option<Secret>,
}

#[derive(Deserialize)]
struct Translated {
    #[serde(rename = "translatedText")]
    translated: Vec<String>,
}

#[derive(Deserialize)]
struct Language {
    code: String,
}

impl Client {
    /// A client for the LibreTranslate at `base`, with `api_key` when it needs one.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new(base: &str, api_key: Option<Secret>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            api_key,
        })
    }

    async fn body(response: reqwest::Response) -> anyhow::Result<Vec<u8>> {
        let status = response.status();
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| anyhow::anyhow!("LibreTranslate: {}", err.without_url()))?
        {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= MAX_BYTES,
                "LibreTranslate's answer is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            // Its error says why (a wrong key, an unknown language); never anything secret.
            let why = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or_default();
            anyhow::bail!("LibreTranslate answered {status}: {why}");
        }
        Ok(bytes)
    }

    /// The languages it translates into, by code ("en", "de", "zh-Hans").
    pub async fn languages(&self) -> anyhow::Result<Vec<String>> {
        let response = self
            .http
            .get(format!("{}/languages", self.base))
            .send()
            .await
            .map_err(|err| anyhow::anyhow!("LibreTranslate: {}", err.without_url()))?;
        let languages: Vec<Language> = serde_json::from_slice(&Self::body(response).await?)
            .map_err(|err| anyhow::anyhow!("LibreTranslate's answer: {err}"))?;
        Ok(languages.into_iter().map(|l| l.code).collect())
    }

    /// Each of `lines` in `target`, in the same order, the source language worked out by
    /// LibreTranslate.
    pub async fn translate(&self, lines: &[String], target: &str) -> anyhow::Result<Vec<String>> {
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        let mut body = json!({ "q": lines, "source": "auto", "target": target, "format": "text" });
        if let Some(key) = &self.api_key {
            body["api_key"] = json!(key.expose());
        }
        let response = self
            .http
            .post(format!("{}/translate", self.base))
            .json(&body)
            .send()
            .await
            .map_err(|err| anyhow::anyhow!("LibreTranslate: {}", err.without_url()))?;
        let answer: Translated = serde_json::from_slice(&Self::body(response).await?)
            .map_err(|err| anyhow::anyhow!("LibreTranslate's answer: {err}"))?;
        anyhow::ensure!(
            answer.translated.len() == lines.len(),
            "LibreTranslate answered {} lines for {}",
            answer.translated.len(),
            lines.len()
        );
        Ok(answer.translated)
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use serde_json::{Value, json};

    /// Each translate request's body, in order.
    pub type Log = Arc<Mutex<Vec<Value>>>;
    pub const KEY: &str = "translate-key";

    /// A fake LibreTranslate: knows "en" (lines as they are) and "de" (lines in capitals),
    /// refuses other targets, and wants [`KEY`] when `keyed`.
    pub async fn start(keyed: bool) -> (String, Log) {
        let log: Log = Arc::default();
        let router = axum::Router::new()
            .route(
                "/languages",
                axum::routing::get(|| async { Json(json!([{ "code": "en" }, { "code": "de" }])) }),
            )
            .route(
                "/translate",
                axum::routing::post(
                    move |State(log): State<Log>, Json(body): Json<Value>| async move {
                        log.lock().unwrap().push(body.clone());
                        if keyed && body["api_key"] != KEY {
                            return (
                                StatusCode::FORBIDDEN,
                                Json(json!({ "error": "Invalid API key" })),
                            )
                                .into_response();
                        }
                        let target = body["target"].as_str().unwrap_or_default().to_owned();
                        if target != "de" && target != "en" {
                            return (
                                StatusCode::BAD_REQUEST,
                                Json(json!({ "error": format!("{target} is not supported") })),
                            )
                                .into_response();
                        }
                        let lines: Vec<String> = body["q"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|l| l.as_str().unwrap())
                            .map(|l| match target.as_str() {
                                "de" => l.to_uppercase(),
                                _ => l.to_owned(),
                            })
                            .collect();
                        Json(json!({ "translatedText": lines })).into_response()
                    },
                ),
            )
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (base, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn translates_lines_in_order_with_the_key_in_the_body() {
        let (base, log) = testing::start(true).await;
        let client = Client::new(&base, Some(Secret::for_tests(testing::KEY))).unwrap();
        let lines = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(
            client.translate(&lines, "de").await.unwrap(),
            ["ONE", "TWO"]
        );
        assert_eq!(client.languages().await.unwrap(), ["en", "de"]);
        let sent = log.lock().unwrap()[0].clone();
        assert_eq!(sent["source"], "auto");
        let mut keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["api_key", "format", "q", "source", "target"]);

        let unkeyed = Client::new(&base, None).unwrap();
        let err = unkeyed.translate(&lines, "de").await.unwrap_err();
        assert!(err.to_string().contains("Invalid API key"), "{err}");
        let gone = Client::new("http://127.0.0.1:9", None).unwrap();
        assert!(gone.translate(&lines, "de").await.is_err());
    }
}
