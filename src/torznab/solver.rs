//! The FlareSolverr indexers reach pages behind Cloudflare's check through (Uindex, and feeds
//! that turn Melete away), its browser fetching the way each indexer goes.

use super::Client;

/// The FlareSolverr to reach Uindex through, from `MELETE_FLARESOLVERR_URL`; set whenever the
/// outside services are (`live`).
static FLARESOLVERR: std::sync::RwLock<Option<crate::flaresolverr::Client>> =
    std::sync::RwLock::new(None);

fn flaresolverr() -> Option<crate::flaresolverr::Client> {
    FLARESOLVERR
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Makes FlareSolverr available to indexers that need it, or not.
pub fn use_flaresolverr(client: Option<crate::flaresolverr::Client>) {
    *FLARESOLVERR.write().unwrap_or_else(|e| e.into_inner()) = client;
}

/// Whether a FlareSolverr is set, so Uindex can be searched.
pub fn has_flaresolverr() -> bool {
    flaresolverr().is_some()
}

impl Client {
    /// FlareSolverr, when one is set, fetching through the proxy when this indexer goes
    /// through it.
    pub(super) fn solver(&self) -> Option<crate::flaresolverr::Client> {
        flaresolverr().map(|f| f.through(self.through))
    }
}
