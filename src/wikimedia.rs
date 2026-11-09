//! Wikidata and Wikimedia Commons, for artists' portraits in music searches and the library's
//! artists, on only with `MELETE_ARTIST_PORTRAITS` (and MusicBrainz). Wikidata is asked for one
//! item's image (its P18 statement) by the item's id, Commons for that file's picture at one
//! width with its author and licence by the file's name, and the picture host Commons names
//! (upload.wikimedia.org or thumb.wikimedia.org) for the picture itself; nothing else is sent.
//! Every request goes over https to those hosts only, redirects included, with Melete's usual
//! User-Agent (no contact, though Wikimedia's policy asks for one: a contact would name the
//! household), a timeout and a size cap.
//!
//! Requests keep to about one a second, all of them together, as Wikimedia asks of clients
//! that make many: each takes the next free turn, and one whose turn is more than [`MAX_WAIT`]
//! away isn't sent at all ([`Busy`]). A 429 or 503 holds every request back for as long as its
//! `Retry-After` says (or [`BUSY_WAIT`] when it says nothing); a hold that short is waited out
//! and the request tried once more, a longer one fails it and the requests after it until it's
//! over.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;

const WIKIDATA: &str = "https://www.wikidata.org";
const COMMONS: &str = "https://commons.wikimedia.org";
/// The width pictures are asked for at, in pixels: an album cover's.
pub const WIDTH: &str = "500";
/// The largest answer read from either API; one item's claims or one file's details are a few
/// kilobytes.
const MAX_ANSWER: usize = 1024 * 1024;
/// The largest picture read.
const MAX_PICTURE: usize = 10 * 1024 * 1024;
/// The longest author or licence kept.
const MAX_TEXT: usize = 200;
/// The time between two requests' turns.
const SPACING: Duration = Duration::from_secs(1);
/// The longest a request waits for its turn, or for a hold to be over; past that it isn't sent.
pub const MAX_WAIT: Duration = Duration::from_secs(15);
/// How long a 429 or 503 that doesn't say holds requests back.
const BUSY_WAIT: Duration = Duration::from_secs(60);
/// The longest hold kept, however long Wikimedia asks for.
const MOST_HELD: Duration = Duration::from_secs(24 * 3600);

/// Wikimedia asked for a pause, or too many requests are waiting their turn, so nothing was
/// sent: worth asking again later, not a sign anything is wrong with what was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Wikimedia asked for a pause, or is being asked too much at once")
    }
}

impl std::error::Error for Busy {}

/// When requests may go.
#[derive(Debug)]
struct Turns {
    /// The next free turn.
    next: Instant,
    /// Until when Wikimedia asked for a pause.
    held_until: Option<Instant>,
}

/// A Wikimedia client that follows redirects only to Wikimedia's hosts over https.
pub struct Client {
    http: crate::proxy::Http,
    wikidata: String,
    commons: String,
    /// For tests: any host, and plain http.
    any_host: bool,
    turns: Mutex<Turns>,
    spacing: Duration,
}

/// A picture on Commons, with what its licence asks to be shown with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    /// The picture at [`WIDTH`] pixels wide (or less when it's smaller), on one of
    /// [`PICTURE_HOSTS`].
    pub url: String,
    /// The file's page on Commons.
    pub page: Option<String>,
    /// Who made it, as plain text.
    pub author: Option<String>,
    /// Its licence's short name, such as "CC BY-SA 4.0".
    pub licence: Option<String>,
    /// Where the licence is written.
    pub licence_url: Option<String>,
}

/// One of Wikimedia's hosts, over https.
fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str().unwrap_or_default(),
            "www.wikidata.org"
                | "commons.wikimedia.org"
                | "upload.wikimedia.org"
                | "thumb.wikimedia.org"
        )
}

/// The hosts Commons serves pictures from: originals on upload.wikimedia.org, and resized
/// copies on thumb.wikimedia.org (on upload.wikimedia.org before Commons moved them).
const PICTURE_HOSTS: [&str; 2] = ["upload.wikimedia.org", "thumb.wikimedia.org"];

/// Whether `url` is a picture on Commons' picture hosts, over https.
fn is_picture(url: &reqwest::Url) -> bool {
    allowed(url) && PICTURE_HOSTS.contains(&url.host_str().unwrap_or_default())
}

/// A Wikidata item's id: Q and a number.
pub fn is_item(id: &str) -> bool {
    id.len() <= 16
        && id.strip_prefix('Q').is_some_and(|n| {
            !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
        })
}

