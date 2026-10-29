//! LRCLIB (lrclib.net), for synced lyrics of imported albums' tracks. Off unless
//! `MELETE_LYRICS=lrclib`: each track's title, artist, album and length leave
//! the server, so asking is a choice, not a default. No key and no account; the User-Agent
//! names only Melete and its version, and redirects aren't followed.

use std::time::Duration;

use serde::Deserialize;

const BASE: &str = "https://lrclib.net";
/// Lyrics are text; anything this big isn't.
const MAX_BYTES: usize = 1024 * 1024;

/// An LRCLIB client: no key, and redirects aren't followed.
pub struct Client {
    http: reqwest::Client,
    base: String,
}

/// The track asked about.
pub struct Track<'a> {
    /// The track's title.
    pub title: &'a str,
    /// Its artist.
    pub artist: &'a str,
    /// Its album's title.
    pub album: &'a str,
    /// LRCLIB matches the length within a couple of seconds.
    pub seconds: u64,
}

/// What asking for a track's synced lyrics gave.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// LRC text: lines starting with their time ("[01:02.34]").
    Synced(String),
    /// Not known, instrumental, or only plain lyrics.
    None,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    #[serde(default)]
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// What LRCLIB knows of a track's lyrics: none of either for one it doesn't know or that's
/// instrumental.
#[derive(Debug, Default, PartialEq)]
pub struct Known {
    /// LRC text, as [`Found::Synced`] holds it.
    pub synced: Option<String>,
    /// Plain lyrics, with Unix line ends.
    pub plain: Option<String>,
}

impl Client {
    /// A client for lrclib.net.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_base(BASE.into())
    }

    /// A client for another address, for tests.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn with_base(base: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("Melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, base })
    }

    /// The track's synced lyrics, when LRCLIB has them.
    pub async fn synced(&self, track: &Track<'_>) -> anyhow::Result<Found> {
        Ok(self
            .lyrics(track)
            .await?
            .synced
            .map_or(Found::None, Found::Synced))
    }

    /// The track's lyrics, synced and plain, as far as LRCLIB has them.
    pub async fn lyrics(&self, track: &Track<'_>) -> anyhow::Result<Known> {
        let seconds = track.seconds.to_string();
        let mut response = self
            .http
            .get(format!("{}/api/get", self.base))
            .query(&[
                ("track_name", track.title),
                ("artist_name", track.artist),
                ("album_name", track.album),
                ("duration", &seconds),
            ])
            .send()
            .await
            .map_err(|err| anyhow::anyhow!("LRCLIB: {}", err.without_url()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Known::default());
        }
        anyhow::ensure!(
            response.status().is_success(),
            "LRCLIB answered {}",
            response.status()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| anyhow::anyhow!("LRCLIB: {}", err.without_url()))?
        {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= MAX_BYTES,
                "LRCLIB's answer is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let answer: Answer = serde_json::from_slice(&bytes)
            .map_err(|err| anyhow::anyhow!("LRCLIB's answer: {err}"))?;
        if answer.instrumental {
            return Ok(Known::default());
        }
        Ok(Known {
            synced: answer.synced_lyrics.and_then(|text| lrc(&text)),
            plain: answer
                .plain_lyrics
                .map(|text| text.replace("\r\n", "\n").trim().to_owned())
                .filter(|text| !text.is_empty()),
        })
    }
}

/// `text` as LRC with Unix line ends, or None when it isn't: every line must be empty or
/// start with a bracketed tag, and at least one must carry a time.
pub fn lrc(text: &str) -> Option<String> {
    let text = text.replace("\r\n", "\n");
    let timed = |line: &str| {
        let Some(rest) = line.strip_prefix('[') else {
            return false;
        };
        let (minutes, rest) = rest.split_once(':').unwrap_or_default();
        let seconds = rest.split(']').next().unwrap_or_default();
        !minutes.is_empty()
            && minutes.bytes().all(|b| b.is_ascii_digit())
            && seconds.len() >= 2
            && seconds.bytes().all(|b| b.is_ascii_digit() || b == b'.')
    };
    let lines_ok = text
        .lines()
        .all(|line| line.trim().is_empty() || line.starts_with('['));
    (lines_ok && text.lines().any(timed)).then(|| {
        let mut text = text.trim_end().to_owned();
        text.push('\n');
        text
    })
}

#[cfg(test)]
pub mod testing {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Query, State};
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    /// Each request's query, in order.
    pub type Log = Arc<Mutex<Vec<HashMap<String, String>>>>;

    pub const AIRBAG: &str =
        "[00:01.00] In the next world war\n[00:03.50] In a jackknifed juggernaut\n";

    /// A fake LRCLIB: "Airbag" has synced lyrics, "Fitter Happier" is instrumental, others
    /// aren't known. Each request's query is logged.
    pub async fn start() -> (super::Client, Log) {
        let log: Log = Arc::default();
        let router = axum::Router::new()
            .route(
                "/api/get",
                axum::routing::get(
                    |State(log): State<Log>,
                     Query(query): Query<HashMap<String, String>>| async move {
                        let title = query.get("track_name").cloned().unwrap_or_default();
                        log.lock().unwrap().push(query);
                        match title.as_str() {
                            "Airbag" => axum::Json(serde_json::json!({
                                "id": 1, "trackName": "Airbag", "instrumental": false,
                                "plainLyrics": "In the next world war",
                                "syncedLyrics": AIRBAG.replace('\n', "\r\n"),
                            }))
                            .into_response(),
                            "No Surprises" => axum::Json(serde_json::json!({
                                "id": 3, "instrumental": false,
                                "plainLyrics": "A heart that's full up like a landfill\r\nA job that slowly kills you",
                                "syncedLyrics": null,
                            }))
                            .into_response(),
                            "Fitter Happier" => axum::Json(serde_json::json!({
                                "id": 2, "instrumental": true, "syncedLyrics": null,
                            }))
                            .into_response(),
                            _ => StatusCode::NOT_FOUND.into_response(),
                        }
                    },
                ),
            )
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (super::Client::with_base(base).unwrap(), log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str) -> Track<'_> {
        Track {
            title,
            artist: "Radiohead",
            album: "OK Computer",
            seconds: 284,
        }
    }

    #[tokio::test]
    async fn asks_with_only_the_track_and_takes_synced_lyrics() {
        let (client, log) = testing::start().await;
        assert_eq!(
            client.synced(&track("Airbag")).await.unwrap(),
            Found::Synced(testing::AIRBAG.into())
        );
        let asked = log.lock().unwrap()[0].clone();
        let mut keys: Vec<&String> = asked.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["album_name", "artist_name", "duration", "track_name"],
            "nothing else leaves the server"
        );
        assert_eq!(asked["duration"], "284");
        assert_eq!(asked["artist_name"], "Radiohead");
        assert_eq!(
            client.synced(&track("Fitter Happier")).await.unwrap(),
            Found::None
        );
        assert_eq!(client.synced(&track("Lucky")).await.unwrap(), Found::None);
    }

    #[test]
    fn only_lrc_counts_as_synced_lyrics() {
        assert_eq!(
            lrc("[ar: Radiohead]\r\n[00:01.00] One\r\n\r\n[00:02.5] Two\r\n").as_deref(),
            Some("[ar: Radiohead]\n[00:01.00] One\n\n[00:02.5] Two\n")
        );
        assert_eq!(lrc("One\nTwo"), None, "plain lyrics");
        assert_eq!(lrc("[ar: Radiohead]\n"), None, "no times");
        assert_eq!(lrc("[00:01.00] One\nTwo"), None, "an untimed line");
        assert_eq!(lrc("<html>"), None);
    }
}
