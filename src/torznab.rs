//! Torznab: asking torrent indexers (Prowlarr's per-indexer feeds) what they have. The only
//! code that talks to indexers.
//!
//! Torznab puts the API key in the query string, and download links carry it too. So errors
//! never quote URLs (reqwest's are stripped of theirs), redirects aren't followed, and download
//! links stay inside Melete: `Item`'s Debug output hides them, and nothing returns them to API
//! clients. Guids and info URLs are treated the same way: feeds behind Prowlarr often use the
//! tracker's download link, passkey included, as the guid.

use std::fmt;
use std::time::Duration;

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use reqwest::StatusCode;

mod other_kinds;
mod xml;

pub use other_kinds::*;
pub use xml::*;

/// The largest reply read; a page of 100 results is well under 1 MiB.
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Why an indexer gave no results. The messages never quote a URL, so never the API key.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The indexer refused the API key.
    Rejected,
    /// Prowlarr has the indexer turned off.
    Disabled,
    /// No answer: the connection failed or timed out, the URL isn't one, or the indexer needs
    /// FlareSolverr and none is set. The reason never holds the URL.
    Unreachable(String),
    /// An HTTP status other than success, 401, 403 or 410.
    Status(u16),
    /// The indexer answered with a Torznab error.
    Indexer {
        /// Torznab's error code, as sent.
        code: String,
        /// The indexer's own words, cut to 200 characters.
        description: String,
    },
    /// The reply was over the size cap (16 MiB).
    TooLarge,
    /// The reply wasn't what the indexer's kind sends (not Torznab, not UTF-8, no results
    /// table).
    Malformed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Rejected => f.write_str("the indexer rejected the API key"),
            Error::Disabled => f.write_str("the indexer is turned off in Prowlarr"),
            Error::Unreachable(reason) => write!(f, "can't reach the indexer: {reason}"),
            Error::Status(status) => write!(f, "the indexer answered {status}"),
            Error::Indexer { code, description } => {
                write!(f, "the indexer reported error {code}: {description}")
            }
            Error::TooLarge => write!(
                f,
                "the indexer sent more than {} MiB",
                MAX_RESPONSE_BYTES >> 20
            ),
            Error::Malformed(reason) => write!(f, "the indexer's reply isn't Torznab: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

/// Where an indexer is and its key.
pub struct Endpoint<'a> {
    /// The feed's URL (for Prowlarr, one indexer's Torznab URL), without the key.
    pub url: &'a str,
    /// The indexer's API key. Only Torznab feeds are sent it, in the query string.
    pub api_key: &'a str,
}

/// What an indexer says it can do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// Standard category ids (Prowlarr adds its own, from 100000; those are left out).
    pub categories: Vec<i64>,
    /// The parameters movie and TV searches take; empty when unavailable.
    pub movie_search: Vec<String>,
    /// The same for TV searches.
    pub tv_search: Vec<String>,
}

/// One release an indexer listed. Its Debug output hides the guid and both links.
#[derive(Default, Clone, PartialEq, Eq)]
pub struct Item {
    /// The release name, as the indexer gives it.
    pub title: String,
    /// May carry a tracker passkey (some feeds use the download link): never logged or returned.
    pub guid: Option<String>,
    /// The torrent or magnet link. It carries the API key: never logged or returned.
    pub download_url: Option<String>,
    /// The release's page on the tracker. Treated like the guid.
    pub info_url: Option<String>,
    /// Unix seconds.
    pub published: Option<i64>,
    /// The download's size in bytes.
    pub size: Option<u64>,
    /// Peers with the whole release.
    pub seeders: Option<u32>,
    /// The indexer's peer count: Torznab's `peers`, or the leechers where an indexer lists
    /// them apart from the seeders.
    pub peers: Option<u32>,
    /// Torznab category ids (2000 movies, 5000 TV and so on); empty when the feed gives none.
    pub categories: Vec<i64>,
    /// The torrent's info hash, as the feed writes it.
    pub infohash: Option<String>,
    /// As custom formats name indexer flags: "freeleech", "halfleech", "double_upload"…
    pub flags: Vec<String>,
    /// The title's ids, when the indexer gives them; the IMDb id without its "tt".
    pub imdb_id: Option<String>,
    /// The TVDB id.
    pub tvdb_id: Option<i64>,
    /// The TMDB id.
    pub tmdb_id: Option<i64>,
}

