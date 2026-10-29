//! TMDB API client: the only code that talks to themoviedb.org.
//!
//! Authenticates with the API Read Access Token as a bearer header, so the secret never
//! appears in URLs or in the errors that quote them.

use std::fmt;
use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::{StatusCode, header};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep, sleep_until};

mod credits;
mod images;
pub mod language;
mod ratings;
mod types;
mod videos;

pub use credits::*;
pub use images::*;
use language::ENGLISH;
pub use types::*;
pub use videos::*;

const API_BASE: &str = "https://api.themoviedb.org/3";
/// TMDB allows around 50 requests a second; stay well under.
const MIN_INTERVAL: Duration = Duration::from_millis(50);
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const MAX_ATTEMPTS: u32 = 4;
/// TMDB's limit on `append_to_response` entries per request.
const MAX_APPENDS: usize = 20;
/// Longest `Retry-After` worth waiting out; anything longer stops the run.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
/// Search pages read when there is no year to narrow the search, so a same-titled remake
/// further down still makes the match ambiguous.
const MAX_SEARCH_PAGES: i64 = 3;

/// TMDB rejected the token. Retrying cannot help, so callers stop instead.
#[derive(Debug)]
pub struct Unauthorized;

impl fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "TMDB rejected the token (401); check MELETE_TMDB_TOKEN is the API Read Access Token",
        )
    }
}

impl std::error::Error for Unauthorized {}

/// TMDB asked for a longer pause than a run should sit through.
#[derive(Debug)]
pub struct RateLimited(pub Duration);

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TMDB asked to wait {}s before the next request",
            self.0.as_secs()
        )
    }
}

impl std::error::Error for RateLimited {}

/// Treats an explicit `null` like a missing field; `#[serde(default)]` alone only covers missing.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A client for TMDB's API. Requests go out at least 50 ms apart, give up after 15 seconds,
/// and are retried (up to four attempts) on rate limits, server errors and timeouts.
///
/// Every request fails with [`Unauthorized`] when TMDB rejects the token, with
/// [`RateLimited`] when it asks for a pause of more than a minute, and with a plain error on
/// any other failure.
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
    min_interval: Duration,
    backoff_base: Duration,
    next_slot: Mutex<Instant>,
    /// What details, seasons and images are asked for in.
    language: String,
}

impl Client {
    /// A client for TMDB itself, with the API Read Access Token. Nothing is sent until a request.
    pub fn new(token: String) -> anyhow::Result<Self> {
        Self::with_base(API_BASE.into(), token, MIN_INTERVAL, BACKOFF_BASE)
    }

