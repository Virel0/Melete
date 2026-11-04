//! JASSUB, which the web player draws ASS/SSA subtitles with: libass compiled to WebAssembly,
//! run in a worker. Bundled from its npm release (`src/web/vendor/jassub-2.5.21/README.md` says
//! how) and served from the binary, so nothing is fetched from anywhere else.
//!
//! The page keeps its policy: it only loads the module and starts the worker. The worker alone
//! compiles WebAssembly, and a worker follows the policy sent with its own script, so only the
//! worker's adds `'wasm-unsafe-eval'`, which lets it compile WebAssembly and no more (no `eval`
//! of script). It fetches only from Melete: the WebAssembly, the track and the video's fonts.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use super::{FOREVER, JS, POLICY};

/// Where the files are served; the version in the path lets them be kept forever.
const DIR: &str = "/web/vendor/jassub-2.5.21";

const MODULE: &str = include_str!("../../web/vendor/jassub-2.5.21/jassub.mjs");
const WORKER: &str = include_str!("../../web/vendor/jassub-2.5.21/jassub-worker.mjs");
const WASM: &[u8] = include_bytes!("../../web/vendor/jassub-2.5.21/jassub-worker.wasm");
/// The same, built for browsers with WebAssembly SIMD and growable memory; faster there.
const WASM_MODERN: &[u8] =
    include_bytes!("../../web/vendor/jassub-2.5.21/jassub-worker-modern.wasm");
/// Liberation Sans, for text whose font neither the track nor the video brings.
const FONT: &[u8] = include_bytes!("../../web/vendor/jassub-2.5.21/default.woff2");

/// The worker's policy: compiling WebAssembly, and fetching from Melete only.
const WORKER_POLICY: &str =
    "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'";

/// JASSUB's files, open to all as the page's scripts are.
pub(super) fn routes() -> Router {
    Router::new()
        .route(
            &format!("{DIR}/jassub.mjs"),
            get(|| async { served(JS, POLICY, MODULE.as_bytes()) }),
        )
        .route(
            &format!("{DIR}/jassub-worker.mjs"),
            get(|| async { served(JS, WORKER_POLICY, WORKER.as_bytes()) }),
        )
        .route(
            &format!("{DIR}/jassub-worker.wasm"),
            get(|| async { served("application/wasm", POLICY, WASM) }),
        )
        .route(
            &format!("{DIR}/jassub-worker-modern.wasm"),
            get(|| async { served("application/wasm", POLICY, WASM_MODERN) }),
        )
        .route(
            &format!("{DIR}/default.woff2"),
            get(|| async { served("font/woff2", POLICY, FONT) }),
        )
}

fn served(
    content_type: &'static str,
    policy: &'static str,
    body: &'static [u8],
) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_SECURITY_POLICY, policy),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CACHE_CONTROL, FOREVER),
        ],
        body,
    )
}

#[cfg(test)]
mod tests;
