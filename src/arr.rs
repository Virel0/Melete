//! Radarr, Sonarr, Lidarr and Prowlarr API clients: the only code that talks to them. Used to
//! import their lists, profiles and indexers.
//!
//! The API key travels only in the `X-Api-Key` header, so errors that quote URLs can't carry it.

use std::time::Duration;

/// The largest reply read from an *arr; a full Sonarr series list is a few MiB.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

use anyhow::{Context, bail};
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

mod types;

pub use types::*;

/// The *arr rejected the API key.
#[derive(Debug)]
pub struct Rejected;

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the API key was rejected (401)")
    }
}

impl std::error::Error for Rejected {}

/// The URL belongs to another app (Sonarr where Radarr was expected, say).
#[derive(Debug)]
pub struct WrongApp(pub String);

impl std::fmt::Display for WrongApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WrongApp {}

/// A client for one Radarr, Sonarr, Lidarr or Prowlarr: its address and API key. It only reads
/// (GET requests), never follows redirects, and gives up after 60 seconds.
///
/// Every request fails with [`Rejected`] when the *arr answers 401, and with a plain error when
/// it can't be reached, redirects, answers another error status, sends more than 64 MiB, or
/// sends something that doesn't read as the expected JSON.
#[derive(Clone)]
pub struct ArrClient {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl ArrClient {
    /// `url` is the *arr's address as Melete can reach it, e.g. `http://192.168.1.10:7878`.
    ///
    /// # Errors
    ///
    /// Refuses a URL that isn't http or https with a host, or that holds a user name,
    /// password, query or fragment, and an empty API key. Nothing is sent to the *arr.
    pub fn new(url: &str, key: String) -> anyhow::Result<Self> {
        let parsed = reqwest::Url::parse(url).context("the URL isn't valid")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
            bail!("the URL must be http:// or https:// with a host");
        }
        // A password in the URL would be stored and shown (indexer feeds are built from it),
        // and a query would end up in the middle of them.
        if !parsed.username().is_empty() || parsed.password().is_some() {
            bail!("the URL can't contain a user name or password; use the API key");
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            bail!("the URL can't have a query or fragment");
        }
        if key.trim().is_empty() {
            bail!("the API key is empty");
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            // reqwest would carry X-Api-Key to wherever a redirect points (an auth proxy's
            // login page, say); never follow one.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base: url.trim_end_matches('/').to_owned(),
            key,
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let mut response = self
            .http
            .get(format!("{}{path}", self.base))
            .header("X-Api-Key", &self.key)
            .send()
            .await
            .with_context(|| format!("can't reach {}", self.base))?;
        match response.status() {
            StatusCode::UNAUTHORIZED => return Err(Rejected.into()),
            status if status.is_redirection() => bail!(
                "{} redirected the request (an auth proxy in front of it?); use its direct address",
                self.base
            ),
            status if !status.is_success() => bail!("{} answered {status} for {path}", self.base),
            _ => {}
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .with_context(|| format!("reading {path}"))?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                bail!("{} sent more than 64 MiB for {path}", self.base);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).with_context(|| format!("reading {path}"))
    }

    /// The app's status, whose name tells Radarr and Sonarr apart (their v3 API).
    pub async fn system_status(&self) -> anyhow::Result<SystemStatus> {
        self.get("/api/v3/system/status").await
    }

    /// Fails unless the app at the URL is `expected` ("Radarr", "Sonarr", "Lidarr" or
    /// "Prowlarr").
    ///
    /// # Errors
    ///
    /// [`WrongApp`] when another app answers, and [`Rejected`] when the key is refused.
    pub async fn expect_app(&self, expected: &str) -> anyhow::Result<()> {
        // Prowlarr's and Lidarr's API is v1; Radarr's and Sonarr's v3.
        let status = match expected {
            "Prowlarr" | "Lidarr" => self.get("/api/v1/system/status").await?,
            _ => self.system_status().await?,
        };
        if !status.app_name.eq_ignore_ascii_case(expected) {
            let message = format!("that URL is {}, not {expected}", status.app_name);
            return Err(WrongApp(message).into());
        }
        Ok(())
    }

