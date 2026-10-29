//! Fetching a release's torrent from its indexer link, which carries the indexer's key: errors
//! never quote it.

use super::*;

/// A torrent as an indexer's link gives it.
#[derive(Debug, PartialEq, Eq)]
pub enum Torrent {
    /// A .torrent file's bytes.
    File(Vec<u8>),
    /// A magnet link.
    Magnet(String),
}

/// Fetches .torrent files from indexer links (Prowlarr's, which carry its key: errors never
/// quote them).
#[derive(Clone)]
pub struct Fetcher {
    pub(super) http: reqwest::Client,
}

impl Fetcher {
    /// A fetcher whose requests give up after 60 seconds and don't follow redirects by themselves.
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            // Followed by hand: a redirect may point at a magnet link.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http })
    }

    /// Fetches what an indexer link stands for: a magnet link as it is, or the .torrent file it
    /// serves. Up to four redirects are followed, and one to a magnet link is taken as the torrent.
    ///
    /// # Errors
    ///
    /// Why it got no torrent, in words that never quote the link: not a URL or not http(s), an
    /// answer other than 200, a redirect into the local network from another host, a file over
    /// 10 MiB or one that isn't a torrent, or too many redirects.
    pub async fn fetch(&self, link: &str) -> Result<Torrent, String> {
        if link.starts_with("magnet:") {
            return Ok(Torrent::Magnet(link.to_owned()));
        }
        let mut url = reqwest::Url::parse(link).map_err(|_| "the link isn't a URL".to_owned())?;
        for _ in 0..4 {
            if !matches!(url.scheme(), "http" | "https") {
                return Err("the link isn't http or https".into());
            }
            let mut response = self
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(|err| err.without_url().to_string())?;
            let status = response.status();
            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .ok_or("a redirect without a location")?;
                if location.starts_with("magnet:") {
                    return Ok(Torrent::Magnet(location.to_owned()));
                }
                let next = url
                    .join(location)
                    .map_err(|_| "a redirect to a bad location".to_owned())?;
                // An indexer elsewhere can't send Melete into the local network.
                if next.host_str() != url.host_str() && deluge::local(&next) {
                    return Err("a redirect to a local address".into());
                }
                url = next;
                continue;
            }
            if status != StatusCode::OK {
                return Err(format!("the indexer answered {status}"));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|err| err.without_url().to_string())?
            {
                if body.len() + chunk.len() > MAX_TORRENT_BYTES {
                    return Err("the torrent is larger than 10 MiB".into());
                }
                body.extend_from_slice(&chunk);
            }
            if deluge::torrent_hash(&body).is_none() {
                return Err("the link gave something that isn't a torrent".into());
            }
            return Ok(Torrent::File(body));
        }
        Err("too many redirects".into())
    }
}
