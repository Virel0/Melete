//! Artwork from TMDB's image server, which needs no token, with a cap on how much is read.

use super::*;

pub(super) const IMAGE_BASE: &str = "https://image.tmdb.org/t/p";
pub(super) const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Downloads artwork from TMDB's image server. It needs no token, so none is sent.
#[derive(Clone)]
pub struct Images {
    pub(super) http: reqwest::Client,
    pub(super) base: String,
    pub(super) max_bytes: usize,
}

impl Images {
    /// A client for TMDB's own image server, whose downloads give up after 30 seconds.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_base(IMAGE_BASE.into())
    }

    pub(crate) fn with_base(base: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            base,
            max_bytes: MAX_IMAGE_BYTES,
        })
    }

    /// The image bytes, or `None` if TMDB has no such image. `size` and `file` must already
    /// be validated, since they go into the URL.
    pub async fn fetch(&self, size: &str, file: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let mut response = self
            .http
            .get(format!("{}/{size}/{file}", self.base))
            .send()
            .await
            .with_context(|| format!("downloading TMDB image {size}/{file}"))?;
        match response.status() {
            status if status.is_success() => {}
            StatusCode::NOT_FOUND => return Ok(None),
            status => bail!("TMDB images answered {status} for {size}/{file}"),
        }
        // A CDN error page served with 200 must not be cached forever as a poster.
        let is_image = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("image/"));
        if !is_image {
            bail!("TMDB images sent something other than an image for {size}/{file}");
        }
        let too_large = || anyhow::anyhow!("TMDB image {size}/{file} is over the size limit");
        if response
            .content_length()
            .is_some_and(|n| n > self.max_bytes as u64)
        {
            return Err(too_large());
        }
        // Read in chunks so a body without a length can't be buffered past the limit.
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > self.max_bytes {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(bytes))
    }
}
