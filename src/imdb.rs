//! IMDb charts and public lists, for import lists. IMDb has no public API and its pages
//! want a browser, so this asks the GraphQL endpoint IMDb's own site uses. It's undocumented
//! and may change; IMDb allows limited non-commercial use, which a household's server is. Only
//! titles' IMDb ids are read; TMDB says what they are.

use std::time::Duration;

use serde_json::{Value, json};

const BASE: &str = "https://caching.graphql.imdb.com/";
/// The most titles asked for at once; charts are 250 long at most.
pub const MAX_TITLES: u32 = 250;

/// One of IMDb's charts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chart {
    TopMovies,
    PopularMovies,
    TopShows,
    PopularShows,
}

impl Chart {
    fn name(self) -> &'static str {
        match self {
            Chart::TopMovies => "TOP_RATED_MOVIES",
            Chart::PopularMovies => "MOST_POPULAR_MOVIES",
            Chart::TopShows => "TOP_RATED_TV_SHOWS",
            Chart::PopularShows => "MOST_POPULAR_TV_SHOWS",
        }
    }
}

/// A client for IMDb's GraphQL endpoint. Requests give up after 30 seconds and don't follow
/// redirects; only a chart's name or a list's id is sent.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    /// A client for IMDb itself. Nothing is sent until a request.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_base(BASE.into())
    }

    /// A client that sends its requests to `base` instead of IMDb, for tests.
    pub fn with_base(base: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, base })
    }

    /// The IMDb ids on a chart, in its order.
    pub async fn chart(&self, chart: Chart, first: u32) -> anyhow::Result<Vec<String>> {
        let query = format!(
            "query {{ chartTitles(first: {}, chart: {{chartType: {}}}) {{ edges {{ node {{ id }} }} }} }}",
            first.min(MAX_TITLES),
            chart.name()
        );
        let data = self.ask(&query).await?;
        ids(&data["chartTitles"]["edges"], "node")
    }

    /// The IMDb ids on a public list ("ls" and digits), in its order.
    pub async fn list(&self, id: &str, first: u32) -> anyhow::Result<Vec<String>> {
        // The id goes into the query, so only its plain form is accepted.
        anyhow::ensure!(is_list_id(id), "{id:?} isn't an IMDb list id");
        let query = format!(
            "query {{ list(id: \"{id}\") {{ titleListItemSearch(first: {}) {{ edges {{ listItem: title {{ id }} }} }} }} }}",
            first.min(MAX_TITLES)
        );
        let data = self.ask(&query).await?;
        ids(&data["list"]["titleListItemSearch"]["edges"], "listItem")
    }

    async fn ask(&self, query: &str) -> anyhow::Result<Value> {
        let response = self
            .http
            .post(&self.base)
            .header("x-imdb-client-name", "imdb-web-next")
            .json(&json!({ "query": query }))
            .send()
            .await
            .map_err(|err| anyhow::anyhow!("IMDb: {}", err.without_url()))?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "IMDb answered {status}");
        let body: Value = response
            .json()
            .await
            .map_err(|err| anyhow::anyhow!("IMDb's answer: {}", err.without_url()))?;
        if let Some(error) = body["errors"][0]["message"].as_str() {
            let not_found = error.contains("NOT_FOUND");
            anyhow::bail!(if not_found {
                "IMDb has no such list (or it's private)".to_owned()
            } else {
                format!("IMDb: {}", error.chars().take(200).collect::<String>())
            });
        }
        Ok(body["data"].clone())
    }
}

/// Whether `id` is a public list's id: "ls" and 6 to 12 digits.
pub fn is_list_id(id: &str) -> bool {
    id.strip_prefix("ls")
        .is_some_and(|d| (6..=12).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit()))
}

/// "tt" ids from a list of edges; anything else is a sign the answer changed shape.
fn ids(edges: &Value, field: &str) -> anyhow::Result<Vec<String>> {
    let edges = edges
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("IMDb's answer isn't the shape Melete knows"))?;
    edges
        .iter()
        .map(|e| {
            let id = e[field]["id"].as_str().unwrap_or_default();
            let plain = id
                .strip_prefix("tt")
                .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
            anyhow::ensure!(plain, "IMDb gave {id:?} as a title id");
            Ok(id.to_owned())
        })
        .collect()
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::State;
    use serde_json::{Value, json};

    /// The fake's query log and the ids it answers with.
    type Fake = (Arc<Mutex<Vec<String>>>, Vec<&'static str>);

    /// A fake endpoint: charts and the list "ls0123456" answer with `ids`; other lists aren't
    /// found. Queries are logged.
    pub async fn start(ids: Vec<&'static str>) -> (super::Client, Arc<Mutex<Vec<String>>>) {
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let state = (log.clone(), ids);
        let router = axum::Router::new()
            .route(
                "/",
                axum::routing::post(
                    |State((log, ids)): State<Fake>,
                     Json(body): Json<Value>| async move {
                        let query = body["query"].as_str().unwrap_or_default().to_owned();
                        log.lock().unwrap().push(query.clone());
                        if query.contains("list(id:") && !query.contains("ls0123456") {
                            return Json(json!({ "errors": [{ "message": "RESOURCE_NOT_FOUND exception" }],
                                                "data": { "list": null } }));
                        }
                        let edges = |field: &str| -> Vec<Value> {
                            ids.iter().map(|id| json!({ field: { "id": id } })).collect()
                        };
                        Json(if query.contains("chartTitles") {
                            json!({ "data": { "chartTitles": { "edges": edges("node") } } })
                        } else {
                            json!({ "data": { "list": { "titleListItemSearch": { "edges": edges("listItem") } } } })
                        })
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (super::Client::with_base(url).unwrap(), log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_charts_and_lists() {
        let (client, log) = testing::start(vec!["tt0111161", "tt0068646"]).await;
        assert_eq!(
            client.chart(Chart::TopMovies, 2).await.unwrap(),
            ["tt0111161", "tt0068646"]
        );
        assert!(log.lock().unwrap()[0].contains("first: 2, chart: {chartType: TOP_RATED_MOVIES}"));
        assert_eq!(client.list("ls0123456", 9).await.unwrap().len(), 2);
        let missing = client.list("ls0000000", 9).await.unwrap_err();
        assert!(missing.to_string().contains("no such list"));
        assert!(
            client.list("ls1\") { x", 9).await.is_err(),
            "only plain ids go into a query"
        );
        assert!(is_list_id("ls055592025") && !is_list_id("ls12") && !is_list_id("tt0111161"));
    }

    #[tokio::test]
    async fn an_odd_answer_is_an_error() {
        let (client, _) = testing::start(vec!["nm0000151"]).await;
        assert!(client.chart(Chart::PopularShows, 5).await.is_err());
    }
}
