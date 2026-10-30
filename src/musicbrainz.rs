//! MusicBrainz, for music metadata. Its web service allows one request a second per
//! address and answers 503 when asked too fast. It asks for a User-Agent naming the application
//! and a contact; Melete names itself only, since a contact would tie the household's address
//! and library to a person. Releases are looked up by id with their tracks; albums without ids
//! are searched for by title, artist and track count. Data is CC0.

use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;

mod answers;
mod finding;
mod moved;
mod releases;

pub use answers::*;
use moved::*;
pub use releases::*;

const BASE: &str = "https://musicbrainz.org/ws/2";
const USER_AGENT: &str = concat!("Melete/", env!("CARGO_PKG_VERSION"));
/// A little over a second between requests, as MusicBrainz asks.
const SPACING: Duration = Duration::from_millis(1100);
/// The largest answer read; a release with its tracks is a few kilobytes.
const MAX_BYTES: usize = 4 * 1024 * 1024;
/// How long to wait after a 503 before the one retry.
const BUSY_WAIT: Duration = Duration::from_secs(5);

/// A MusicBrainz client that keeps to one request a second, shared by everything that asks.
pub struct Client {
    http: reqwest::Client,
    base: String,
    /// When the next request may go.
    next: Mutex<Instant>,
    spacing: Duration,
    busy_wait: Duration,
}

/// Why a lookup gave nothing.
#[derive(Debug)]
pub enum Error {
    /// The id isn't a MusicBrainz id, or MusicBrainz has nothing under it.
    NotFound,
    /// The request or its answer failed, and why.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => f.write_str("MusicBrainz has no such release"),
            Error::Failed(reason) => write!(f, "MusicBrainz: {reason}"),
        }
    }
}

