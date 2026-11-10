//! ListenBrainz, MetaBrainz's open listening data, for each listener's For You list: on only
//! with `MELETE_LISTENBRAINZ` (and MusicBrainz). Three things are asked, each by MusicBrainz ids
//! alone: the recordings listened to alongside a recording, and the artists alongside an artist
//! (both from ListenBrainz Labs, labs.api.listenbrainz.org), and the artists and album of
//! recordings (api.listenbrainz.org). Never a name, an account, play counts or titles. Every
//! request goes over https to those two hosts only, redirects included, with Melete's usual
//! User-Agent, a timeout and a size cap; no account or token is used.
//!
//! Requests keep to one a second, all of them together, each taking the next free turn. A 429
//! or 503 holds every request back for as long as its `Retry-After` says (or [`BUSY_WAIT`]), and
//! an answer saying no requests are left (`X-RateLimit-Remaining: 0`) for as long as its
//! `X-RateLimit-Reset-In` says. A hold up to [`MAX_WAIT`] is waited out and a refused request
//! tried once more; past that the request isn't sent ([`Busy`]).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const LABS: &str = "https://labs.api.listenbrainz.org";
const API: &str = "https://api.listenbrainz.org";
/// The algorithm Labs' similar recordings are asked for by. Labs names each by how it was
/// worked out, and these names aren't a stable API: one may be retired or renamed, and
/// ListenBrainz then answers 400. Kept here alone so it's changed in one place.
pub const RECORDINGS_ALGORITHM: &str =
    "session_based_days_9000_session_300_contribution_5_threshold_15_limit_50_skip_30";
/// The algorithm Labs' similar artists are asked for by; as unstable as
/// [`RECORDINGS_ALGORITHM`].
pub const ARTISTS_ALGORITHM: &str =
    "session_based_days_7500_session_300_contribution_5_threshold_10_limit_100_filter_True_skip_30";
/// The most recordings one metadata request asks about.
pub const METADATA_BATCH: usize = 50;
/// The largest answer read; 100 similar recordings, or 50 recordings' metadata, are a few
/// hundred kilobytes.
const MAX_ANSWER: usize = 4 * 1024 * 1024;
/// The time between two requests' turns.
const SPACING: Duration = Duration::from_secs(1);
/// The longest a request waits for its turn, or for a hold to be over; past that it isn't sent.
/// Lists are made in the background, so this can be longer than a page would wait.
pub const MAX_WAIT: Duration = Duration::from_secs(60);
/// How long a 429 or 503 that doesn't say holds requests back.
const BUSY_WAIT: Duration = Duration::from_secs(60);
/// The longest hold kept, however long ListenBrainz asks for.
const MOST_HELD: Duration = Duration::from_secs(24 * 3600);

/// ListenBrainz asked for a pause, or too many requests are waiting their turn, so nothing was
/// sent: worth asking again later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ListenBrainz asked for a pause, or is being asked too much at once")
    }
}

impl std::error::Error for Busy {}

/// A recording listened to alongside the one asked about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarRecording {
    /// Its MusicBrainz id.
    pub recording_mbid: String,
    /// Its title.
    #[serde(default)]
    pub recording_name: String,
    /// Its artists, as one line.
    #[serde(default)]
    pub artist_credit_name: String,
    /// The release it's listed from, by name.
    pub release_name: Option<String>,
    /// That release's MusicBrainz id.
    pub release_mbid: Option<String>,
    /// How strongly it goes with the one asked about: higher is closer, on a scale of the
    /// answer's own.
    #[serde(default)]
    pub score: f64,
}

/// An artist listened to alongside the one asked about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimilarArtist {
    /// Their MusicBrainz id.
    pub artist_mbid: String,
    /// Their name.
    #[serde(default)]
    pub name: String,
    /// How strongly they go with the one asked about, on a scale of the answer's own.
    #[serde(default)]
    pub score: f64,
}