/// HTML as plain text: tags left out (a line break or a block's end read as a space), the usual
/// entities read, spaces collapsed, at most [`MAX_TEXT`] characters. None when nothing's left.
fn plain(html: &str) -> Option<String> {
    let mut text = String::new();
    let mut tag: Option<String> = None;
    for c in html.chars() {
        match (&mut tag, c) {
            (None, '<') => tag = Some(String::new()),
            (None, c) => text.push(c),
            (Some(name), '>') => {
                let name = name
                    .trim_matches(|c| c == '/' || c == ' ')
                    .to_ascii_lowercase();
                if ["br", "p", "div", "li"]
                    .iter()
                    .any(|b| name == *b || name.starts_with(&format!("{b} ")))
                {
                    text.push(' ');
                }
                tag = None;
            }
            (Some(name), c) => name.push(c),
        }
    }
    let text = text
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(MAX_TEXT).collect();
    (!text.is_empty()).then_some(text)
}

#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    claims: std::collections::HashMap<String, Vec<Claim>>,
}

#[derive(Deserialize)]
struct Claim {
    #[serde(default)]
    rank: String,
    mainsnak: Snak,
}

#[derive(Deserialize)]
struct Snak {
    datavalue: Option<DataValue>,
}

#[derive(Deserialize)]
struct DataValue {
    value: serde_json::Value,
}

#[derive(Deserialize)]
struct Query {
    query: Option<Pages>,
}

#[derive(Deserialize)]
struct Pages {
    #[serde(default)]
    pages: Vec<Page>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    imageinfo: Vec<ImageInfo>,
}

#[derive(Deserialize)]
struct ImageInfo {
    thumburl: Option<String>,
    descriptionurl: Option<String>,
    #[serde(default)]
    extmetadata: std::collections::HashMap<String, Meta>,
}

#[derive(Deserialize)]
struct Meta {
    value: serde_json::Value,
}

