//! The Cover Art Archive, for album covers of releases matched to MusicBrainz. Asked
//! only for an album with no picture of its own, once (then cached), or once when an imported
//! album brought none, and only when MusicBrainz matching is on; it learns a
//! release group's id and nothing else. Its answers redirect to
//! archive.org, which is followed over https to those hosts only.

use std::time::Duration;

/// The size asked for: big enough for an album page, small enough for a grid.
const SIZE: &str = "500";
const BASE: &str = "https://coverartarchive.org";
const MAX_BYTES: usize = 10 * 1024 * 1024;

/// A Cover Art Archive client that follows redirects only to archive.org's hosts over https.
pub struct Client {
    http: reqwest::Client,
    base: String,
}

/// What asking for a release group's front cover gave.
#[derive(Debug, PartialEq)]
pub enum Fetched {
    /// The cover, at most 10 MiB, with the type the archive gave it.
    Image {
        bytes: Vec<u8>,
        content_type: String,
    },
    /// It has no front cover for that release group.
    None,
}

fn allowed(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    url.scheme() == "https"
        && (host == "coverartarchive.org"
            || host == "archive.org"
            || host.ends_with(".archive.org"))
}

impl Client {
    /// A client for coverartarchive.org.
    ///
    /// # Errors
    ///
    /// Fails only when the HTTP client can't be built.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_base(BASE.into(), false)
    }

    /// `any_host` lets tests redirect to a plain-http fake.
    pub fn with_base(base: String, any_host: bool) -> anyhow::Result<Self> {
        let policy = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if any_host || allowed(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirected somewhere other than archive.org")
            }
        });
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(policy)
            .user_agent(concat!("Melete/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { http, base })
    }

    /// A release group's front cover.
    pub async fn front(&self, release_group_id: &str) -> anyhow::Result<Fetched> {
        self.front_sized(release_group_id, SIZE).await
    }

    /// The front cover at `size` pixels ("250", "500" or "1200").
    pub async fn front_sized(&self, release_group_id: &str, size: &str) -> anyhow::Result<Fetched> {
        anyhow::ensure!(
            crate::library::tags::is_mbid(release_group_id),
            "not a MusicBrainz id"
        );
        let url = format!(
            "{}/release-group/{release_group_id}/front-{size}",
            self.base
        );
        let mut response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|err| anyhow::anyhow!("Cover Art Archive: {}", err.without_url()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Fetched::None);
        }
        anyhow::ensure!(
            response.status().is_success(),
            "Cover Art Archive answered {}",
            response.status()
        );
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        anyhow::ensure!(
            content_type.starts_with("image/"),
            "Cover Art Archive sent {content_type:?}, not an image"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| anyhow::anyhow!("Cover Art Archive: {}", err.without_url()))?
        {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= MAX_BYTES,
                "the cover is too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(Fetched::Image {
            bytes,
            content_type,
        })
    }
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, State};
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    /// The front cover the fake serves.
    pub const JPEG: &[u8] = b"\xff\xd8\xff\xe0fake jpeg";

    /// A fake archive: release group b1392450… redirects to its image; others have none.
    /// Requests are logged.
    pub async fn start() -> (super::Client, Arc<Mutex<Vec<String>>>) {
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let router = axum::Router::new()
            .route(
                "/release-group/{id}/{size}",
                axum::routing::get(
                    |State(log): State<Arc<Mutex<Vec<String>>>>,
                     Path((id, size)): Path<(String, String)>| async move {
                        log.lock().unwrap().push(format!("{id} {size}"));
                        if id.starts_with("b1392450") {
                            (
                                StatusCode::TEMPORARY_REDIRECT,
                                [(header::LOCATION, "/image.jpg")],
                            )
                                .into_response()
                        } else {
                            StatusCode::NOT_FOUND.into_response()
                        }
                    },
                ),
            )
            .route(
                "/image.jpg",
                axum::routing::get(|| async { ([(header::CONTENT_TYPE, "image/jpeg")], JPEG) }),
            )
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (super::Client::with_base(base, true).unwrap(), log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_only_archive_hosts_over_https() {
        let ok = |u: &str| allowed(&reqwest::Url::parse(u).unwrap());
        assert!(ok("https://coverartarchive.org/release-group/x/front-500"));
        assert!(ok("https://archive.org/download/mbid-x/x.jpg"));
        assert!(ok("https://ia800.us.archive.org/x.jpg"));
        assert!(!ok("http://archive.org/x.jpg"));
        assert!(!ok("https://archive.org.evil.example/x.jpg"));
        assert!(!ok("https://notarchive.org/x.jpg"));
    }

    #[tokio::test]
    async fn fetches_a_front_cover_through_its_redirect() {
        let (client, _) = testing::start().await;
        let found = client
            .front("b1392450-e666-3926-a536-22c65f834433")
            .await
            .unwrap();
        assert_eq!(
            found,
            Fetched::Image {
                bytes: testing::JPEG.to_vec(),
                content_type: "image/jpeg".into()
            }
        );
        let none = client
            .front("00000000-0000-0000-0000-000000000000")
            .await
            .unwrap();
        assert_eq!(none, Fetched::None);
        assert!(client.front("../../etc").await.is_err());
    }
}