/// What ListenBrainz knows of a recording: its artists and the album it's listed from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// Its credited artists' MusicBrainz ids, in order.
    pub artist_mbids: Vec<String>,
    /// Its first credited artist's name.
    pub artist: Option<String>,
    /// The release it's listed from, by name.
    pub release: Option<String>,
    /// That release's release group (its album), for its cover.
    pub release_group_mbid: Option<String>,
}

#[derive(Deserialize)]
struct Metadata {
    artist: Option<MetaArtist>,
    release: Option<MetaRelease>,
}

#[derive(Deserialize)]
struct MetaArtist {
    #[serde(default)]
    artists: Vec<MetaCredit>,
}

#[derive(Deserialize)]
struct MetaCredit {
    artist_mbid: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct MetaRelease {
    name: Option<String>,
    release_group_mbid: Option<String>,
}

/// When requests may go.
#[derive(Debug)]
struct Turns {
    /// The next free turn.
    next: Instant,
    /// Until when ListenBrainz asked for a pause.
    held_until: Option<Instant>,
}

/// A ListenBrainz client that follows redirects only to ListenBrainz's two hosts over https.
pub struct Client {
    http: crate::proxy::Http,
    labs: String,
    api: String,
    turns: Mutex<Turns>,
    spacing: Duration,
}

/// One of ListenBrainz's two hosts, over https.
fn allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str().unwrap_or_default(),
            "labs.api.listenbrainz.org" | "api.listenbrainz.org"
        )
}

