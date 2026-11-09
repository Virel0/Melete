//! AI features through OpenRouter, off unless `MELETE_OPENROUTER_KEY` is set. One key on
//! the server, so no device holds one; the only code that talks to OpenRouter.
//!
//! What leaves the server for a search: the words someone typed to describe what they're
//! after, and the instructions below; nothing about the library, the household or who asked.
//! The model answers with titles it knows, which Melete then looks for in the library itself.
//! For music search and mixes (`music_search`, `music_mix`): the words someone typed, and the
//! names of the library's genres they may see, so the model uses the library's own; the model
//! answers with filters or rules, never tracks, and the apps run those against the library.
//! For picks (`picks`), while an admin has them on: titles with their year, up to three genres
//! and a short premise, and nothing else. The key
//! goes in the Authorization header only, and never into a log, an error or a URL. No
//! attribution headers are sent, and OpenRouter is asked to route only to providers that don't
//! keep or train on what they're sent (`data_collection: deny`).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{OpenRouter, Secret};

mod budget;
mod music_mix;
mod music_search;
mod picks;

pub use budget::*;
pub use picks::*;

const USER_AGENT: &str = concat!("Melete/", env!("CARGO_PKG_VERSION"));
const API: &str = "https://openrouter.ai/api/v1";
/// Answers bigger than this aren't read.
const MAX_ANSWER: usize = 1 << 20;
/// Titles a description may bring back.
pub const MAX_SUGGESTIONS: usize = 30;
/// The longest description sent.
pub const MAX_DESCRIPTION: usize = 300;

/// What kind of title to suggest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleKind {
    Movie,
    Series,
}

/// A title the model suggested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The title as the model gave it.
    pub title: String,
    /// Its year, when the model gave one.
    pub year: Option<i64>,
}

/// Why OpenRouter couldn't answer, in words for the person who asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The key was refused.
    Key,
    /// The account is out of credit.
    Credit,
    /// Too many requests just now.
    Busy,
    /// Anything else: no answer, a bad one.
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Key => f.write_str("OpenRouter refused the key"),
            Failure::Credit => f.write_str("the OpenRouter account is out of credit"),
            Failure::Busy => f.write_str("OpenRouter is busy; try again in a minute"),
            Failure::Other(why) => write!(f, "OpenRouter: {why}"),
        }
    }
}

impl std::error::Error for Failure {}

/// OpenRouter as Melete talks to it, with the household's one key.
pub struct Client {
    http: reqwest::Client,
    key: Secret,
    model: String,
    api: String,
    budget: Arc<Budget>,
}

impl Client {
    /// A client for OpenRouter's API, with timeouts and no redirects (the key is in a header).
    pub fn new(settings: &OpenRouter) -> anyhow::Result<Self> {
        Self::with_api(settings, API, Arc::default())
    }

    /// As [`Client::new`], spending from `budget`, which outlasts the client.
    pub fn with_budget(settings: &OpenRouter, budget: Arc<Budget>) -> anyhow::Result<Self> {
        Self::with_api(settings, API, budget)
    }

