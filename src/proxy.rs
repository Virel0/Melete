//! Sending chosen services through a proxy (HTTP, HTTPS or SOCKS5), with a kill switch. Every
//! HTTP client Melete makes to reach another service is a [`Http`] from here, so none can skip
//! the choice made for it (clippy's `disallowed-methods` and `disallowed-types` hold every other
//! module to that, and `tests/outside_clients.rs` reads the sources for it). Each service goes
//! direct unless an admin switches it on; one switched on only ever goes through the proxy: when
//! the proxy is down or isn't set, its requests fail with words that name the proxy, never its
//! login, and nothing is sent direct. SOCKS5 is always used as socks5h, so the proxy looks names
//! up and the server doesn't. A change applies to every client from its next request, those
//! kept for long jobs included.

// The one module that makes reqwest clients and picks their proxy.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::sync::{Arc, LazyLock, RwLock};

use crate::config::Secret;

mod check;
mod http;
mod setup;
mod watch;

pub use check::{ECHO, test};
pub use http::Http;
pub use watch::{Status, Unreachable, finding, mark, trouble_since};

/// How the proxy is spoken to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// SOCKS5, with names looked up by the proxy (socks5h).
    Socks5,
    /// An HTTP proxy, reached in the clear (https sites still go through it encrypted).
    Http,
    /// An HTTP proxy reached over TLS.
    Https,
}

impl Kind {
    /// The scheme reqwest knows it by.
    fn scheme(self) -> &'static str {
        match self {
            Kind::Socks5 => "socks5h",
            Kind::Http => "http",
            Kind::Https => "https",
        }
    }
}

/// The proxy: where it is and the login it may want. Its Debug output leaves the login out.
#[derive(Clone)]
pub struct Proxy {
    /// How it's spoken to (`MELETE_PROXY_TYPE`).
    pub kind: Kind,
    /// `MELETE_PROXY_HOST`: a name, an IPv4 address, or an IPv6 one in brackets.
    pub host: String,
    /// `MELETE_PROXY_PORT`.
    pub port: u16,
    /// `MELETE_PROXY_USERNAME` and `MELETE_PROXY_PASSWORD`, when it wants a login.
    pub login: Option<(String, Secret)>,
}

impl Proxy {
    /// Where it is, as messages name it, e.g. `socks5h://192.168.1.10:1080`; never the login.
    pub fn address(&self) -> String {
        format!("{}://{}:{}", self.kind.scheme(), self.host, self.port)
    }

    /// The proxy as reqwest takes it, the login in its URL's user part.
    fn reqwest(&self) -> anyhow::Result<reqwest::Proxy> {
        let proxy = reqwest::Proxy::all(self.address())
            .map_err(|_| anyhow::anyhow!("{} isn't a proxy address", self.address()))?;
        Ok(match &self.login {
            Some((user, password)) => proxy.basic_auth(user, password.expose()),
            None => proxy,
        })
    }
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("address", &self.address())
            .field("login", &self.login.is_some())
            .finish()
    }
}

/// A service that can be sent through the proxy, each with its own switch. Indexers aren't
/// here: each has a choice of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// TMDB's API and its image server.
    Tmdb,
    MusicBrainz,
    /// The Cover Art Archive, and archive.org where it redirects.
    CoverArt,
    /// Wikidata and Wikimedia Commons, for artists' portraits.
    Wikimedia,
    /// LRCLIB, for lyrics.
    Lyrics,
    OpenSubtitles,
    /// OpenRouter, for AI features.
    OpenRouter,
    /// IMDb's and Rotten Tomatoes' charts, for import lists.
    Lists,
    /// LibreTranslate, at the address set.
    Translation,
    /// The identity provider for OIDC sign-in.
    Oidc,
    /// Deluge's web UI on the seedbox.
    Deluge,
    /// Sonarr, Radarr, Lidarr and Prowlarr.
    Arr,
    Slskd,
    /// Melete's requests to FlareSolverr itself (the pages it fetches are its own).
    FlareSolverr,
}

impl Service {
    /// Every service, in the order the settings show them.
    pub const ALL: [Service; 14] = [
        Service::Tmdb,
        Service::MusicBrainz,
        Service::CoverArt,
        Service::Wikimedia,
        Service::Lyrics,
        Service::OpenSubtitles,
        Service::OpenRouter,
        Service::Lists,
        Service::Translation,
        Service::Oidc,
        Service::Deluge,
        Service::Arr,
        Service::Slskd,
        Service::FlareSolverr,
    ];

