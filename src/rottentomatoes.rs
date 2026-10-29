//! Rotten Tomatoes browse pages, for import lists. There's no API: a browse page
//! ("https://www.rottentomatoes.com/browse/movies_at_home/sort:popular") describes its titles
//! for search engines in a JSON-LD ItemList, with each title's name and year. That's all that's
//! read, and TMDB's search says which title each is.

use std::time::Duration;

use serde_json::Value;

/// A title on a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The title as the page names it.
    pub title: String,
    /// The year it came out or started; None when the page doesn't say.
    pub year: Option<i64>,
    /// Whether it's a series rather than a movie.
    pub series: bool,
}

const HOST: &str = "www.rottentomatoes.com";
/// The largest page read.
const MAX_BYTES: usize = 5 * 1024 * 1024;

/// Whether a URL is a browse page this reads: Rotten Tomatoes' own, nothing else.
pub fn is_browse_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some(HOST)
            && u.port().is_none()
            && u.path().starts_with("/browse/")
            && u.username().is_empty()
    })
}

/// A client for Rotten Tomatoes' browse pages. Requests give up after 30 seconds and don't
/// follow redirects.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    /// Where requests for HOST go instead, in tests.
    base: Option<String>,
}

impl Client {
    /// A client for Rotten Tomatoes itself. Nothing is sent until a request.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_base(None)
    }

    /// A client that, given `base`, sends its requests there instead of Rotten Tomatoes, for
    /// tests; None reaches Rotten Tomatoes itself.
    pub fn with_base(base: Option<String>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, base })
    }

    /// The titles a browse page lists, in its order.
    pub async fn browse(&self, url: &str) -> anyhow::Result<Vec<Listed>> {
        anyhow::ensure!(
            is_browse_url(url),
            "only https://www.rottentomatoes.com/browse/... pages can be read"
        );
        let target = match &self.base {
            Some(base) => {
                let parsed = reqwest::Url::parse(url)?;
                format!("{base}{}", &url[url.find(parsed.path()).unwrap_or(0)..])
            }
            None => url.to_owned(),
        };
        let mut response = self
            .http
            .get(&target)
            .send()
            .await
            .map_err(|err| anyhow::anyhow!("Rotten Tomatoes: {}", err.without_url()))?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "Rotten Tomatoes answered {status}");
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| anyhow::anyhow!("Rotten Tomatoes: {}", err.without_url()))?
        {
            anyhow::ensure!(
                body.len() + chunk.len() <= MAX_BYTES,
                "the page is too large"
            );
            body.extend_from_slice(&chunk);
        }
        let titles = parse(&String::from_utf8_lossy(&body));
        anyhow::ensure!(
            !titles.is_empty(),
            "the page lists no titles Melete can read (has it changed?)"
        );
        Ok(titles)
    }
}

/// The titles in a page's JSON-LD ItemList.
pub fn parse(html: &str) -> Vec<Listed> {
    let mut titles = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("application/ld+json") {
        rest = &rest[start..];
        let Some(open) = rest.find('>') else { break };
        let Some(close) = rest.find("</script>") else {
            break;
        };
        if open < close
            && let Ok(json) = serde_json::from_str::<Value>(&rest[open + 1..close])
        {
            collect(&json, &mut titles);
        }
        rest = &rest[close..];
    }
    titles
}

fn collect(node: &Value, into: &mut Vec<Listed>) {
    match node {
        Value::Array(items) => items.iter().for_each(|i| collect(i, into)),
        Value::Object(map) => {
            let kind = map.get("@type").and_then(Value::as_str).unwrap_or_default();
            if let ("Movie" | "TVSeries", Some(title)) =
                (kind, map.get("name").and_then(Value::as_str))
            {
                let year = map
                    .get("dateCreated")
                    .or_else(|| map.get("startDate"))
                    .and_then(Value::as_str)
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse().ok());
                into.push(Listed {
                    title: title.trim().to_owned(),
                    year,
                    series: kind == "TVSeries",
                });
            } else if let Some(items) = map.get("itemListElement") {
                collect(items, into);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const PAGE: &str = r#"<html><head><script type="application/ld+json">{"@context":"http://schema.org",
        "@type":"ItemList","name":"Best Movies to Stream at Home","itemListElement":{"@type":"ItemList",
        "itemListElement":[{"@type":"Movie","position":1,"name":"Tenzing","dateCreated":"2026",
        "url":"https://www.rottentomatoes.com/m/tenzing"},{"@type":"Movie","position":2,"name":" Sacrifice ",
        "dateCreated":"2025"},{"@type":"TVSeries","name":"Severance","startDate":"2022-02-18"},
        {"@type":"Person","name":"Someone"}]}}</script>
        <script type="application/ld+json">not json</script></head></html>"#;

    #[test]
    fn reads_the_item_list() {
        assert_eq!(
            parse(PAGE),
            [
                Listed {
                    title: "Tenzing".into(),
                    year: Some(2026),
                    series: false
                },
                Listed {
                    title: "Sacrifice".into(),
                    year: Some(2025),
                    series: false
                },
                Listed {
                    title: "Severance".into(),
                    year: Some(2022),
                    series: true
                },
            ]
        );
        assert!(parse("<html>nothing</html>").is_empty());
    }

    #[test]
    fn only_rotten_tomatoes_browse_pages() {
        assert!(is_browse_url(
            "https://www.rottentomatoes.com/browse/movies_at_home/sort:popular"
        ));
        for url in [
            "http://www.rottentomatoes.com/browse/movies_at_home",
            "https://www.rottentomatoes.com/m/tenzing",
            "https://rottentomatoes.com.evil.example/browse/x",
            "https://www.rottentomatoes.com:8443/browse/x",
            "https://user@www.rottentomatoes.com/browse/x",
            "https://192.168.1.1/browse/x",
        ] {
            assert!(!is_browse_url(url), "{url}");
        }
    }
}