    /// What AI picks may spend.
    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    fn with_api(settings: &OpenRouter, api: &str, budget: Arc<Budget>) -> anyhow::Result<Self> {
        let http = crate::proxy::builder(crate::proxy::Service::OpenRouter)?
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            // The key is in a header; it goes to OpenRouter and nowhere it might send us.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            key: settings.key.clone(),
            model: settings.model.clone(),
            api: api.trim_end_matches('/').to_owned(),
            budget,
        })
    }

    /// Whether OpenRouter takes the key, asked about the key itself: no model runs.
    pub async fn check(&self) -> Result<(), Failure> {
        let answer = self
            .http
            .get(format!("{}/auth/key", self.api))
            .bearer_auth(self.key.expose())
            .send()
            .await
            .map_err(|e| Failure::Other(format!("no answer ({})", e.without_url())))?;
        status(answer.status())
    }

    /// Titles of the kind asked for that fit `description`, best first, as the model knows
    /// them.
    pub async fn suggest(
        &self,
        kind: TitleKind,
        description: &str,
    ) -> Result<Vec<Suggestion>, Failure> {
        let what = match kind {
            TitleKind::Movie => "films",
            TitleKind::Series => "television series",
        };
        let system = format!(
            "You suggest {what} that fit a description someone gives. Answer with JSON only, \
             no other text: {{\"titles\": [{{\"title\": \"...\", \"year\": 1999}}]}}, at most \
             {MAX_SUGGESTIONS} entries, best fit first, each title as it was released in English \
             where it was, with the year it first came out. If nothing fits, answer \
             {{\"titles\": []}}."
        );
        let answer = self.complete(&system, description).await?;
        parse_suggestions(&answer).ok_or_else(|| {
            Failure::Other("the model's answer wasn't the list asked for".to_owned())
        })
    }

    /// The model's answer to `user` under `system`. Thinking is asked to be off: these tasks
    /// don't need it, and a model's thinking counts against the same token limit as its answer,
    /// so a model that thinks at length can stop before it answers. A model that can't turn
    /// thinking off refuses that, and is asked again once with it kept low.
    async fn complete(&self, system: &str, user: &str) -> Result<String, Failure> {
        match self.ask(system, user, "none").await {
            Err(Refused::Request) => self.ask(system, user, "low").await,
            other => other,
        }
        .map_err(|refused| match refused {
            Refused::Request => Failure::Other("it answered 400".to_owned()),
            Refused::Failed(failure) => failure,
        })
    }

    async fn ask(&self, system: &str, user: &str, thinking: &str) -> Result<String, Refused> {
        let body = json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
            "temperature": 0.2,
            "max_tokens": MAX_ANSWER_TOKENS,
            "reasoning": { "effort": thinking },
            "provider": { "data_collection": "deny" },
        });
        let answer = self
            .http
            .post(format!("{}/chat/completions", self.api))
            .bearer_auth(self.key.expose())
            .json(&body)
            .send()
            .await
            .map_err(|e| Failure::Other(format!("no answer ({})", e.without_url())))?;
        if answer.status() == reqwest::StatusCode::BAD_REQUEST {
            return Err(Refused::Request);
        }
        status(answer.status())?;
        let bytes = read(answer)
            .await
            .map_err(|e| Failure::Other(e.to_string()))?;
        let completion: Completion = serde_json::from_slice(&bytes)
            .map_err(|_| Failure::Other("its answer wasn't a chat completion".to_owned()))?;
        Ok(completion.text()?)
    }
}

/// Why a request didn't get an answer: OpenRouter refused the request itself (as a model that
/// can't turn thinking off does), or anything else.
enum Refused {
    Request,
    Failed(Failure),
}

impl From<Failure> for Refused {
    fn from(failure: Failure) -> Self {
        Self::Failed(failure)
    }
}

/// The most tokens asked for: room for a model that still thinks a little, and the answer.
const MAX_ANSWER_TOKENS: u32 = 4000;

/// A chat completion as OpenRouter answers it: which model answered, and its first choice.
#[derive(Deserialize)]
struct Completion {
    #[serde(default)]
    model: Option<String>,
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

impl Completion {
    /// The answer's text, or what the model did instead: an empty answer says which model
    /// OpenRouter used and why it stopped, so a model cut off by the length limit says so.
    fn text(self) -> Result<String, Failure> {
        let model = self
            .model
            .map(|m| {
                m.chars()
                    .filter(|c| !c.is_control())
                    .take(100)
                    .collect::<String>()
            })
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "the model".to_owned());
        let Some(choice) = self.choices.into_iter().next() else {
            return Err(Failure::Other(format!("{model} gave no answer")));
        };
        match choice.message.content {
            Some(text) if !text.trim().is_empty() => Ok(text),
            _ => Err(Failure::Other(match choice.finish_reason.as_deref() {
                Some("length") => format!("{model} stopped at the length limit before answering"),
                Some(reason) => format!(
                    "{model} said nothing (it stopped: {})",
                    reason
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(40)
                        .collect::<String>()
                ),
                None => format!("{model} said nothing"),
            })),
        }
    }
}

/// OpenRouter's status as a failure, or none. Its error bodies aren't read: only the status
/// says what happened.
fn status(status: reqwest::StatusCode) -> Result<(), Failure> {
    match status.as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(Failure::Key),
        402 => Err(Failure::Credit),
        429 => Err(Failure::Busy),
        other => Err(Failure::Other(format!("it answered {other}"))),
    }
}

async fn read(answer: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    if answer
        .content_length()
        .is_some_and(|n| n > MAX_ANSWER as u64)
    {
        bail!("its answer is too big");
    }
    let bytes = answer
        .bytes()
        .await
        .map_err(|e| anyhow!("its answer was cut off ({})", e.without_url()))?;
    if bytes.len() > MAX_ANSWER {
        bail!("its answer is too big");
    }
    Ok(bytes.to_vec())
}