    /// Radarr's or Sonarr's quality profiles, by id and name only.
    pub async fn quality_profiles(&self) -> anyhow::Result<Vec<QualityProfile>> {
        self.get("/api/v3/qualityprofile").await
    }

    /// Radarr's or Sonarr's quality profiles in full, for importing them.
    pub async fn full_quality_profiles(&self) -> anyhow::Result<Vec<FullQualityProfile>> {
        self.get("/api/v3/qualityprofile").await
    }

    /// Radarr's or Sonarr's custom formats with their conditions, for importing them.
    pub async fn custom_formats(&self) -> anyhow::Result<Vec<CustomFormat>> {
        self.get("/api/v3/customformat").await
    }

    /// The app's languages: its custom formats and profiles refer to them by id, and Radarr
    /// and Sonarr number them differently.
    pub async fn languages(&self) -> anyhow::Result<Vec<IdName>> {
        self.get("/api/v3/language").await
    }

    /// Prowlarr's indexers, with their categories and search capabilities.
    pub async fn prowlarr_indexers(&self) -> anyhow::Result<Vec<ProwlarrIndexer>> {
        self.get("/api/v1/indexer").await
    }

    /// The URL as given, without a trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The API key, for Prowlarr's per-indexer feeds, which take the same key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Lidarr's quality profiles, by id and name only.
    pub async fn lidarr_quality_profiles(&self) -> anyhow::Result<Vec<QualityProfile>> {
        self.get("/api/v1/qualityprofile").await
    }

    /// Lidarr's quality profiles in full, for importing them.
    pub async fn lidarr_full_quality_profiles(&self) -> anyhow::Result<Vec<FullQualityProfile>> {
        self.get("/api/v1/qualityprofile").await
    }

    /// Every artist Lidarr has, monitored or not.
    pub async fn lidarr_artists(&self) -> anyhow::Result<Vec<LidarrArtist>> {
        self.get("/api/v1/artist").await
    }

    /// Every album Lidarr has, for all its artists.
    pub async fn lidarr_albums(&self) -> anyhow::Result<Vec<LidarrAlbum>> {
        self.get("/api/v1/album").await
    }

    /// Every movie Radarr has, monitored or not.
    pub async fn radarr_movies(&self) -> anyhow::Result<Vec<RadarrMovie>> {
        self.get("/api/v3/movie").await
    }

    /// Every series Sonarr has, monitored or not, with its seasons.
    pub async fn sonarr_series(&self) -> anyhow::Result<Vec<SonarrSeries>> {
        self.get("/api/v3/series").await
    }

    /// One series' episodes, by Sonarr's own series id.
    pub async fn sonarr_episodes(&self, series_id: i64) -> anyhow::Result<Vec<SonarrEpisode>> {
        self.get(&format!("/api/v3/episode?seriesId={series_id}"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_urls_that_would_carry_secrets_or_break_feeds() {
        for url in [
            "http://user:pass@host:9696",
            "http://user@host:9696",
            "http://host:9696/?a=b",
            "http://host:9696/#x",
        ] {
            assert!(ArrClient::new(url, "key".into()).is_err(), "{url}");
        }
        assert!(ArrClient::new("http://192.168.1.10:9696/", "key".into()).is_ok());
    }

    #[test]
    fn a_prowlarr_indexer_without_capabilities_still_reads() {
        let indexer: ProwlarrIndexer = serde_json::from_str(
            r#"{ "id": 1, "name": "X", "enable": true, "protocol": "torrent", "capabilities": null }"#,
        )
        .unwrap();
        assert!(indexer.capabilities.categories.is_empty());
    }
}