    /// The setting that sends it through the proxy, on or off.
    pub const fn setting(self) -> &'static str {
        match self {
            Service::Tmdb => "MELETE_PROXY_TMDB",
            Service::MusicBrainz => "MELETE_PROXY_MUSICBRAINZ",
            Service::CoverArt => "MELETE_PROXY_COVERART",
            Service::Wikimedia => "MELETE_PROXY_WIKIMEDIA",
            Service::Lyrics => "MELETE_PROXY_LYRICS",
            Service::OpenSubtitles => "MELETE_PROXY_OPENSUBTITLES",
            Service::OpenRouter => "MELETE_PROXY_OPENROUTER",
            Service::Lists => "MELETE_PROXY_LISTS",
            Service::Translation => "MELETE_PROXY_TRANSLATION",
            Service::Oidc => "MELETE_PROXY_OIDC",
            Service::Deluge => "MELETE_PROXY_DELUGE",
            Service::Arr => "MELETE_PROXY_ARR",
            Service::Slskd => "MELETE_PROXY_SLSKD",
            Service::FlareSolverr => "MELETE_PROXY_FLARESOLVERR",
        }
    }
}

/// The proxy, if one is set, and the services sent through it.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    /// The proxy; None when none is set.
    pub at: Option<Proxy>,
    /// The services switched on to go through it.
    pub through: Vec<Service>,
}

impl Setup {
    /// Whether `service` goes through the proxy.
    pub fn uses(&self, service: Service) -> bool {
        self.through.contains(&service)
    }
}

/// The setup in use, and how its proxy has answered.
pub struct Applied {
    pub setup: Setup,
    pub status: Arc<Status>,
}

impl Applied {
    fn new(setup: Setup) -> Applied {
        Applied {
            setup,
            status: Arc::default(),
        }
    }
}

static APPLIED: LazyLock<RwLock<Arc<Applied>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Applied::new(Setup::default()))));

#[cfg(test)]
tokio::task_local! {
    /// A test's own setup, in place of the server's, so tests running together don't share one;
    /// a test can change it while it runs (`testing::switch`).
    static TESTING: Arc<std::sync::Mutex<Arc<Applied>>>;
}

/// The setup in use now: every request a client makes goes the way it says.
pub fn current() -> Arc<Applied> {
    #[cfg(test)]
    if let Ok(applied) = TESTING.try_with(|t| t.lock().unwrap_or_else(|e| e.into_inner()).clone()) {
        return applied;
    }
    APPLIED.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Puts `setup` in use, for every request from now on, by clients old and new; returns the one
/// it replaces, to put back with [`restore`] if what's built with it can't be used.
pub fn apply(setup: Setup) -> Arc<Applied> {
    let mut applied = APPLIED.write().unwrap_or_else(|e| e.into_inner());
    std::mem::replace(&mut *applied, Arc::new(Applied::new(setup)))
}

/// Puts back a setup [`apply`] replaced.
pub fn restore(previous: Arc<Applied>) {
    *APPLIED.write().unwrap_or_else(|e| e.into_inner()) = previous;
}

/// A bare client builder: the one place outside services' clients start, so nothing else may
/// make one (clippy's `disallowed-methods` and `tests/outside_clients.rs` hold every other file
/// to that).
fn bare() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
}

/// `configured` (a bare builder with a service's own settings) set to go the way `applied`
/// says when `through` is true: through its proxy, or nowhere when it has none.
fn route(
    applied: &Applied,
    through: bool,
    configured: reqwest::ClientBuilder,
) -> anyhow::Result<reqwest::ClientBuilder> {
    if !through {
        return Ok(configured);
    }
    let Some(proxy) = &applied.setup.at else {
        return Ok(nowhere(configured));
    };
    via(configured, proxy, Some(applied.status.clone()))
}

/// `configured` set to go nowhere: every connection is refused, none made direct.
fn nowhere(configured: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    configured
        .no_proxy()
        .connector_layer(watch::Watch::refusing())
}

/// `builder` sent through `proxy`, every connection watched, and noted in `status` when given.
fn via(
    builder: reqwest::ClientBuilder,
    proxy: &Proxy,
    status: Option<Arc<Status>>,
) -> anyhow::Result<reqwest::ClientBuilder> {
    Ok(builder
        .proxy(proxy.reqwest()?)
        .connector_layer(watch::Watch::through(proxy, status)))
}

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