impl Client {
    /// A client for wikidata.org and commons.wikimedia.org.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_bases(WIKIDATA.into(), COMMONS.into(), false)
    }

    /// Clients for other addresses (tests' fakes). `any_host` also lets the fake's redirects and
    /// picture addresses be anywhere, over plain http; without it they're held to Wikimedia's
    /// hosts over https, as the real client's are.
    pub fn with_bases(wikidata: String, commons: String, any_host: bool) -> anyhow::Result<Self> {
        let policy = move || {
            reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if any_host || allowed(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("redirected somewhere other than Wikimedia")
                }
            })
        };
        let http = crate::proxy::Http::new(crate::proxy::Service::Wikimedia, move |b| {
            b.timeout(Duration::from_secs(20))
                .redirect(policy())
                .user_agent(concat!("Melete/", env!("CARGO_PKG_VERSION")))
        })?;
        Ok(Self {
            http,
            wikidata,
            commons,
            any_host,
            turns: Mutex::new(Turns {
                next: Instant::now(),
                held_until: None,
            }),
            spacing: SPACING,
        })
    }

    /// The same client with `spacing` between turns (tests' fakes, to keep them quick).
    #[cfg(test)]
    pub fn spaced(mut self, spacing: Duration) -> Self {
        self.spacing = spacing;
        self
    }

    /// Whether Wikimedia asked for a pause longer than [`MAX_WAIT`] that isn't over, so every
    /// request would fail [`Busy`] now: a lookup that would also ask MusicBrainz isn't worth
    /// starting.
    pub fn held(&self) -> bool {
        let turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        turns
            .held_until
            .is_some_and(|until| until > Instant::now() + MAX_WAIT)
    }

    /// Holds every request back for `wait` (at most [`MOST_HELD`]), or longer when an earlier
    /// hold says so.
    fn hold(&self, wait: Duration) {
        let until = Instant::now() + wait.min(MOST_HELD);
        let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        turns.held_until = Some(turns.held_until.map_or(until, |held| held.max(until)));
    }

    /// Waits for a request's turn: the next free one, after any hold. [`Busy`], and no turn
    /// taken, when that's more than [`MAX_WAIT`] away.
    async fn turn(&self) -> Result<(), Busy> {
        let at = {
            let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            let at = turns.next.max(now).max(turns.held_until.unwrap_or(now));
            if at > now + MAX_WAIT {
                return Err(Busy);
            }
            turns.next = at + self.spacing;
            at
        };
        tokio::time::sleep_until(at.into()).await;
        Ok(())
    }

    /// The file name of a Wikidata item's image (P18): its preferred one, else its first
    /// normal one. None when it has none.
    pub async fn image_of(&self, item: &str) -> anyhow::Result<Option<String>> {
        anyhow::ensure!(is_item(item), "not a Wikidata item");
        let url = reqwest::Url::parse_with_params(
            &format!("{}/w/api.php", self.wikidata),
            &[
                ("action", "wbgetclaims"),
                ("entity", item),
                ("property", "P18"),
                ("format", "json"),
            ],
        )?;
        let (body, _) = self.get(url, MAX_ANSWER, "Wikidata").await?;
        let answer: Claims = serde_json::from_slice(&body)
            .map_err(|err| anyhow::anyhow!("Wikidata's answer: {err}"))?;
        let claims = answer
            .claims
            .get("P18")
            .map(Vec::as_slice)
            .unwrap_or_default();
        let file = |rank: &str| {
            claims.iter().filter(|c| c.rank == rank).find_map(|c| {
                c.mainsnak
                    .datavalue
                    .as_ref()?
                    .value
                    .as_str()
                    .map(str::to_owned)
            })
        };
        Ok(file("preferred").or_else(|| file("normal")))
    }

    /// The Commons file `name` at [`WIDTH`] pixels, with its author and licence. None when
    /// Commons has no such file, or its picture isn't on one of [`PICTURE_HOSTS`].
    pub async fn picture(&self, name: &str) -> anyhow::Result<Option<Picture>> {
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= 255
                && !name.contains(['|', '#', '<', '>', '[', ']', '{', '}']),
            "not a Commons file name"
        );
        let title = format!("File:{name}");
        let url = reqwest::Url::parse_with_params(
            &format!("{}/w/api.php", self.commons),
            &[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("prop", "imageinfo"),
                ("iiprop", "url|extmetadata"),
                ("iiurlwidth", WIDTH),
                ("iiextmetadatafilter", "Artist|LicenseShortName|LicenseUrl"),
                ("titles", title.as_str()),
            ],
        )?;
        let (body, _) = self.get(url, MAX_ANSWER, "Commons").await?;
        let answer: Query = serde_json::from_slice(&body)
            .map_err(|err| anyhow::anyhow!("Commons' answer: {err}"))?;
        let Some(info) = answer
            .query
            .and_then(|q| q.pages.into_iter().next())
            .and_then(|p| p.imageinfo.into_iter().next())
        else {
            return Ok(None);
        };
        let ok = |url: &str| reqwest::Url::parse(url).is_ok_and(|u| self.any_host || allowed(&u));
        let picture =
            |u: &str| reqwest::Url::parse(u).is_ok_and(|u| self.any_host || is_picture(&u));
        let Some(url) = info.thumburl.filter(|u| picture(u)) else {
            return Ok(None);
        };
        let meta = |key: &str| {
            info.extmetadata
                .get(key)
                .and_then(|m| m.value.as_str())
                .and_then(plain)
        };
        let licence_url =
            meta("LicenseUrl").filter(|u| u.starts_with("https://") || u.starts_with("http://"));
        Ok(Some(Picture {
            url,
            page: info.descriptionurl.filter(|u| ok(u)),
            author: meta("Artist"),
            licence: meta("LicenseShortName"),
            licence_url,
        }))
    }

    /// A picture [`picture`](Self::picture) gave, at most 10 MiB, with its type.
    pub async fn download(&self, url: &str) -> anyhow::Result<(Vec<u8>, String)> {
        let url =
            reqwest::Url::parse(url).map_err(|_| anyhow::anyhow!("not a picture's address"))?;
        anyhow::ensure!(
            self.any_host || is_picture(&url),
            "a picture not on Wikimedia's picture hosts"
        );
        let (bytes, content_type) = self.get(url, MAX_PICTURE, "Wikimedia").await?;
        anyhow::ensure!(
            content_type.starts_with("image/"),
            "Wikimedia sent {content_type:?}, not an image"
        );
        Ok((bytes, content_type))
    }

    /// One request's body, at most `most` bytes, and its type, at its turn ([`turn`](Self::turn)).
    /// A 429 or 503 holds requests back ([`hold`](Self::hold)) and is tried once more after a
    /// short hold; [`Busy`] when the hold is longer, or the second try is refused too. Errors
    /// never carry the address.
    async fn get(
        &self,
        url: reqwest::Url,
        most: usize,
        who: &str,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let failed = |err: reqwest::Error| anyhow::anyhow!("{who}: {}", err.without_url());
        let mut tries = 0;
        let mut response = loop {
            self.turn().await?;
            let response = self.http.get(url.clone()).send().await.map_err(failed)?;
            let status = response.status();
            if status != reqwest::StatusCode::TOO_MANY_REQUESTS
                && status != reqwest::StatusCode::SERVICE_UNAVAILABLE
            {
                break response;
            }
            let wait = retry_after(&response).unwrap_or(BUSY_WAIT);
            self.hold(wait);
            tries += 1;
            if tries > 1 || wait > MAX_WAIT {
                tracing::warn!(
                    service = who,
                    status = status.as_u16(),
                    wait_seconds = wait.min(MOST_HELD).as_secs(),
                    "Wikimedia asked for a pause"
                );
                return Err(Busy.into());
            }
        };
        anyhow::ensure!(
            response.status().is_success(),
            "{who} answered {}",
            response.status()
        );
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= most,
                "{who}'s answer is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok((bytes, content_type))
    }
}

/// The wait a 429 or 503 asks for, in seconds as Wikimedia gives it. None when it names none, or
/// names it some other way.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    let seconds: u64 = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