impl Client {
    /// A client for labs.api.listenbrainz.org and api.listenbrainz.org.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_bases(LABS.into(), API.into(), false)
    }

    /// A client for other addresses (tests' fakes). `any_host` also lets redirects go anywhere,
    /// over plain http; without it they're held to ListenBrainz's hosts over https.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn with_bases(labs: String, api: String, any_host: bool) -> anyhow::Result<Self> {
        let policy = move || {
            reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 {
                    attempt.error("too many redirects")
                } else if any_host || allowed(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("redirected somewhere other than ListenBrainz")
                }
            })
        };
        let http = crate::proxy::Http::new(crate::proxy::Service::ListenBrainz, move |b| {
            b.timeout(Duration::from_secs(30))
                .redirect(policy())
                .user_agent(concat!("Melete/", env!("CARGO_PKG_VERSION")))
        })?;
        Ok(Self {
            http,
            labs,
            api,
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

    /// Whether ListenBrainz asked for a pause longer than [`MAX_WAIT`] that isn't over, so every
    /// request would fail [`Busy`] now.
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

    /// The recordings listened to alongside `recording` (a MusicBrainz id), closest first.
    ///
    /// # Errors
    ///
    /// When `recording` isn't a MusicBrainz id, ListenBrainz doesn't answer or answers
    /// something else, or [`Busy`].
    pub async fn similar_recordings(
        &self,
        recording: &str,
    ) -> anyhow::Result<Vec<SimilarRecording>> {
        anyhow::ensure!(
            crate::library::tags::is_mbid(recording),
            "not a MusicBrainz id"
        );
        let url = reqwest::Url::parse_with_params(
            &format!("{}/similar-recordings/json", self.labs),
            &[
                ("recording_mbids", recording),
                ("algorithm", RECORDINGS_ALGORITHM),
            ],
        )?;
        let mut found: Vec<SimilarRecording> = self.json(url).await?;
        found.retain(|r| crate::library::tags::is_mbid(&r.recording_mbid));
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(found)
    }

    /// The artists listened to alongside `artist` (a MusicBrainz id), closest first.
    ///
    /// # Errors
    ///
    /// As [`similar_recordings`](Self::similar_recordings).
    pub async fn similar_artists(&self, artist: &str) -> anyhow::Result<Vec<SimilarArtist>> {
        anyhow::ensure!(
            crate::library::tags::is_mbid(artist),
            "not a MusicBrainz id"
        );
        let url = reqwest::Url::parse_with_params(
            &format!("{}/similar-artists/json", self.labs),
            &[("artist_mbids", artist), ("algorithm", ARTISTS_ALGORITHM)],
        )?;
        let mut found: Vec<SimilarArtist> = self.json(url).await?;
        found.retain(|a| crate::library::tags::is_mbid(&a.artist_mbid) && a.artist_mbid != artist);
        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(found)
    }

    /// The artists and album of each of `recordings` (MusicBrainz ids, at most
    /// [`METADATA_BATCH`]) that ListenBrainz knows, by id.
    ///
    /// # Errors
    ///
    /// When there are too many or one isn't a MusicBrainz id, or as
    /// [`similar_recordings`](Self::similar_recordings).
    pub async fn recordings(
        &self,
        recordings: &[String],
    ) -> anyhow::Result<HashMap<String, Recording>> {
        anyhow::ensure!(
            recordings.len() <= METADATA_BATCH
                && recordings.iter().all(|r| crate::library::tags::is_mbid(r)),
            "at most {METADATA_BATCH} MusicBrainz ids"
        );
        if recordings.is_empty() {
            return Ok(HashMap::new());
        }
        let url = reqwest::Url::parse_with_params(
            &format!("{}/1/metadata/recording/", self.api),
            &[
                ("recording_mbids", recordings.join(",").as_str()),
                ("inc", "artist release"),
            ],
        )?;
        let answer: HashMap<String, Metadata> = self.json(url).await?;
        Ok(answer
            .into_iter()
            .filter(|(id, _)| recordings.contains(id))
            .map(|(id, meta)| {
                let credits = meta.artist.map(|a| a.artists).unwrap_or_default();
                let recording = Recording {
                    artist_mbids: credits
                        .iter()
                        .filter_map(|c| c.artist_mbid.clone())
                        .filter(|m| crate::library::tags::is_mbid(m))
                        .collect(),
                    artist: credits.first().and_then(|c| c.name.clone()),
                    release: meta.release.as_ref().and_then(|r| r.name.clone()),
                    release_group_mbid: meta
                        .release
                        .and_then(|r| r.release_group_mbid)
                        .filter(|g| crate::library::tags::is_mbid(g)),
                };
                (id, recording)
            })
            .collect())
    }

    /// One request's answer read as JSON, at its turn ([`turn`](Self::turn)). A 429 or 503
    /// holds requests back ([`hold`](Self::hold)) and is tried once more after a short hold;
    /// [`Busy`] when the hold is longer, or the second try is refused too. An answer saying no
    /// requests are left holds the next ones back until the limit resets. Errors never carry
    /// the address.
    async fn json<T: serde::de::DeserializeOwned>(&self, url: reqwest::Url) -> anyhow::Result<T> {
        let failed = |err: reqwest::Error| anyhow::anyhow!("ListenBrainz: {}", err.without_url());
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
            let wait = number(&response, reqwest::header::RETRY_AFTER.as_str())
                .or_else(|| number(&response, "x-ratelimit-reset-in"))
                .map_or(BUSY_WAIT, Duration::from_secs);
            self.hold(wait);
            tries += 1;
            if tries > 1 || wait > MAX_WAIT {
                tracing::warn!(
                    status = status.as_u16(),
                    wait_seconds = wait.min(MOST_HELD).as_secs(),
                    "ListenBrainz asked for a pause"
                );
                return Err(Busy.into());
            }
        };
        if number(&response, "x-ratelimit-remaining") == Some(0)
            && let Some(wait) = number(&response, "x-ratelimit-reset-in")
        {
            self.hold(Duration::from_secs(wait));
        }
        anyhow::ensure!(
            response.status().is_success(),
            "ListenBrainz answered {}",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= MAX_ANSWER,
                "ListenBrainz's answer is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes)
            .map_err(|err| anyhow::anyhow!("ListenBrainz's answer: {err}"))
    }
}

/// A header's whole number: a wait in seconds, or the requests left. None when it's missing or
/// written some other way (`Retry-After` as a date, say).
fn number(response: &reqwest::Response, name: &str) -> Option<u64> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