/// The titles in a model's answer: the JSON object in it (models sometimes wrap it in a code
/// fence or a sentence), each with a title of sensible length and a plausible year, at most
/// [`MAX_SUGGESTIONS`]. None when there's no such list.
fn parse_suggestions(answer: &str) -> Option<Vec<Suggestion>> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let value: Value = serde_json::from_str(answer.get(start..=end)?).ok()?;
    let titles = value["titles"].as_array()?;
    Some(
        titles
            .iter()
            .filter_map(|t| {
                let title = t["title"].as_str()?.trim();
                if title.is_empty() || title.chars().count() > 200 {
                    return None;
                }
                let year = t["year"]
                    .as_i64()
                    .or_else(|| t["year"].as_str()?.trim().parse().ok())
                    .filter(|y| (1870..=2100).contains(y));
                Some(Suggestion {
                    title: title.to_owned(),
                    year,
                })
            })
            .take(MAX_SUGGESTIONS)
            .collect(),
    )
}

/// OpenRouter on this machine for tests: answers chat completions with `content`, and keeps
/// what it was sent.
#[cfg(test)]
pub mod fake {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};

    use super::*;

    #[derive(Clone, Default)]
    pub struct Fake {
        /// What the model answers, and the status OpenRouter answers with.
        pub content: Arc<Mutex<String>>,
        pub status: Arc<Mutex<u16>>,
        /// Each request's body and headers.
        pub seen: Arc<Mutex<Vec<(Value, HeaderMap)>>>,
        /// How long the model takes to answer, in milliseconds.
        pub delay_ms: Arc<Mutex<u64>>,
        /// The whole answer in place of one made from `content`.
        pub completion: Arc<Mutex<Option<Value>>>,
        /// Refuse a request that turns thinking off (400), as a model that must think does.
        pub must_think: Arc<Mutex<bool>>,
    }

    pub async fn start() -> (Fake, String) {
        let fake = Fake::default();
        *fake.status.lock().unwrap() = 200;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/api/v1/chat/completions", post(complete))
            .route("/api/v1/auth/key", get(key))
            .with_state(fake.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (fake, api)
    }

    async fn complete(
        State(fake): State<Fake>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        fake.seen.lock().unwrap().push((body, headers));
        let delay = *fake.delay_ms.lock().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        let off = fake.seen.lock().unwrap().last().unwrap().0["reasoning"]["effort"] == "none";
        if off && *fake.must_think.lock().unwrap() {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "reasoning is mandatory" })),
            );
        }
        let status = StatusCode::from_u16(*fake.status.lock().unwrap()).unwrap();
        if let Some(whole) = fake.completion.lock().unwrap().clone() {
            return (status, Json(whole));
        }
        let content = fake.content.lock().unwrap().clone();
        (
            status,
            Json(
                json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] }),
            ),
        )
    }

    async fn key(State(fake): State<Fake>, headers: HeaderMap) -> StatusCode {
        fake.seen.lock().unwrap().push((Value::Null, headers));
        StatusCode::from_u16(*fake.status.lock().unwrap()).unwrap()
    }

    pub fn client(api: &str) -> Client {
        let settings = OpenRouter {
            key: Secret::for_tests("sk-or-test-key"),
            model: "openrouter/auto".into(),
        };
        Client::with_api(&settings, api, Arc::default()).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_the_description_and_instructions_go_out() {
        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() =
            r#"{"titles": [{"title": "Heat", "year": 1995}, {"title": "Ronin", "year": "1998"}]}"#
                .into();
        let client = fake::client(&api);
        let found = client
            .suggest(TitleKind::Movie, "a tense heist in Los Angeles")
            .await
            .unwrap();
        assert_eq!(
            found,
            [
                Suggestion {
                    title: "Heat".into(),
                    year: Some(1995)
                },
                Suggestion {
                    title: "Ronin".into(),
                    year: Some(1998)
                },
            ]
        );
        let seen = fake.seen.lock().unwrap();
        let (body, headers) = &seen[0];
        assert_eq!(headers["authorization"], "Bearer sk-or-test-key");
        assert_eq!(
            headers["user-agent"],
            concat!("Melete/", env!("CARGO_PKG_VERSION"))
        );
        for attribution in ["http-referer", "referer", "x-title"] {
            assert!(!headers.contains_key(attribution), "{attribution}");
        }
        assert_eq!(body["provider"]["data_collection"], "deny");
        assert_eq!(body["model"], "openrouter/auto");
        assert!(body.get("user").is_none(), "nobody is named");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1]["content"], "a tense heist in Los Angeles");
        assert!(messages[0]["content"].as_str().unwrap().contains("films"));
    }

    #[tokio::test]
    async fn openrouters_refusals_are_said_plainly_without_the_key() {
        let (fake, api) = fake::start().await;
        let client = fake::client(&api);
        for (status, failure) in [
            (401, Failure::Key),
            (402, Failure::Credit),
            (429, Failure::Busy),
        ] {
            *fake.status.lock().unwrap() = status;
            assert_eq!(client.check().await.unwrap_err(), failure);
            let err = client.suggest(TitleKind::Series, "x").await.unwrap_err();
            assert_eq!(err, failure);
            assert!(!err.to_string().contains("sk-or"), "{err}");
        }
        *fake.status.lock().unwrap() = 200;
        client.check().await.unwrap();
        *fake.content.lock().unwrap() = "Sorry, I can't help with that.".into();
        assert!(matches!(
            client.suggest(TitleKind::Movie, "x").await,
            Err(Failure::Other(_))
        ));
    }

    #[test]
    fn suggestions_are_read_from_whatever_the_model_wraps_them_in() {
        let fenced = "Here you go:\n```json\n{\"titles\": [{\"title\": \" Alien \", \"year\": 1979}, {\"title\": \"\"}, {\"title\": \"Odd\", \"year\": 3000}, {\"year\": 1990}]}\n```";
        assert_eq!(
            parse_suggestions(fenced).unwrap(),
            [
                Suggestion {
                    title: "Alien".into(),
                    year: Some(1979)
                },
                Suggestion {
                    title: "Odd".into(),
                    year: None
                },
            ]
        );
        let many = format!(
            "{{\"titles\": [{}]}}",
            vec![r#"{"title": "X"}"#; 50].join(",")
        );
        assert_eq!(parse_suggestions(&many).unwrap().len(), MAX_SUGGESTIONS);
        assert_eq!(parse_suggestions(r#"{"titles": []}"#).unwrap(), []);
        assert!(parse_suggestions("no json here").is_none());
        assert!(parse_suggestions(r#"{"films": []}"#).is_none());
    }

    #[tokio::test]
    async fn thinking_is_asked_off_with_room_left_for_the_answer() {
        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() = r#"{"titles": []}"#.into();
        fake::client(&api)
            .suggest(TitleKind::Movie, "x")
            .await
            .unwrap();
        let body = fake.seen.lock().unwrap()[0].0.clone();
        assert_eq!(body["reasoning"], json!({ "effort": "none" }));
        assert!(body["max_tokens"].as_u64().unwrap() >= 4000);
    }

    #[tokio::test]
    async fn a_model_that_must_think_is_asked_again_with_thinking_kept_low() {
        let (fake, api) = fake::start().await;
        *fake.must_think.lock().unwrap() = true;
        *fake.content.lock().unwrap() = r#"{"titles": [{"title": "Heat", "year": 1995}]}"#.into();
        let found = fake::client(&api)
            .suggest(TitleKind::Movie, "x")
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        let efforts: Vec<Value> = fake
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|(body, _)| body["reasoning"]["effort"].clone())
            .collect();
        assert_eq!(efforts, [json!("none"), json!("low")]);
    }

    #[tokio::test]
    async fn an_empty_answer_says_which_model_answered_and_why_it_stopped() {
        let (fake, api) = fake::start().await;
        let client = fake::client(&api);
        let empty = |content: Value, finish: Value| {
            json!({ "model": "z-ai/glm-5.3-flash", "choices": [{
                "message": { "role": "assistant", "content": content },
                "finish_reason": finish }] })
        };
        for (answer, said) in [
            (
                empty(json!(""), json!("length")),
                "z-ai/glm-5.3-flash stopped at the length limit before answering",
            ),
            (
                empty(Value::Null, json!("content_filter")),
                "z-ai/glm-5.3-flash said nothing (it stopped: content_filter)",
            ),
            (
                empty(json!("  "), Value::Null),
                "z-ai/glm-5.3-flash said nothing",
            ),
            (json!({ "choices": [] }), "the model gave no answer"),
        ] {
            *fake.completion.lock().unwrap() = Some(answer);
            let err = client.suggest(TitleKind::Movie, "x").await.unwrap_err();
            assert_eq!(err, Failure::Other(said.to_owned()));
        }
    }
}
