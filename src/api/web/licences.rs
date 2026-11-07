//! The licence texts of what the web UI serves from `src/web/vendor`, as they ship beside the
//! libraries, for the Licences page (`licences.js`) to show. Open to all, as the libraries are.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use super::{ALWAYS_ASK, POLICY};

/// hls.js's `LICENSE`, as its npm release has it.
pub(super) const HLS: &str = include_str!("../../web/vendor/hls.js-LICENSE");
/// JASSUB's licence and those of everything bundled with it, one `==== name version licence`
/// heading each.
pub(super) const JASSUB: &str = include_str!("../../web/vendor/jassub-2.5.21/LICENSES.txt");

pub(super) fn routes() -> Router {
    Router::new()
        .route("/web/licences/hls.js.txt", get(|| async { text(HLS) }))
        .route("/web/licences/jassub.txt", get(|| async { text(JASSUB) }))
}

fn text(body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, POLICY),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CACHE_CONTROL, ALWAYS_ASK),
        ],
        body,
    )
}

#[cfg(test)]
mod tests;