impl fmt::Debug for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Item")
            .field("title", &self.title)
            .field("guid", &self.guid.as_ref().map(|_| "(hidden)"))
            .field(
                "download_url",
                &self.download_url.as_ref().map(|_| "(hidden)"),
            )
            .field("info_url", &self.info_url.as_ref().map(|_| "(hidden)"))
            .field("published", &self.published)
            .field("size", &self.size)
            .field("seeders", &self.seeders)
            .field("peers", &self.peers)
            .field("categories", &self.categories)
            .field("infohash", &self.infohash)
            .field("flags", &self.flags)
            .field("imdb_id", &self.imdb_id)
            .field("tvdb_id", &self.tvdb_id)
            .field("tmdb_id", &self.tmdb_id)
            .finish()
    }
}

/// Asks indexers for results. Holds the one HTTP client every indexer request goes through:
/// 30-second timeout, no redirects, a User-Agent that names only Melete and its version.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
}

impl Client {
    /// A client with that HTTP setup.
    ///
    /// # Errors
    ///
    /// When the HTTP client can't be built (no TLS backend).
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // The key is in the URL; a redirect would hand it to wherever it points.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http })
    }

    /// Asks a Torznab indexer what it can do (`t=caps`).
    pub async fn caps(&self, endpoint: &Endpoint<'_>) -> Result<Caps, Error> {
        parse_caps(&self.get(endpoint, &[("t", "caps")]).await?)
    }

    /// Searches an indexer of any kind with Torznab's parameters, mapped onto its own: for The
    /// Pirate Bay `q` or `imdbid`, for Nyaa `q` (titles only: fansubs don't name episodes as
    /// S01E05). Without either, its newest releases. Only `cat`'s kinds of release come back.
    pub async fn search_as(
        &self,
        kind: Kind,
        endpoint: &Endpoint<'_>,
        params: &[(&str, &str)],
    ) -> Result<Vec<Item>, Error> {
        let param = |name: &str| params.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
        let wanted: Vec<i64> = param("cat")
            .unwrap_or_default()
            .split(',')
            .filter_map(|c| c.parse().ok())
            .collect();
        match kind {
            Kind::Torznab => self.search(endpoint, params).await,
            Kind::Apibay => {
                let base = endpoint.url.trim_end_matches('/');
                let query = param("imdbid")
                    .map(|id| format!("tt{id}"))
                    .or(param("q").map(str::to_owned));
                let mut items = Vec::new();
                match query {
                    Some(q) => {
                        // Its music (100) when music is what's asked for, else its video.
                        let audio = !wanted.is_empty() && wanted.iter().all(|w| w / 1000 == 3);
                        let (category, known) = if audio {
                            ("100", &APIBAY_AUDIO_CATEGORIES[..])
                        } else {
                            ("200", &APIBAY_CATEGORIES[..])
                        };
                        let json = self
                            .fetch(&format!("{base}/q.php"), &[("q", &q), ("cat", category)])
                            .await?;
                        items.extend(parse_apibay_in(&json, known)?);
                    }
                    // Newest: the last two days' busiest of each video category asked for.
                    None => {
                        for (category, kind) in APIBAY_CATEGORIES {
                            if !wanted.iter().any(|w| w / 1000 == kind / 1000) {
                                continue;
                            }
                            let url = format!("{base}/precompiled/data_top100_48h_{category}.json");
                            items.extend(parse_apibay(&self.fetch(&url, &[]).await?)?);
                        }
                        let mut seen = std::collections::HashSet::new();
                        items.retain(|i: &Item| seen.insert(i.infohash.clone()));
                    }
                }
                items.retain(|i| {
                    i.categories
                        .iter()
                        .any(|c| wanted.iter().any(|w| w / 1000 == c / 1000))
                });
                Ok(items)
            }
            Kind::Uindex => {
                let Some(flaresolverr) = flaresolverr() else {
                    return Err(Error::Unreachable(
                        "Uindex sits behind Cloudflare's check; set MELETE_FLARESOLVERR_URL".into(),
                    ));
                };
                search_uindex(&flaresolverr, endpoint.url, param("q"), &wanted).await
            }
            Kind::Nyaa => {
                // Without its episode numbers, which fansubs write as "- 05".
                let title = param("q").map(|q| strip_numbers(q).to_owned());
                let mut query = vec![("page", "rss"), ("c", "1_2"), ("f", "0")];
                if let Some(title) = title.as_deref() {
                    query.push(("q", title));
                }
                let xml = self
                    .fetch(endpoint.url.trim_end_matches('/'), &query)
                    .await?;
                let mut items = parse_items(&xml)?;
                // English-translated anime: series, and its films.
                for item in &mut items {
                    item.categories = vec![5070, 2000];
                }
                Ok(items)
            }
        }
    }

    /// Runs a search: `params` are Torznab's ("t", "q", "cat", "imdbid", "season"…). With only
    /// `t=search`, indexers list their newest releases.
    pub async fn search(
        &self,
        endpoint: &Endpoint<'_>,
        params: &[(&str, &str)],
    ) -> Result<Vec<Item>, Error> {
        parse_items(&self.get(endpoint, params).await?)
    }

    async fn get(&self, endpoint: &Endpoint<'_>, params: &[(&str, &str)]) -> Result<String, Error> {
        let mut with_key = params.to_vec();
        with_key.push(("apikey", endpoint.api_key));
        self.fetch(endpoint.url, &with_key).await
    }

    /// GETs a URL with a query, as text: the timeout, the size cap, no redirects, and errors
    /// without the URL hold for every kind of indexer.
    async fn fetch(&self, url: &str, params: &[(&str, &str)]) -> Result<String, Error> {
        let unreachable = |err: reqwest::Error| Error::Unreachable(err.without_url().to_string());
        let mut response = self
            .http
            .get(url)
            .query(params)
            .send()
            .await
            .map_err(unreachable)?;
        match response.status() {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(Error::Rejected),
            StatusCode::GONE => return Err(Error::Disabled),
            status if !status.is_success() => return Err(Error::Status(status.as_u16())),
            _ => {}
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(unreachable)? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(Error::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        String::from_utf8(body).map_err(|_| Error::Malformed("not UTF-8".into()))
    }
}

/// How an indexer is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A Torznab feed, Prowlarr's or any other.
    Torznab,
    /// The Pirate Bay's JSON API (apibay.org).
    Apibay,
    /// Nyaa's RSS feed.
    Nyaa,
    /// Uindex's search page, through FlareSolverr (it's behind Cloudflare's check).
    Uindex,
}