    fn with_base(
        base: String,
        token: String,
        min_interval: Duration,
        backoff_base: Duration,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base,
            token,
            min_interval,
            backoff_base,
            next_slot: Mutex::new(Instant::now()),
            language: ENGLISH.into(),
        })
    }

    /// Movies called `query`, released in `year` when given. Without a year the first three
    /// pages are read, so a same-titled remake further down still shows up.
    pub async fn search_movies(
        &self,
        query: &str,
        year: Option<i64>,
    ) -> anyhow::Result<Vec<MovieResult>> {
        let year = year.map(|y| y.to_string());
        let mut params = vec![("query", query), ("include_adult", "false")];
        if let Some(year) = &year {
            params.push(("year", year));
        }
        self.search("/search/movie", params, year.is_none()).await
    }

    /// Series called `query`, first aired in `year` when given. Without a year the first three
    /// pages are read, so a same-titled remake further down still shows up.
    pub async fn search_series(
        &self,
        query: &str,
        year: Option<i64>,
    ) -> anyhow::Result<Vec<SeriesResult>> {
        let year = year.map(|y| y.to_string());
        let mut params = vec![("query", query), ("include_adult", "false")];
        if let Some(year) = &year {
            params.push(("first_air_date_year", year));
        }
        self.search("/search/tv", params, year.is_none()).await
    }

    /// Whether TMDB takes the token: a connection test.
    pub async fn check(&self) -> anyhow::Result<()> {
        let _: serde_json::Value = self.get_found("/authentication", &[]).await?;
        Ok(())
    }

    /// The first page of movies called `query`, for someone choosing one to ask for.
    pub async fn search_movies_page(&self, query: &str) -> anyhow::Result<Vec<MovieResult>> {
        let params = vec![("query", query), ("include_adult", "false")];
        self.search("/search/movie", params, false).await
    }

    /// The first page of series called `query`, for someone choosing one to ask for.
    pub async fn search_series_page(&self, query: &str) -> anyhow::Result<Vec<SeriesResult>> {
        let params = vec![("query", query), ("include_adult", "false")];
        self.search("/search/tv", params, false).await
    }

    /// The movies and series TMDB knows by an IMDb or TVDB id.
    ///
    /// # Errors
    ///
    /// Refuses an IMDb id that isn't `tt` followed by digits, without asking TMDB.
    pub async fn find(&self, id: ExternalId<'_>) -> anyhow::Result<FindResults> {
        // The id goes into the request path, so it must not be able to change it.
        let (path, source) = match id {
            ExternalId::Imdb(imdb)
                if imdb
                    .strip_prefix("tt")
                    .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())) =>
            {
                (format!("/find/{imdb}"), "imdb_id")
            }
            ExternalId::Imdb(imdb) => bail!("{imdb:?} is not an IMDb id"),
            ExternalId::Tvdb(tvdb) => (format!("/find/{tvdb}"), "tvdb_id"),
        };
        self.get_found(&path, &[("external_source", source)]).await
    }

    /// A movie's earliest digital and physical release in any country; `None` when TMDB has
    /// no movie with this id.
    pub async fn release_dates(&self, id: i64) -> anyhow::Result<Option<ReleaseDates>> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(default, deserialize_with = "null_as_default")]
            results: Vec<Country>,
        }
        #[derive(Deserialize)]
        struct Country {
            #[serde(default, deserialize_with = "null_as_default")]
            release_dates: Vec<Dated>,
        }
        #[derive(Deserialize)]
        struct Dated {
            release_date: Option<String>,
            #[serde(rename = "type")]
            kind: Option<i64>,
        }
        let Some(reply) = self
            .get::<Reply>(&format!("/movie/{id}/release_dates"), &[])
            .await?
        else {
            return Ok(None);
        };
        // TMDB's release types: 4 is digital, 5 physical.
        let earliest = |kind: i64| {
            reply
                .results
                .iter()
                .flat_map(|c| &c.release_dates)
                .filter(|d| d.kind == Some(kind))
                .filter_map(|d| d.release_date.as_deref()?.get(..10))
                .filter(|day| {
                    let b = day.as_bytes();
                    b.len() == 10
                        && b[4] == b'-'
                        && b[7] == b'-'
                        && b.iter()
                            .enumerate()
                            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
                })
                .min()
                .map(str::to_owned)
        };
        Ok(Some(ReleaseDates {
            digital: earliest(4),
            physical: earliest(5),
        }))
    }

    /// One page of a TMDB chart or list: `path` is "/movie/popular", "/trending/tv/week",
    /// "/list/8123"…; lists give `items`, charts `results`. `None` when there's no such list.
    pub async fn list_page(&self, path: &str, page: u32) -> anyhow::Result<Option<ListPage>> {
        let page = page.to_string();
        self.get(path, &[("page", &page)]).await
    }

    /// What's trending among movies (or with `tv`, series) today, or with `week` this week:
    /// the chart's first page. Only which chart leaves the server.
    pub async fn trending(&self, tv: bool, week: bool) -> anyhow::Result<Vec<Listed>> {
        let kind = if tv { "tv" } else { "movie" };
        let window = if week { "week" } else { "day" };
        let page: ListedPage = self
            .get_found(&format!("/trending/{kind}/{window}"), &[])
            .await?;
        Ok(page.results)
    }

    /// Titles TMDB recommends with a movie (or with `tv`, a series), its first page; `None`
    /// when TMDB has no such title. Only the title's id leaves the server.
    pub async fn recommendations(&self, tv: bool, id: i64) -> anyhow::Result<Option<Vec<Listed>>> {
        let kind = if tv { "tv" } else { "movie" };
        let page: Option<ListedPage> = self
            .get(&format!("/{kind}/{id}/recommendations"), &[])
            .await?;
        Ok(page.map(|p| p.results))
    }

    /// The first page of movies (or with `tv`, series) called `query`, with their posters.
    pub async fn search_listed(&self, tv: bool, query: &str) -> anyhow::Result<Vec<Listed>> {
        let path = if tv { "/search/tv" } else { "/search/movie" };
        let page: ListedPage = self
            .get_found(path, &[("query", query), ("include_adult", "false")])
            .await?;
        Ok(page.results)
    }

    /// Runs a search, reading further pages (up to [`MAX_SEARCH_PAGES`]) when `all_pages`.
    async fn search<T: DeserializeOwned>(
        &self,
        path: &str,
        mut params: Vec<(&str, &str)>,
        all_pages: bool,
    ) -> anyhow::Result<Vec<T>> {
        let first: Page<T> = self.get_found(path, &params).await?;
        let mut results = first.results;
        if all_pages {
            let last = first.total_pages.unwrap_or(1).min(MAX_SEARCH_PAGES);
            let numbers: Vec<String> = (2..=last).map(|n| n.to_string()).collect();
            for number in &numbers {
                params.push(("page", number));
                let page: Page<T> = self.get_found(path, &params).await?;
                params.pop();
                results.extend(page.results);
            }
        }
        Ok(results)
    }

    /// Like [`Self::get`], for endpoints that always exist (search, find): a 404 there is a
    /// failure, not an empty answer, or an outage would mark titles unmatched.
    async fn get_found<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> anyhow::Result<T> {
        self.get(path, params)
            .await?
            .with_context(|| format!("TMDB answered 404 Not Found for {path}"))
    }

    /// GETs a path in English, retrying rate limits, server errors and timeouts. `None` on 404.
    async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> anyhow::Result<Option<T>> {
        self.get_in(ENGLISH, path, params).await
    }

    /// Like [`Self::get`], in `language`.
    async fn get_in<T: DeserializeOwned>(
        &self,
        language: &str,
        path: &str,
        params: &[(&str, &str)],
    ) -> anyhow::Result<Option<T>> {
        let url = format!("{}{path}", self.base);
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.wait_turn().await;
            let response = self
                .http
                .get(&url)
                .bearer_auth(&self.token)
                .query(&[("language", language)])
                .query(params)
                .send()
                .await;
            let delay = match response {
                Ok(response) => match response.status() {
                    status if status.is_success() => {
                        let body = response
                            .json()
                            .await
                            .with_context(|| format!("decoding TMDB response for {path}"))?;
                        return Ok(Some(body));
                    }
                    StatusCode::NOT_FOUND => return Ok(None),
                    StatusCode::UNAUTHORIZED => return Err(Unauthorized.into()),
                    StatusCode::TOO_MANY_REQUESTS => match retry_after(&response) {
                        Some(wait) if wait > MAX_RETRY_AFTER => {
                            return Err(RateLimited(wait).into());
                        }
                        Some(wait) => wait,
                        None => self.backoff(attempt),
                    },
                    status if status.is_server_error() => self.backoff(attempt),
                    status => bail!("TMDB answered {status} for {path}"),
                },
                Err(err) if err.is_timeout() || err.is_connect() => {
                    if attempt >= MAX_ATTEMPTS {
                        return Err(err).with_context(|| format!("requesting {path} from TMDB"));
                    }
                    self.backoff(attempt)
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("requesting {path} from TMDB"));
                }
            };
            if attempt >= MAX_ATTEMPTS {
                bail!("TMDB still failing for {path} after {MAX_ATTEMPTS} attempts");
            }
            sleep(delay).await;
        }
    }

    /// Spaces requests at least `min_interval` apart.
    async fn wait_turn(&self) {
        let mut next = self.next_slot.lock().await;
        sleep_until(*next).await;
        *next = Instant::now() + self.min_interval;
    }

    fn backoff(&self, attempt: u32) -> Duration {
        self.backoff_base * 2u32.pow(attempt - 1)
    }
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let seconds = response
        .headers()
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
pub mod testing {
    //! A local stand-in for the TMDB API, serving canned responses.

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    use axum::extract::{Query, Request, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use serde_json::Value;

    use super::*;

    /// One canned reply: the status, an optional `Retry-After`, and the body.
    #[derive(Clone)]
    pub struct Reply {
        pub status: StatusCode,
        pub retry_after: Option<u64>,
        pub body: Value,
    }

    impl Reply {
        pub fn ok(body: Value) -> Self {
            Self {
                status: StatusCode::OK,
                retry_after: None,
                body,
            }
        }

        pub fn status(status: StatusCode) -> Self {
            Self {
                status,
                retry_after: None,
                body: serde_json::json!({}),
            }
        }
    }

    /// Requests are matched on path plus sorted query string (without `language`).
    /// A key may hold several replies, served in order; the last one repeats.
    #[derive(Clone, Default)]
    pub struct FakeTmdb {
        replies: Arc<StdMutex<HashMap<String, Vec<Reply>>>>,
        pub log: Arc<StdMutex<Vec<String>>>,
        /// Languages besides English that requests may carry; others are refused.
        languages: Arc<StdMutex<Vec<String>>>,
    }

    impl FakeTmdb {
        /// Takes requests in `language` too, whose keys (as replies are set and requests logged)
        /// end in `#` and the language, as in `/movie/1#fr-FR`; English ones' keys don't.
        pub fn language(&self, language: &str) -> &Self {
            self.languages.lock().unwrap().push(language.into());
            self
        }

        pub fn reply(&self, key: &str, replies: Vec<Reply>) -> &Self {
            self.replies.lock().unwrap().insert(key.into(), replies);
            self
        }

        /// Starts the server and returns a client pointed at it.
        pub async fn start(&self) -> Client {
            let app = axum::Router::new()
                .fallback(handle)
                .with_state(self.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Client::with_base(
                format!("http://{addr}"),
                "test-token".into(),
                Duration::ZERO,
                Duration::ZERO,
            )
            .unwrap()
        }
    }

    async fn handle(
        State(fake): State<FakeTmdb>,
        Query(mut query): Query<Vec<(String, String)>>,
        headers: HeaderMap,
        request: Request,
    ) -> Response {
        let language = query
            .iter()
            .find(|(k, _)| k == "language")
            .map(|(_, v)| v.clone());
        let other = language
            .as_ref()
            .filter(|l| *l != "en-US" && fake.languages.lock().unwrap().contains(l));
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer test-token")
            || !(language.as_deref() == Some("en-US") || other.is_some())
        {
            return StatusCode::BAD_REQUEST.into_response();
        }
        query.retain(|(k, _)| k != "language");
        query.sort();
        let mut key = request.uri().path().to_string();
        if !query.is_empty() {
            let pairs: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            key = format!("{key}?{}", pairs.join("&"));
        }
        if let Some(other) = other {
            key = format!("{key}#{other}");
        }
        fake.log.lock().unwrap().push(key.clone());

        let reply = {
            let mut replies = fake.replies.lock().unwrap();
            match replies.get_mut(&key) {
                Some(queue) if queue.len() > 1 => queue.remove(0),
                Some(queue) => queue[0].clone(),
                None => Reply::status(StatusCode::NOT_FOUND),
            }
        };
        let mut response = (reply.status, axum::Json(reply.body)).into_response();
        if let Some(seconds) = reply.retry_after {
            response
                .headers_mut()
                .insert("retry-after", seconds.to_string().parse().unwrap());
        }
        response
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_the_best_voted_textless_backdrop_and_logo() {
        let list: ImageList = serde_json::from_value(serde_json::json!({
            "backdrops": [
                { "file_path": "/text.jpg", "iso_639_1": "en", "vote_average": 9.0, "vote_count": 9 },
                { "file_path": "/plain-b.jpg", "iso_639_1": null, "vote_average": 5.5, "vote_count": 3 },
                { "file_path": "/plain-a.jpg", "iso_639_1": null, "vote_average": 5.5, "vote_count": 3 },
                { "file_path": "/plain-c.jpg", "iso_639_1": null, "vote_average": 5.5, "vote_count": 2 },
                { "file_path": "/plain.svg", "iso_639_1": null, "vote_average": 10.0, "vote_count": 9 }
            ],
            "logos": [
                { "file_path": "/none.png", "iso_639_1": null, "vote_average": 8.0, "vote_count": 1 },
                { "file_path": "/en.svg", "iso_639_1": "en", "vote_average": 9.0, "vote_count": 1 },
                { "file_path": "/en.png", "iso_639_1": "en", "vote_average": 4.0, "vote_count": null }
            ]
        }))
        .unwrap();
        assert_eq!(list.textless_backdrop(), Some("/plain-a.jpg"));
        assert_eq!(list.logo(), Some("/en.png"), "English first, PNGs only");
        let list: ImageList = serde_json::from_value(serde_json::json!({
            "backdrops": null,
            "logos": [{ "file_path": "/none.png", "iso_639_1": null }]
        }))
        .unwrap();
        assert_eq!(
            (list.textless_backdrop(), list.logo()),
            (None, Some("/none.png"))
        );
    }

    use super::testing::{FakeTmdb, Reply};
    use super::*;
    use axum::http::StatusCode as Status;
    use serde_json::json;

    #[tokio::test]
    async fn searches_with_token_language_and_year() {
        let fake = FakeTmdb::default();
        fake.reply(
            "/search/movie?include_adult=false&query=The Matrix&year=1999",
            vec![Reply::ok(json!({ "page": 1, "results": [
                { "id": 603, "title": "The Matrix", "original_title": "The Matrix", "release_date": "1999-03-31" }
            ]}))],
        );
        let client = fake.start().await;
        let results = client
            .search_movies("The Matrix", Some(1999))
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, 603);
    }

    #[tokio::test]
    async fn search_without_year_reads_further_pages() {
        let fake = FakeTmdb::default();
        let page = |id: i64| {
            Reply::ok(json!({ "total_pages": 5, "results": [
                { "id": id, "title": "Dune", "release_date": "2021-09-15" }
            ]}))
        };
        fake.reply(
            "/search/movie?include_adult=false&query=Dune",
            vec![page(1)],
        )
        .reply(
            "/search/movie?include_adult=false&page=2&query=Dune",
            vec![page(2)],
        )
        .reply(
            "/search/movie?include_adult=false&page=3&query=Dune",
            vec![page(3)],
        )
        .reply(
            "/search/movie?include_adult=false&query=Dune&year=2021",
            vec![page(1)],
        );
        let client = fake.start().await;
        let ids: Vec<i64> = client
            .search_movies("Dune", None)
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec![1, 2, 3], "stops at three pages");
        assert_eq!(
            client
                .search_movies("Dune", Some(2021))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fake.log.lock().unwrap().len(),
            4,
            "a year search reads one page"
        );
    }

    #[tokio::test]
    async fn missing_movie_is_none() {
        let client = FakeTmdb::default().start().await;
        assert!(client.movie(1).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn checks_the_token() {
        let fake = FakeTmdb::default();
        fake.reply("/authentication", vec![Reply::ok(json!({"success": true}))]);
        assert!(fake.start().await.check().await.is_ok());
        let refused = FakeTmdb::default();
        refused.reply("/authentication", vec![Reply::status(Status::UNAUTHORIZED)]);
        assert!(
            refused
                .start()
                .await
                .check()
                .await
                .unwrap_err()
                .is::<Unauthorized>()
        );
    }

    #[tokio::test]
    async fn bad_token_is_unauthorized() {
        let fake = FakeTmdb::default();
        fake.reply("/movie/1", vec![Reply::status(Status::UNAUTHORIZED)]);
        let err = fake.start().await.movie(1).await.unwrap_err();
        assert!(err.is::<Unauthorized>());
    }

    #[tokio::test]
    async fn retries_rate_limit_then_succeeds() {
        let fake = FakeTmdb::default();
        let limited = Reply {
            retry_after: Some(0),
            ..Reply::status(Status::TOO_MANY_REQUESTS)
        };
        fake.reply(
            "/movie/603",
            vec![
                limited,
                Reply::ok(json!({ "id": 603, "title": "The Matrix" })),
            ],
        );
        let client = fake.start().await;
        let movie = client.movie(603).await.unwrap().unwrap();
        assert_eq!(movie.title, "The Matrix");
        assert_eq!(fake.log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn search_404_is_an_error_not_an_empty_result() {
        let client = FakeTmdb::default().start().await;
        let err = client.search_movies("Dune", Some(2021)).await.unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");
        assert!(client.find(ExternalId::Tvdb(1)).await.is_err());
    }

    #[tokio::test]
    async fn long_retry_after_stops_instead_of_waiting() {
        let fake = FakeTmdb::default();
        let limited = Reply {
            retry_after: Some(3600),
            ..Reply::status(Status::TOO_MANY_REQUESTS)
        };
        fake.reply("/movie/603", vec![limited]);
        let client = fake.start().await;
        let err = client.movie(603).await.unwrap_err();
        assert!(err.is::<RateLimited>(), "{err}");
        assert_eq!(fake.log.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn null_lists_decode_as_empty() {
        let fake = FakeTmdb::default();
        fake.reply(
            "/tv/1?append_to_response=external_ids",
            vec![Reply::ok(
                json!({ "id": 1, "name": "Show", "genres": null, "seasons": null }),
            )],
        )
        .reply(
            "/search/movie?include_adult=false&query=X",
            vec![Reply::ok(json!({ "results": null }))],
        );
        let client = fake.start().await;
        let series = client.series(1).await.unwrap().unwrap();
        assert!(series.genres.is_empty() && series.seasons.is_empty());
        assert!(client.search_movies("X", None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn missing_season_in_reply_is_an_error() {
        let fake = FakeTmdb::default();
        fake.reply(
            "/tv/1?append_to_response=season/1,season/2",
            vec![Reply::ok(
                json!({ "id": 1, "name": "Show", "season/1": { "episodes": [] } }),
            )],
        );
        let client = fake.start().await;
        let err = client.seasons(1, &[1, 2]).await.unwrap_err();
        assert!(err.to_string().contains("season 2"), "{err}");
    }

    #[tokio::test]
    async fn gives_up_after_repeated_server_errors() {
        let fake = FakeTmdb::default();
        fake.reply("/movie/603", vec![Reply::status(Status::BAD_GATEWAY)]);
        let client = fake.start().await;
        let err = client.movie(603).await.unwrap_err();
        assert!(err.to_string().contains("after 4 attempts"), "{err}");
        assert_eq!(fake.log.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn fetches_seasons_in_appended_chunks() {
        let fake = FakeTmdb::default();
        let numbers: Vec<i64> = (0..=21).collect();
        let first: Vec<String> = numbers[..20]
            .iter()
            .map(|n| format!("season/{n}"))
            .collect();
        let mut body = json!({ "id": 1, "name": "Long Show" });
        for n in &numbers[..20] {
            body[format!("season/{n}")] = json!({ "episodes": [
                { "id": 100 + n, "season_number": n, "episode_number": 1 }
            ]});
        }
        fake.reply(
            &format!("/tv/1?append_to_response={}", first.join(",")),
            vec![Reply::ok(body)],
        );
        fake.reply(
            "/tv/1?append_to_response=season/20,season/21",
            vec![Reply::ok(json!({ "id": 1, "name": "Long Show",
                "season/20": { "episodes": [{ "id": 120, "season_number": 20, "episode_number": 1 }] },
                "season/21": { "episodes": [] }
            }))],
        );
        let client = fake.start().await;
        let seasons = client.seasons(1, &numbers).await.unwrap();
        assert_eq!(seasons.len(), 22);
        assert_eq!(seasons[20].episodes[0].id, 120);
        assert_eq!(fake.log.lock().unwrap().len(), 2);
    }

    /// Serves one raw HTTP response, then closes: lets a test send a body with no length.
    async fn raw_server(response: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn images_refuse_non_images() {
        let base = raw_server(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: 5\r\n\r\noops!"
                .to_vec(),
        )
        .await;
        let err = Images::with_base(base)
            .unwrap()
            .fetch("w342", "a.jpg")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("other than an image"), "{err}");
    }

    #[tokio::test]
    async fn images_stop_reading_past_the_limit_without_a_length() {
        let mut response =
            b"HTTP/1.1 200 OK\r\ncontent-type: image/jpeg\r\nconnection: close\r\n\r\n".to_vec();
        response.extend(std::iter::repeat_n(b'x', 5000));
        let mut images = Images::with_base(raw_server(response).await).unwrap();
        images.max_bytes = 1000;
        let err = images.fetch("w342", "a.jpg").await.unwrap_err();
        assert!(err.to_string().contains("size limit"), "{err}");
    }

    #[tokio::test]
    async fn find_refuses_malformed_imdb_ids() {
        let fake = FakeTmdb::default();
        let client = fake.start().await;
        for id in ["../movie/1", "tt", "nm123", "tt12a"] {
            assert!(client.find(ExternalId::Imdb(id)).await.is_err(), "{id}");
        }
        assert!(fake.log.lock().unwrap().is_empty(), "nothing was sent");
    }
}