impl Client {
    /// A client for musicbrainz.org.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_base(BASE.into(), SPACING, BUSY_WAIT)
    }

    /// A client for another address, with its own spacing and wait after a busy answer (for tests).
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn with_base(base: String, spacing: Duration, busy_wait: Duration) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self {
            http,
            base,
            next: Mutex::new(Instant::now()),
            spacing,
            busy_wait,
        })
    }

    /// A release with its release group, artists and tracks, and the works its recordings
    /// perform with their composers and the works they're parts of.
    pub async fn release(&self, id: &str) -> Result<Release, Error> {
        if !crate::library::tags::is_mbid(id) {
            return Err(Error::NotFound);
        }
        let query = "inc=release-groups+artist-credits+recordings+media+recording-level-rels\
             +work-rels+work-level-rels+artist-rels&fmt=json";
        let body = self.lookup("release", id, query).await?;
        serde_json::from_str(&body).map_err(|err| Error::Failed(format!("its answer: {err}")))
    }

    /// Releases called `title`, by `artist` when given, best first.
    pub async fn search(&self, title: &str, artist: Option<&str>) -> Result<Vec<Found>, Error> {
        let mut query = format!("release:{}", phrase(title));
        if let Some(artist) = artist {
            query.push_str(&format!(" AND artist:{}", phrase(artist)));
        }
        let url = reqwest::Url::parse_with_params(
            &format!("{}/release", self.base),
            &[("query", query.as_str()), ("limit", "25"), ("fmt", "json")],
        )
        .map_err(|err| Error::Failed(err.to_string()))?;
        let body = self.get(url.as_str()).await?;
        let search: Search = serde_json::from_str(&body)
            .map_err(|err| Error::Failed(format!("its answer: {err}")))?;
        Ok(search.releases)
    }

    /// A release group (an album, whichever edition) with its artists.
    pub async fn release_group(&self, id: &str) -> Result<ReleaseGroupDetail, Error> {
        if !crate::library::tags::is_mbid(id) {
            return Err(Error::NotFound);
        }
        let body = self
            .lookup("release-group", id, "inc=artist-credits&fmt=json")
            .await?;
        serde_json::from_str(&body).map_err(|err| Error::Failed(format!("its answer: {err}")))
    }

    /// An artist by id, with its name.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the id isn't a MusicBrainz id or MusicBrainz has no such artist;
    /// [`Error::Failed`] when the request or its answer fails.
    pub async fn artist(&self, id: &str) -> Result<Artist, Error> {
        if !crate::library::tags::is_mbid(id) {
            return Err(Error::NotFound);
        }
        let body = self.lookup("artist", id, "fmt=json").await?;
        serde_json::from_str(&body).map_err(|err| Error::Failed(format!("its answer: {err}")))
    }

    /// An artist's studio albums: release groups of the Album type with no secondary type
    /// (no live albums, compilations, soundtracks…), as Lidarr's standard metadata profile
    /// wants. Up to 500 release groups are looked through.
    pub async fn artist_albums(&self, artist_id: &str) -> Result<Vec<ReleaseGroup>, Error> {
        if !crate::library::tags::is_mbid(artist_id) {
            return Err(Error::NotFound);
        }
        let mut albums = Vec::new();
        for page in 0..BROWSE_PAGES {
            let offset = (page * BROWSE_PAGE).to_string();
            let limit = BROWSE_PAGE.to_string();
            let url = reqwest::Url::parse_with_params(
                &format!("{}/release-group", self.base),
                &[
                    ("artist", artist_id),
                    ("type", "album"),
                    ("limit", limit.as_str()),
                    ("offset", offset.as_str()),
                    ("fmt", "json"),
                ],
            )
            .map_err(|err| Error::Failed(err.to_string()))?;
            let body = self.get(url.as_str()).await?;
            let browse: Browse = serde_json::from_str(&body)
                .map_err(|err| Error::Failed(format!("its answer: {err}")))?;
            let got = browse.release_groups.len();
            albums.extend(browse.release_groups.into_iter().filter(|g| {
                g.primary_type.as_deref() == Some("Album")
                    && g.secondary_types.as_ref().is_none_or(Vec::is_empty)
            }));
            if got < BROWSE_PAGE || (page + 1) * BROWSE_PAGE >= browse.count {
                break;
            }
        }
        Ok(albums)
    }

    /// A recording (a song, as performed) with its artists and the releases it's on.
    pub async fn recording(&self, id: &str) -> Result<RecordingDetail, Error> {
        if !crate::library::tags::is_mbid(id) {
            return Err(Error::NotFound);
        }
        let body = self
            .lookup(
                "recording",
                id,
                "inc=artist-credits+releases+release-groups&fmt=json",
            )
            .await?;
        serde_json::from_str(&body).map_err(|err| Error::Failed(format!("its answer: {err}")))
    }

    /// The recordings with this ISRC, each with its artists and releases; none when it's
    /// unknown.
    pub async fn isrc(&self, code: &str) -> Result<Vec<RecordingDetail>, Error> {
        if !is_isrc(code) {
            return Ok(Vec::new());
        }
        let url = format!(
            "{}/isrc/{code}?inc=artist-credits+releases+release-groups&fmt=json",
            self.base
        );
        match self.get(&url).await {
            Ok(body) => serde_json::from_str::<IsrcLookup>(&body)
                .map(|found| found.recordings)
                .map_err(|err| Error::Failed(format!("its answer: {err}"))),
            Err(Error::NotFound) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    /// Recordings (songs as performed) called `title`, by `artist` when given.
    pub async fn search_songs(
        &self,
        title: &str,
        artist: Option<&str>,
    ) -> Result<Vec<RecordingDetail>, Error> {
        let mut query = format!("recording:{}", phrase(title));
        if let Some(artist) = artist {
            query.push_str(&format!(" AND artist:{}", phrase(artist)));
        }
        let found: RecordingSearch = self.find("recording", query).await?;
        Ok(found.recordings)
    }

    /// A search of one kind of entity.
    async fn find<T: serde::de::DeserializeOwned>(
        &self,
        entity: &str,
        query: String,
    ) -> Result<T, Error> {
        let url = reqwest::Url::parse_with_params(
            &format!("{}/{entity}", self.base),
            &[("query", query.as_str()), ("limit", FOUND), ("fmt", "json")],
        )
        .map_err(|err| Error::Failed(err.to_string()))?;
        let body = self.get(url.as_str()).await?;
        serde_json::from_str(&body).map_err(|err| Error::Failed(format!("its answer: {err}")))
    }

    /// One request at its turn; a redirect isn't followed (only an entity's lookup reads one).
    async fn get(&self, url: &str) -> Result<String, Error> {
        match self.fetch(url).await? {
            Fetched::Body(body) => Ok(body),
            Fetched::Moved(status, location) => Err(refused(status, url, location.as_deref())),
        }
    }

    /// One request at its turn; a busy answer is retried once after a pause, and a redirect
    /// is handed back as it came.
    async fn fetch(&self, url: &str) -> Result<Fetched, Error> {
        for attempt in 0..2 {
            self.wait_turn().await;
            let mut response = self
                .http
                .get(url)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|err| Error::Failed(err.without_url().to_string()))?;
            let status = response.status();
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                if attempt == 0 {
                    tokio::time::sleep(self.busy_wait).await;
                    continue;
                }
                break;
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(Error::NotFound);
            }
            if status.is_redirection() {
                let location = response.headers().get(reqwest::header::LOCATION);
                let location = location.and_then(|l| l.to_str().ok()).map(str::to_owned);
                return Ok(Fetched::Moved(status, location));
            }
            if !status.is_success() {
                return Err(Error::Failed(format!("answered {status}")));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|err| Error::Failed(err.without_url().to_string()))?
            {
                if body.len() + chunk.len() > MAX_BYTES {
                    return Err(Error::Failed("its answer was too large".into()));
                }
                body.extend_from_slice(&chunk);
            }
            return String::from_utf8(body)
                .map(Fetched::Body)
                .map_err(|_| Error::Failed("its answer wasn't text".into()));
        }
        Err(Error::Failed("busy; try again later".into()))
    }

    async fn wait_turn(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep(*next - now).await;
        }
        *next = Instant::now() + self.spacing;
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::extract::{Path, RawQuery, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    /// The fake's log of request paths and queries, and how many 503s to give first.
    type Fake = (Arc<Mutex<Vec<String>>>, Arc<Mutex<usize>>);

    pub const RELEASE: &str = include_str!("../tests/fixtures/musicbrainz/release.json");
    /// A symphony's release, each track a movement of one work, with its composer.
    pub const CLASSICAL: &str = include_str!("../tests/fixtures/musicbrainz/classical.json");
    pub const CLASSICAL_ID: &str = "cd3a7281-4071-43d2-b43b-8be4f64644df";
    pub const SEARCH_OK: &str = include_str!("../tests/fixtures/musicbrainz/search_ok.json");
    pub const SEARCH_AMBIGUOUS: &str =
        include_str!("../tests/fixtures/musicbrainz/search_amb.json");
    pub const RADIOHEAD: &str = "a74b1b7f-71a5-4011-9441-d0b5e4122711";
    pub const OK_COMPUTER: &str = "b1392450-e666-3926-a536-22c65f834433";
    pub const PARANOID_ANDROID: &str = "4d4e5f6a-0b1c-4d2e-8f3a-5b6c7d8e9f01";
    pub const PARANOID_ISRC: &str = "GBAYE9700245";
    /// An OK Computer edition MusicBrainz merged into the one the release fixture is: looking it
    /// up answers 301 to the survivor, over plain http, as MusicBrainz does.
    pub const MERGED: &str = "0f0e0d0c-0b0a-4908-8706-050403020100";
    /// The edition the release fixture is, which `MERGED` joined.
    pub const SURVIVOR: &str = "fba5f8fe-c6c8-4511-8562-c9febf482674";
    /// Merged into `MERGED`, itself merged: two moves in a row.
    pub const MERGED_TWICE: &str = "1f1e1d1c-1b1a-4918-8716-151413121110";
    /// Answers 301 to the survivor's path on another host.
    pub const MOVED_AWAY: &str = "2f2e2d2c-2b2a-4928-8726-252423222120";
    /// Answers 301 to an artist, not a release.
    pub const MOVED_ASIDE: &str = "3f3e3d3c-3b3a-4938-8736-353433323130";
    /// An artist merged into Radiohead.
    pub const MERGED_ARTIST: &str = "4f4e4d4c-4b4a-4948-8746-454443424140";

    /// Where the fake says an id moved, if it did.
    fn moved(kind: &str, id: &str) -> Option<String> {
        let to = |host: &str, path: String| format!("{host}{path}?fmt=json&inc=secret");
        let mb = "http://musicbrainz.org";
        match (kind, id) {
            ("release", MERGED) => Some(to(mb, format!("/ws/2/release/{SURVIVOR}"))),
            ("release", MERGED_TWICE) => Some(to(mb, format!("/ws/2/release/{MERGED}"))),
            ("release", MOVED_AWAY) => Some(to(
                "https://mirror.example",
                format!("/ws/2/release/{SURVIVOR}"),
            )),
            ("release", MOVED_ASIDE) => Some(to(mb, format!("/ws/2/artist/{RADIOHEAD}"))),
            ("artist", MERGED_ARTIST) => Some(to(mb, format!("/ws/2/artist/{RADIOHEAD}"))),
            _ => None,
        }
    }

    /// A 301 to `location`.
    fn redirect(location: String) -> axum::response::Response {
        let header = [(axum::http::header::LOCATION, location)];
        (StatusCode::MOVED_PERMANENTLY, header, "").into_response()
    }

    /// A fake MusicBrainz: OK Computer's editions (as the search fixture lists them) are the
    /// release fixture; a search naming "Greatest Hits"
    /// gets Queen's two release groups, "Nothing" gets nothing, others get OK Computer's.
    pub async fn start(busy_first: usize) -> (super::Client, Arc<Mutex<Vec<String>>>) {
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let state = (log.clone(), Arc::new(Mutex::new(busy_first)));
        let router = axum::Router::new()
            .route(
                "/release/{id}",
                axum::routing::get(
                    |State((log, busy)): State<Fake>, Path(id): Path<String>| async move {
                        log.lock().unwrap().push(format!("release {id}"));
                        if take_busy(&busy) {
                            return (StatusCode::SERVICE_UNAVAILABLE, "busy".to_owned()).into_response();
                        }
                        if let Some(location) = moved("release", &id) {
                            return redirect(location);
                        }
                        // Any OK Computer edition the search fixture lists.
                        if super::testing::SEARCH_OK.contains(&id) {
                            (StatusCode::OK, super::testing::RELEASE.to_owned()).into_response()
                        } else if id == super::testing::CLASSICAL_ID {
                            (StatusCode::OK, super::testing::CLASSICAL.to_owned()).into_response()
                        } else {
                            (StatusCode::NOT_FOUND, r#"{"error":"Not Found"}"#.to_owned()).into_response()
                        }
                    },
                ),
            )
            .route(
                "/release",
                axum::routing::get(
                    |State((log, busy)): State<Fake>, RawQuery(query): RawQuery| async move {
                        let query = query.unwrap_or_default();
                        let decoded = reqwest::Url::parse(&format!("http://x/?{query}"))
                            .unwrap()
                            .query_pairs()
                            .find(|(k, _)| k == "query")
                            .map(|(_, v)| v.into_owned())
                            .unwrap_or_default();
                        log.lock().unwrap().push(format!("search {decoded}"));
                        if take_busy(&busy) {
                            return (StatusCode::SERVICE_UNAVAILABLE, "busy".to_owned())
                                .into_response();
                        }
                        let body = if decoded.contains("Greatest Hits") {
                            super::testing::SEARCH_AMBIGUOUS
                        } else if decoded.contains("Nothing") {
                            r#"{"count":0,"releases":[]}"#
                        } else {
                            super::testing::SEARCH_OK
                        };
                        (StatusCode::OK, body.to_owned()).into_response()
                    },
                ),
            )
            // Radiohead, OK Computer and Paranoid Android, for requests; others are
            // unknown.
            .route(
                "/release-group/{id}",
                axum::routing::get(
                    |State((log, _)): State<Fake>, Path(id): Path<String>| async move {
                        log.lock().unwrap().push(format!("release-group {id}"));
                        if id != super::testing::OK_COMPUTER {
                            return (StatusCode::NOT_FOUND, r#"{"error":"Not Found"}"#.to_owned());
                        }
                        (StatusCode::OK, serde_json::json!({
                            "id": id, "title": "OK Computer", "primary-type": "Album",
                            "secondary-types": [], "first-release-date": "1997-05-21",
                            "artist-credit": [{"name": "Radiohead", "joinphrase": "",
                                "artist": {"id": super::testing::RADIOHEAD, "name": "Radiohead"}}],
                        }).to_string())
                    },
                ),
            )
            .route(
                "/artist/{id}",
                axum::routing::get(
                    |State((log, _)): State<Fake>, Path(id): Path<String>| async move {
                        log.lock().unwrap().push(format!("artist {id}"));
                        if let Some(location) = moved("artist", &id) {
                            return redirect(location);
                        }
                        if id != super::testing::RADIOHEAD {
                            return (StatusCode::NOT_FOUND, r#"{"error":"Not Found"}"#.to_owned()).into_response();
                        }
                        (StatusCode::OK, serde_json::json!({"id": id, "name": "Radiohead"}).to_string()).into_response()
                    },
                ),
            )
            .route(
                "/artist",
                axum::routing::get(
                    |State((log, _)): State<Fake>, RawQuery(query): RawQuery| async move {
                        log.lock().unwrap().push(format!("search artist {}", query.unwrap_or_default()));
                        serde_json::json!({"artists": [
                            {"id": super::testing::RADIOHEAD, "name": "Radiohead", "type": "Group",
                             "country": "GB", "disambiguation": "", "score": 100},
                            {"id": "0a0b0c0d-1e2f-4a5b-8c6d-7e8f9a0b1c2d", "name": "Radiohead", "type": "Person",
                             "disambiguation": "a tribute act", "score": 62},
                        ]})
                        .to_string()
                    },
                ),
            )
            .route(
                "/isrc/{code}",
                axum::routing::get(
                    |State((log, _)): State<Fake>, Path(code): Path<String>| async move {
                        log.lock().unwrap().push(format!("isrc {code}"));
                        if code != super::testing::PARANOID_ISRC {
                            return (StatusCode::NOT_FOUND, r#"{"error":"Not Found"}"#.to_owned());
                        }
                        (StatusCode::OK, serde_json::json!({"isrc": code, "recordings": [{
                            "id": super::testing::PARANOID_ANDROID, "title": "Paranoid Android", "length": 383000,
                            "artist-credit": [{"name": "Radiohead", "joinphrase": "",
                                "artist": {"id": super::testing::RADIOHEAD, "name": "Radiohead"}}],
                        }]}).to_string())
                    },
                ),
            )
            .route(
                "/recording",
                axum::routing::get(
                    |State((log, _)): State<Fake>, RawQuery(query): RawQuery| async move {
                        log.lock().unwrap().push(format!("search recording {}", query.unwrap_or_default()));
                        serde_json::json!({"recordings": [{
                            "id": super::testing::PARANOID_ANDROID, "title": "Paranoid Android", "length": 383000,
                            "score": 100,
                            "artist-credit": [{"name": "Radiohead", "joinphrase": "",
                                "artist": {"id": super::testing::RADIOHEAD, "name": "Radiohead"}}],
                            "releases": [{"title": "OK Computer", "status": "Official",
                                "release-group": {"id": super::testing::OK_COMPUTER, "title": "OK Computer",
                                    "primary-type": "Album"}}],
                        }]})
                        .to_string()
                    },
                ),
            )
            .route(
                "/release-group",
                axum::routing::get(
                    |State((log, _)): State<Fake>, RawQuery(query): RawQuery| async move {
                        let query = query.unwrap_or_default();
                        if query.contains("query=") {
                            log.lock().unwrap().push(format!("search release-group {query}"));
                            return (StatusCode::OK, serde_json::json!({"release-groups": [{
                                "id": super::testing::OK_COMPUTER, "title": "OK Computer", "score": 100,
                                "primary-type": "Album", "secondary-types": [],
                                "first-release-date": "1997-05-21",
                                "artist-credit": [{"name": "Radiohead", "joinphrase": "",
                                    "artist": {"id": super::testing::RADIOHEAD, "name": "Radiohead"}}],
                            }]}).to_string());
                        }
                        log.lock().unwrap().push(format!("browse {query}"));
                        let group = |id: &str, title: &str, primary: &str, secondary: &[&str], date: &str| {
                            serde_json::json!({"id": id, "title": title, "primary-type": primary,
                                "secondary-types": secondary, "first-release-date": date})
                        };
                        (StatusCode::OK, serde_json::json!({
                            "release-group-count": 4,
                            "release-groups": [
                                group(super::testing::OK_COMPUTER, "OK Computer", "Album", &[], "1997-05-21"),
                                group("2a0981fb-9593-3019-864b-ce934d97a16e", "Kid A", "Album", &[], "2000-10-02"),
                                group("3c7b8d16-60f6-4f8a-8a5b-2a0f9b1c4e21", "I Might Be Wrong", "Album", &["Live"], "2001-11-12"),
                                group("4d4e5f6a-0b1c-4d2e-8f3a-5b6c7d8e9f02", "Creep", "Single", &[], "1992-09-21"),
                            ],
                        }).to_string())
                    },
                ),
            )
            .route(
                "/recording/{id}",
                axum::routing::get(
                    |State((log, _)): State<Fake>, Path(id): Path<String>| async move {
                        log.lock().unwrap().push(format!("recording {id}"));
                        if id != super::testing::PARANOID_ANDROID {
                            return (StatusCode::NOT_FOUND, r#"{"error":"Not Found"}"#.to_owned());
                        }
                        (StatusCode::OK, serde_json::json!({
                            "id": id, "title": "Paranoid Android", "length": 383000,
                            "artist-credit": [{"name": "Radiohead", "joinphrase": "",
                                "artist": {"id": super::testing::RADIOHEAD, "name": "Radiohead"}}],
                            "releases": [{"title": "OK Computer", "status": "Official",
                                "release-group": {"id": super::testing::OK_COMPUTER, "title": "OK Computer",
                                    "primary-type": "Album", "first-release-date": "1997-05-21"}}],
                        }).to_string())
                    },
                ),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client =
            super::Client::with_base(base, Duration::from_millis(1), Duration::from_millis(1))
                .unwrap();
        (client, log)
    }

    fn take_busy(busy: &Mutex<usize>) -> bool {
        let mut left = busy.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn looks_up_albums_artists_and_recordings_for_requests() {
        let (client, log) = testing::start(0).await;
        let group = client.release_group(testing::OK_COMPUTER).await.unwrap();
        assert_eq!(
            (
                group.title.as_str(),
                group.artist_credit[0].artist.name.as_str()
            ),
            ("OK Computer", "Radiohead")
        );
        assert_eq!(
            client.artist(testing::RADIOHEAD).await.unwrap().name,
            "Radiohead"
        );
        let albums: Vec<String> = client
            .artist_albums(testing::RADIOHEAD)
            .await
            .unwrap()
            .into_iter()
            .filter_map(|g| g.title)
            .collect();
        assert_eq!(
            albums,
            ["OK Computer", "Kid A"],
            "no live albums or singles"
        );
        let song = client.recording(testing::PARANOID_ANDROID).await.unwrap();
        assert_eq!(
            (song.title.as_str(), song.length),
            ("Paranoid Android", Some(383000))
        );
        assert!(matches!(
            client.artist("not-an-id").await,
            Err(Error::NotFound)
        ));
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|l| l.starts_with("browse ") && l.contains("type=album"))
        );
    }

    #[tokio::test]
    async fn reads_the_works_recordings_perform_and_their_parts() {
        let (client, _) = testing::start(0).await;
        let release = client.release(testing::CLASSICAL_ID).await.unwrap();
        let performed: Vec<Performed> = release.media[0]
            .tracks
            .iter()
            .map(|t| t.recording.performed().unwrap())
            .collect();
        assert_eq!(performed.len(), 4);
        let first = &performed[0];
        assert_eq!(first.work_title, "Symphony no. 5 in C minor, op. 67");
        assert_eq!(
            (first.movement.as_deref(), first.movement_number),
            (Some("Allegro con brio"), Some(1))
        );
        assert_eq!(first.composers[0].name, "Ludwig van Beethoven");
        // All four are parts of one work; a medley the second is in isn't what it's part of.
        assert!(performed.iter().all(|p| p.work_id == first.work_id));
        assert_eq!(
            performed
                .iter()
                .map(|p| (p.movement.clone().unwrap(), p.movement_number.unwrap()))
                .collect::<Vec<_>>(),
            [
                ("Allegro con brio".to_owned(), 1),
                ("Andante con moto".to_owned(), 2),
                ("Scherzo. Allegro".to_owned(), 3),
                ("Allegro".to_owned(), 4)
            ]
        );
        // OK Computer's recordings carry no works.
        let rock = client
            .release("fba5f8fe-c6c8-4511-8562-c9febf482674")
            .await
            .unwrap();
        assert_eq!(rock.media[0].tracks[0].recording.performed(), None);
    }

    #[test]
    fn a_parts_name_is_its_own() {
        let whole = "Symphony no. 5 in C minor, op. 67";
        for (title, number, name) in [
            (
                "Symphony no. 5 in C minor, op. 67: I. Allegro con brio",
                Some(1),
                "Allegro con brio",
            ),
            (
                "SYMPHONY NO. 5 IN C MINOR, OP. 67: IV. Allegro",
                Some(4),
                "Allegro",
            ),
            // A numeral that isn't its number stays.
            (
                "Symphony no. 5 in C minor, op. 67: II. Andante",
                Some(3),
                "II. Andante",
            ),
            (
                "Symphony no. 5 in C minor, op. 67, 12. Finale",
                Some(12),
                "Finale",
            ),
            ("Ouverture", None, "Ouverture"),
            (
                "Symphony no. 5 in C minor, op. 67",
                Some(1),
                "Symphony no. 5 in C minor, op. 67",
            ),
        ] {
            assert_eq!(part_name(title, whole, number), name, "{title}");
        }
        assert_eq!(
            (roman(14), roman(49), roman(1999)),
            ("XIV".into(), "XLIX".into(), "MCMXCIX".into())
        );
    }

    #[tokio::test]
    async fn looks_up_releases_with_their_tracks() {
        let (client, _) = testing::start(0).await;
        let release = client
            .release("fba5f8fe-c6c8-4511-8562-c9febf482674")
            .await
            .unwrap();
        assert_eq!(release.title, "OK Computer");
        assert_eq!(
            release.release_group.id,
            "b1392450-e666-3926-a536-22c65f834433"
        );
        assert_eq!(credit_line(&release.artist_credit), "Radiohead");
        assert_eq!(release.media.len(), 1);
        assert_eq!(release.media[0].tracks.len(), 12);
        assert_eq!(release.media[0].tracks[0].title, "Airbag");
        assert!(matches!(
            client.release("00000000-0000-0000-0000-000000000000").await,
            Err(Error::NotFound)
        ));
        assert!(matches!(
            client.release("not-an-id").await,
            Err(Error::NotFound)
        ));
    }

    #[tokio::test]
    async fn searches_with_quoted_phrases_and_retries_once_when_busy() {
        let (client, log) = testing::start(1).await;
        let found = client
            .search("OK \"Computer\"", Some("Radiohead"))
            .await
            .unwrap();
        assert_eq!(found.len(), 10);
        assert!(
            found
                .iter()
                .all(|f| f.release_group.id.starts_with("b1392450"))
        );
        let log = log.lock().unwrap().clone();
        assert_eq!(log.len(), 2, "one busy answer, one retry: {log:?}");
        assert_eq!(
            log[1],
            r#"search release:"OK \"Computer\"" AND artist:"Radiohead""#
        );
    }

    #[tokio::test]
    async fn a_merged_id_is_looked_up_again_as_the_one_it_joined() {
        let (client, log) = testing::start(0).await;
        // The fake answers 301 to an http:// address on musicbrainz.org: never asked as given,
        // the survivor is asked for on the client's own address, with the same inc.
        let release = client.release(testing::MERGED).await.unwrap();
        assert_eq!(release.id, testing::SURVIVOR);
        assert_eq!(
            *log.lock().unwrap(),
            [
                format!("release {}", testing::MERGED),
                format!("release {}", testing::SURVIVOR)
            ]
        );
        let artist = client.artist(testing::MERGED_ARTIST).await.unwrap();
        assert_eq!(artist.id, testing::RADIOHEAD);
    }

    #[tokio::test]
    async fn a_move_elsewhere_or_twice_is_refused_and_says_where() {
        let (client, log) = testing::start(0).await;
        let err = client.release(testing::MOVED_AWAY).await.unwrap_err();
        let said = err.to_string();
        assert!(said.contains("301"), "{said}");
        assert!(
            said.contains(&format!(
                "https://mirror.example/ws/2/release/{}",
                testing::SURVIVOR
            )),
            "{said}"
        );
        assert!(!said.contains("secret"), "never the query: {said}");
        // Another kind of entity isn't the one asked for.
        let said = client.release(testing::MOVED_ASIDE).await.unwrap_err();
        assert!(said.to_string().contains("/ws/2/artist/"), "{said}");
        // One move is followed, not two.
        let said = client.release(testing::MERGED_TWICE).await.unwrap_err();
        assert!(said.to_string().contains("one move"), "{said}");
        let asked: Vec<String> = log.lock().unwrap().clone();
        assert_eq!(
            asked,
            [
                format!("release {}", testing::MOVED_AWAY),
                format!("release {}", testing::MOVED_ASIDE),
                format!("release {}", testing::MERGED_TWICE),
                format!("release {}", testing::MERGED),
            ]
        );
    }

    #[tokio::test]
    async fn searches_use_the_documented_address_without_a_trailing_slash() {
        let (client, log) = testing::start(0).await;
        // The fake only answers searches at /release, /artist, /release-group and /recording.
        assert!(!client.search("OK Computer", None).await.unwrap().is_empty());
        assert!(!client.find_artists("Radiohead").await.unwrap().is_empty());
        assert!(
            !client
                .find_albums("OK Computer", None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !client
                .search_songs("Airbag", None)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !client
                .artist_albums(testing::RADIOHEAD)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(log.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn gives_up_after_two_busy_answers() {
        let (client, _) = testing::start(2).await;
        let err = client.search("x", None).await.unwrap_err();
        assert!(err.to_string().contains("busy"));
    }

    #[tokio::test]
    async fn requests_wait_their_turn() {
        let (fake, _) = testing::start(0).await;
        let client = Client::with_base(
            fake.base.clone(),
            Duration::from_millis(150),
            Duration::from_millis(1),
        )
        .unwrap();
        let started = Instant::now();
        client.search("a", None).await.unwrap();
        client.search("b", None).await.unwrap();
        client.search("c", None).await.unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "{:?}",
            started.elapsed()
        );
    }
}
