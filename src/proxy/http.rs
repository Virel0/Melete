//! The HTTP client every service holds. It's made from the setup in use, and made again from
//! the new one the first time it's used after a change, so a client kept for a long job (an
//! import of hundreds of tracks, a search for everything missing) goes the new way from its next
//! request. A setup it can't be made with refuses every connection rather than going on the old
//! way.

use std::sync::Mutex;

use reqwest::{IntoUrl, Method, RequestBuilder};

use super::*;

/// Which way a client goes: by a service's switch, or by a choice of its own (an indexer's).
#[derive(Clone, Copy)]
enum Way {
    Service(Service),
    Through(bool),
}

type Configure = dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync;

struct Inner {
    way: Way,
    /// The service's own settings (timeout, redirects, User-Agent), given each time it's made.
    configure: Box<Configure>,
    /// The setup it was last made with, and what that made.
    made: Mutex<(Arc<Applied>, reqwest::Client)>,
    /// What's used when a setup can't be made into a client: one that never connects.
    refusing: reqwest::Client,
}

/// An HTTP client that always goes the way the setup in use says. Cheap to clone.
#[derive(Clone)]
pub struct Http(Arc<Inner>);

impl Http {
    /// A client for `service`, with its own settings from `configure`.
    ///
    /// # Errors
    ///
    /// When the client can't be made (no TLS backend, a proxy address reqwest won't take).
    pub fn new(
        service: Service,
        configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync + 'static,
    ) -> anyhow::Result<Http> {
        Self::made(Way::Service(service), Box::new(configure))
    }

    /// A client that goes through the proxy when `through` is true, for what has a choice of its
    /// own rather than one switch.
    ///
    /// # Errors
    ///
    /// As [`Http::new`].
    pub fn through(
        through: bool,
        configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync + 'static,
    ) -> anyhow::Result<Http> {
        Self::made(Way::Through(through), Box::new(configure))
    }

    fn made(way: Way, configure: Box<Configure>) -> anyhow::Result<Http> {
        let applied = current();
        let client = make(&applied, way, &configure)?;
        let refusing = nowhere(configure(bare())).build()?;
        Ok(Http(Arc::new(Inner {
            way,
            configure,
            made: Mutex::new((applied, client)),
            refusing,
        })))
    }

    /// The client for the setup in use now, made again when it changed.
    fn client(&self) -> reqwest::Client {
        let applied = current();
        let mut made = self.0.made.lock().unwrap_or_else(|e| e.into_inner());
        if !Arc::ptr_eq(&made.0, &applied) {
            let client = make(&applied, self.0.way, &self.0.configure)
                .unwrap_or_else(|_| self.0.refusing.clone());
            *made = (applied, client);
        }
        made.1.clone()
    }

    pub fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        self.client().request(method, url)
    }

    pub fn get<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.client().get(url)
    }

    pub fn post<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.client().post(url)
    }

    pub fn delete<U: IntoUrl>(&self, url: U) -> RequestBuilder {
        self.client().delete(url)
    }
}

/// The client `way` makes under `applied`.
fn make(applied: &Applied, way: Way, configure: &Configure) -> anyhow::Result<reqwest::Client> {
    let through = match way {
        Way::Service(service) => applied.setup.uses(service),
        Way::Through(through) => through,
    };
    Ok(route(applied, through, configure(bare()))?.build()?)
}