/// The FlareSolverr to reach Uindex through, from `MELETE_FLARESOLVERR_URL`; set whenever the
/// outside services are (`live`).
static FLARESOLVERR: std::sync::RwLock<Option<crate::flaresolverr::Client>> =
    std::sync::RwLock::new(None);

fn flaresolverr() -> Option<crate::flaresolverr::Client> {
    FLARESOLVERR
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Makes FlareSolverr available to indexers that need it, or not.
pub fn use_flaresolverr(client: Option<crate::flaresolverr::Client>) {
    *FLARESOLVERR.write().unwrap_or_else(|e| e.into_inner()) = client;
}

/// Whether a FlareSolverr is set, so Uindex can be searched.
pub fn has_flaresolverr() -> bool {
    flaresolverr().is_some()
}

impl Kind {
    /// The kind stored as `kind` ("torznab", "apibay", "nyaa" or "uindex"); `None` for any
    /// other text.
    pub fn parse(kind: &str) -> Option<Self> {
        match kind {
            "torznab" => Some(Kind::Torznab),
            "apibay" => Some(Kind::Apibay),
            "nyaa" => Some(Kind::Nyaa),
            "uindex" => Some(Kind::Uindex),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn reads_prowlarrs_caps() {
        let caps = parse_caps(include_str!("../tests/fixtures/prowlarr/caps.xml")).unwrap();
        assert_eq!(caps.categories, [2000, 2040, 2045, 5000, 5040]);
        assert_eq!(caps.movie_search, ["q", "imdbid", "tmdbid"]);
        assert_eq!(caps.tv_search, ["q", "season", "ep", "tvdbid"]);
    }

    #[test]
    fn reads_prowlarrs_results() {
        let items = parse_items(include_str!("../tests/fixtures/prowlarr/search.xml")).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(
            item.title,
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR"
        );
        assert_eq!(item.guid.as_deref(), Some("https://fake.example/t/1"));
        let link = item.download_url.as_deref().unwrap();
        assert!(
            link.starts_with("http://127.0.0.1:19696/1/download?apikey=KEY&link="),
            "{link}"
        );
        assert_eq!(item.info_url.as_deref(), Some("https://fake.example/t/1"));
        assert_eq!(item.published, Some(1_792_836_000));
        assert_eq!(
            (item.size, item.seeders, item.peers),
            (Some(78_000_000_000), Some(120), Some(150))
        );
        assert_eq!(item.categories, [2045]);
        assert_eq!(
            item.infohash.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(item.flags, ["freeleech"]);
        let debug = format!("{item:?}");
        assert!(
            !debug.contains("apikey") && !debug.contains("fake.example"),
            "Debug hides links: {debug}"
        );
    }

    #[test]
    fn reads_the_ids_an_indexer_gives() {
        let xml = r#"<rss><channel>
            <item><title>a</title><torznab:attr name="imdbid" value="0111161"/>
              <torznab:attr name="tvdbid" value="81189"/><torznab:attr name="tmdbid" value="278"/></item>
            <item><title>b</title><torznab:attr name="imdb" value="tt15239678"/>
              <torznab:attr name="tvdbid" value="0"/><torznab:attr name="imdbid" value="0000000"/></item>
            </channel></rss>"#;
        let items = parse_items(xml).unwrap();
        let ids = |i: &Item| (i.imdb_id.clone(), i.tvdb_id, i.tmdb_id);
        assert_eq!(
            ids(&items[0]),
            (Some("111161".into()), Some(81189), Some(278))
        );
        assert_eq!(
            ids(&items[1]),
            (Some("15239678".into()), None, None),
            "0 is unknown"
        );
    }

    #[test]
    fn reads_entities_cdata_and_errors() {
        let xml = r#"<rss><channel><item><title><![CDATA[A & B]]> &amp; C&#33;</title>
            <link>magnet:?xt=urn:btih:ab&amp;dn=x</link></item><item><title></title></item></channel></rss>"#;
        let items = parse_items(xml).unwrap();
        assert_eq!(items.len(), 1, "items without a title are dropped");
        assert_eq!(items[0].title, "A & B & C!");
        assert_eq!(
            items[0].download_url.as_deref(),
            Some("magnet:?xt=urn:btih:ab&dn=x")
        );
        let error =
            r#"<?xml version="1.0"?><error code="410" description="Indexer is disabled" />"#;
        assert_eq!(
            parse_items(error).unwrap_err(),
            Error::Indexer {
                code: "410".into(),
                description: "Indexer is disabled".into()
            }
        );
        assert_eq!(
            parse_caps(r#"<error code="100" description="Invalid API Key"/>"#).unwrap_err(),
            Error::Rejected
        );
        assert!(matches!(
            parse_items("<html>login</html>").unwrap_err(),
            Error::Malformed(_)
        ));
        assert!(matches!(
            parse_items("<rss><item><title>x</rss>").unwrap_err(),
            Error::Malformed(_)
        ));
        // A reply cut off inside an item isn't a shorter list.
        let cut = "<rss><channel><item><title>a</title></item><item><title>b</title>";
        assert!(matches!(parse_items(cut).unwrap_err(), Error::Malformed(_)));
        // Enclosures and attributes written with end tags count too.
        let long = r#"<rss><channel><item><title>a</title>
            <enclosure url="https://x/1.torrent" length="5"></enclosure>
            <torznab:attr name="seeders" value="7"></torznab:attr></item></channel></rss>"#;
        let item = &parse_items(long).unwrap()[0];
        assert_eq!(
            (item.download_url.as_deref(), item.size, item.seeders),
            (Some("https://x/1.torrent"), Some(5), Some(7))
        );
        // Error descriptions are cut short.
        let wordy = format!(
            r#"<error code="900" description="{}"/>"#,
            "x".repeat(10_000)
        );
        match parse_items(&wordy).unwrap_err() {
            Error::Indexer { description, .. } => assert_eq!(description.len(), 200),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_dates() {
        assert_eq!(
            rfc2822("Sat, 24 Oct 2026 10:00:00 +0000"),
            Some(1_792_836_000)
        );
        assert_eq!(rfc2822("24 Oct 2026 12:00:00 +0200"), Some(1_792_836_000));
        assert_eq!(rfc2822("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(rfc2822("Tue, 29 Feb 2000 23:59 -0100"), Some(951_872_340));
        assert_eq!(rfc2822("yesterday"), None);
        // An absurd year from a hostile feed is refused, not overflowed.
        assert_eq!(
            rfc2822("Sat, 24 Oct 99999999999999999 10:00:00 +0000"),
            None
        );
    }

    async fn serve(router: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}/1/api")
    }

    #[tokio::test]
    async fn sends_the_key_and_keeps_it_out_of_errors() {
        use axum::extract::Query;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        use std::collections::HashMap;

        let router =
            axum::Router::new().fallback(|Query(q): Query<HashMap<String, String>>| async move {
                match (
                    q.get("apikey").map(String::as_str),
                    q.get("t").map(String::as_str),
                ) {
                    (Some("secret-key"), Some("caps")) => {
                        include_str!("../tests/fixtures/prowlarr/caps.xml").into_response()
                    }
                    (Some("secret-key"), Some("search")) => StatusCode::GONE.into_response(),
                    (Some("secret-key"), _) => {
                        axum::response::Redirect::to("http://127.0.0.1:9/elsewhere").into_response()
                    }
                    _ => StatusCode::UNAUTHORIZED.into_response(),
                }
            });
        let url = serve(router).await;
        let client = Client::new().unwrap();
        let good = Endpoint {
            url: &url,
            api_key: "secret-key",
        };
        assert_eq!(client.caps(&good).await.unwrap().categories.len(), 5);
        assert_eq!(
            client.search(&good, &[("t", "search")]).await.unwrap_err(),
            Error::Disabled
        );
        assert_eq!(
            client.search(&good, &[("t", "movie")]).await.unwrap_err(),
            Error::Status(303)
        );
        let bad = Endpoint {
            url: &url,
            api_key: "wrong",
        };
        assert_eq!(client.caps(&bad).await.unwrap_err(), Error::Rejected);
        // Nothing listens on port 9: the error names no URL, so no key.
        let gone = Endpoint {
            url: "http://127.0.0.1:9/1/api",
            api_key: "secret-key",
        };
        let err = client.caps(&gone).await.unwrap_err();
        assert!(matches!(err, Error::Unreachable(_)), "{err}");
        assert!(!err.to_string().contains("secret-key"), "{err}");
    }

    #[tokio::test]
    async fn stops_reading_past_the_size_limit() {
        let router = axum::Router::new().fallback(|| async { "x".repeat(MAX_RESPONSE_BYTES + 1) });
        let url = serve(router).await;
        let endpoint = Endpoint {
            url: &url,
            api_key: "k",
        };
        assert_eq!(
            Client::new().unwrap().caps(&endpoint).await.unwrap_err(),
            Error::TooLarge
        );
    }

    const APIBAY_SEARCH: &str = r#"[{"id":"75005578","name":"Dune Part Two (2024) [1080p] [WEBRip]","info_hash":"2770FE270845674966E184BE60ED1BE0FE494F3A",
        "leechers":"168","seeders":"1197","size":"2968337547","num_files":"3","username":"x","added":"1712378383","status":"vip",
        "category":"207","imdb":"tt15239678"},
        {"id":"1","name":"Some Album","info_hash":"0123456789ABCDEF0123456789ABCDEF01234567","leechers":"0","seeders":"5",
        "size":"1","added":"0","category":"101","imdb":""},
        {"id":"2","name":"Bad hash","info_hash":"zz","leechers":"0","seeders":"5","size":"1","added":"0","category":"207"}]"#;

    #[test]
    fn reads_the_pirate_bays_results() {
        let items = parse_apibay(APIBAY_SEARCH).unwrap();
        assert_eq!(items.len(), 1, "only video, with a real hash");
        let dune = &items[0];
        assert_eq!(
            (dune.seeders, dune.peers, dune.size, dune.published),
            (Some(1197), Some(168), Some(2968337547), Some(1712378383))
        );
        assert_eq!(dune.categories, [2040]);
        assert_eq!(dune.imdb_id.as_deref(), Some("15239678"));
        assert_eq!(
            dune.infohash.as_deref(),
            Some("2770fe270845674966e184be60ed1be0fe494f3a")
        );
        let link = dune.download_url.as_deref().unwrap();
        assert!(
            link.starts_with("magnet:?xt=urn:btih:2770fe27")
                && link.contains("&dn=Dune%20Part%20Two"),
            "{link}"
        );
        assert_eq!(
            crate::deluge::magnet_hash(link).as_deref(),
            Some("2770fe270845674966e184be60ed1be0fe494f3a")
        );
        // Its lists give numbers; "No results returned" is a row with id 0.
        let listed = parse_apibay(
            r#"[{"id":84843196,"info_hash":"3D5539172BB52ED6A019751B16EEBF2D9B2DDD56","category":208,"name":"Show S01E01 1080p",
                "size":1000,"seeders":3,"leechers":1,"added":1700000000},
               {"id":"0","name":"No results returned","info_hash":"0000000000000000000000000000000000000000","category":"0"}]"#,
        )
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].categories, [5040]);
        assert!(parse_apibay("<html>").is_err());
    }

    const NYAA: &str = r#"<rss xmlns:atom="http://www.w3.org/2005/Atom" xmlns:nyaa="https://nyaa.si/xmlns/nyaa" version="2.0"><channel>
        <item><title>[SubsPlease] Frieren - 05 (1080p) [ABCD1234].mkv</title><link>https://nyaa.si/download/1.torrent</link>
        <guid isPermaLink="true">https://nyaa.si/view/1</guid><pubDate>Fri, 06 Oct 2023 16:30:00 -0000</pubDate>
        <nyaa:seeders>812</nyaa:seeders><nyaa:leechers>9</nyaa:leechers><nyaa:downloads>40000</nyaa:downloads>
        <nyaa:infoHash>F7JBGPZI7PJJAINXOXAXZDQ46WWOXGQVABCDEF12</nyaa:infoHash><nyaa:categoryId>1_2</nyaa:categoryId>
        <nyaa:size>1.4 GiB</nyaa:size></item></channel></rss>"#;

    #[test]
    fn reads_nyaas_feed() {
        let items = parse_items(NYAA).unwrap();
        let item = &items[0];
        assert_eq!(
            item.download_url.as_deref(),
            Some("https://nyaa.si/download/1.torrent")
        );
        assert_eq!((item.seeders, item.peers), (Some(812), Some(9)));
        assert_eq!(item.size, Some(1503238554));
        assert!(item.infohash.is_some() && item.published.is_some());
        assert_eq!(
            strip_numbers("Frieren Beyond Journeys End S01E05"),
            "Frieren Beyond Journeys End"
        );
        assert_eq!(strip_numbers("Show S02"), "Show");
        assert_eq!(strip_numbers("Suzume 2022"), "Suzume");
        assert_eq!(strip_numbers("Steins Gate"), "Steins Gate");
        assert_eq!(strip_numbers("1917"), "1917");
    }

    /// The Pirate Bay's API and Nyaa's feed, faked; requests are logged.
    async fn trackers() -> (String, Arc<Mutex<Vec<String>>>) {
        use axum::extract::{OriginalUri, State};
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let router = axum::Router::new()
            .fallback(|State(log): State<Arc<Mutex<Vec<String>>>>, OriginalUri(uri): OriginalUri| async move {
                let path = uri.to_string();
                log.lock().unwrap().push(path.clone());
                if path.starts_with("/q.php") {
                    APIBAY_SEARCH.to_owned()
                } else if path.starts_with("/precompiled/data_top100_48h_208.json") {
                    r#"[{"id":5,"info_hash":"3D5539172BB52ED6A019751B16EEBF2D9B2DDD56","category":208,"name":"Show S01E01 1080p"}]"#.to_owned()
                } else if path.starts_with("/precompiled/") {
                    "[]".to_owned()
                } else {
                    NYAA.to_owned()
                }
            })
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (url, log)
    }

    #[tokio::test]
    async fn searches_trackers_of_every_kind_with_torznab_parameters() {
        let (url, log) = trackers().await;
        let client = Client::new().unwrap();
        let endpoint = Endpoint {
            url: &url,
            api_key: "",
        };
        // A film by its IMDb id.
        let found = client
            .search_as(
                Kind::Apibay,
                &endpoint,
                &[("t", "movie"), ("cat", "2000,2040"), ("imdbid", "15239678")],
            )
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(log.lock().unwrap()[0].starts_with("/q.php?q=tt15239678&cat=200"));
        // TV only: the film is left out.
        let tv = client
            .search_as(
                Kind::Apibay,
                &endpoint,
                &[("t", "search"), ("cat", "5000,5040"), ("q", "Dune")],
            )
            .await
            .unwrap();
        assert!(tv.is_empty());
        // Newest, for TV: the 48-hour lists of TV categories only.
        log.lock().unwrap().clear();
        let newest = client
            .search_as(
                Kind::Apibay,
                &endpoint,
                &[("t", "search"), ("cat", "5000,5040")],
            )
            .await
            .unwrap();
        assert_eq!(newest.len(), 1);
        let asked = log.lock().unwrap().clone();
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert!(
            asked
                .iter()
                .all(|p| p.contains("_205.") || p.contains("_208.") || p.contains("_212."))
        );
        // Nyaa by title, English-translated anime.
        log.lock().unwrap().clear();
        let anime = client
            .search_as(
                Kind::Nyaa,
                &endpoint,
                &[("t", "search"), ("cat", "5070"), ("q", "Frieren S01E05")],
            )
            .await
            .unwrap();
        assert_eq!(anime[0].categories, [5070, 2000]);
        let asked = log.lock().unwrap()[0].clone();
        assert!(
            asked.contains("page=rss")
                && asked.contains("c=1_2")
                && asked.contains("q=Frieren")
                && !asked.contains("S01E05"),
            "{asked}"
        );
    }

    const UINDEX: &str = include_str!("../tests/fixtures/uindex/search.html");

    #[test]
    fn reads_uindexs_results() {
        let now = 1_792_950_000;
        let items = parse_uindex(UINDEX, now).unwrap();
        assert_eq!(items.len(), 3);
        let first = &items[0];
        assert!(
            first.title.starts_with("Dune Part Two 2024"),
            "{}",
            first.title
        );
        assert!(!first.title.contains("<mark>"));
        assert_eq!((first.seeders, first.peers), (Some(336), Some(45)));
        assert_eq!(
            first.size,
            Some((34.26f64 * 1024.0 * 1024.0 * 1024.0).round() as u64)
        );
        assert_eq!(first.categories, [2000]);
        assert_eq!(
            first.infohash.as_deref(),
            Some("468b01356a39b6e5226e1c7ff6edffc55eaa9da4")
        );
        assert!(first.guid.as_deref().unwrap().starts_with("uindex:"));
        let age = now - first.published.unwrap();
        assert!(
            (age - (1.5 * 365.0 * 86_400.0) as i64).abs() < 86_400,
            "{age}"
        );
        // TV and anime map to Torznab's; music and the rest are left out.
        let tv = UINDEX
            .replacen("/search.php?c=1\"", "/search.php?c=2\"", 1)
            .replacen("/search.php?c=1\"", "/search.php?c=4\"", 1);
        let cats: Vec<Vec<i64>> = parse_uindex(&tv, now)
            .unwrap()
            .into_iter()
            .map(|i| i.categories)
            .collect();
        assert_eq!(cats, [vec![5000], vec![2000]]);
        // A page without the table (a challenge, a redesign) is an error, not "nothing found".
        assert!(matches!(
            parse_uindex("<html><title>Just a moment...</title></html>", now),
            Err(Error::Malformed(_))
        ));
        assert_eq!(ago("3 hours ago", 10_000), Some(10_000 - 3 * 3600));
        assert_eq!(ago("an hour ago", 10_000), Some(10_000 - 3600));
        assert_eq!(ago("yesterday", 10_000), None);
    }

    #[tokio::test]
    async fn searches_uindex_through_flaresolverr() {
        let (url, log) = crate::flaresolverr::testing::start(UINDEX).await;
        let flaresolverr = crate::flaresolverr::Client::new(&url).unwrap();
        let found = search_uindex(
            &flaresolverr,
            "https://uindex.org",
            Some("Dune Part Two"),
            &[2000, 2040],
        )
        .await
        .unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(
            log.lock().unwrap()[0],
            "https://uindex.org/search.php?search=Dune+Part+Two&c=0"
        );
        let tv_only = search_uindex(&flaresolverr, "https://uindex.org", None, &[5000])
            .await
            .unwrap();
        assert!(tv_only.is_empty());
        assert_eq!(
            log.lock().unwrap()[1],
            "https://uindex.org/search.php?search=&c=0",
            "newest"
        );
        // Without FlareSolverr set, it says what's missing.
        let client = Client::new().unwrap();
        let endpoint = Endpoint {
            url: "https://uindex.org",
            api_key: "",
        };
        if !has_flaresolverr() {
            let err = client
                .search_as(Kind::Uindex, &endpoint, &[("t", "search"), ("cat", "2000")])
                .await
                .unwrap_err();
            assert!(err.to_string().contains("MELETE_FLARESOLVERR_URL"));
        }
    }
}
