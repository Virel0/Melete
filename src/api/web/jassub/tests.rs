use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::*;

/// Each file's SHA-256, as `src/web/vendor/jassub-2.5.21/README.md`'s steps made it from
/// jassub 2.5.21 and esbuild 0.25.10 (pinned with their integrity hashes in `build/`).
const MADE: [(&str, &str); 5] = [
    (
        "jassub.mjs",
        "a2eed09c54e6b313fcb65d81cd9753db5011add89877e2f950be9c038af0aa79",
    ),
    (
        "jassub-worker.mjs",
        "d713a127943d0c058a45c1e079d1d6fd97ac8224a7192505c0a707d6a8fd7e4d",
    ),
    (
        "jassub-worker.wasm",
        "833b31f827e5879cb00310b4d3d8944955e239d13e2268fbb48ed0a6fafcdd04",
    ),
    (
        "jassub-worker-modern.wasm",
        "6970b0e4469c0b6cca54c4f19d78236a05bd87b3029c147a0f29251644dea756",
    ),
    (
        "default.woff2",
        "886929903707c5bb28b07cd2eed69921b3d5ef5c49c73a0dd1e187ebf6546a4c",
    ),
];

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn each_file_is_served_as_made_with_its_type_and_policy() {
    for (name, digest) in MADE {
        let request = Request::get(format!("{DIR}/{name}"))
            .body(Body::empty())
            .unwrap();
        let response = routes().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{name}");
        let headers = response.headers().clone();
        let kind = headers[header::CONTENT_TYPE].to_str().unwrap();
        let expected = match name.rsplit_once('.').unwrap().1 {
            "mjs" => JS,
            "wasm" => "application/wasm",
            _ => "font/woff2",
        };
        assert_eq!(kind, expected, "{name}");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::CACHE_CONTROL], FOREVER, "{name}");
        let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        if name == "jassub-worker.mjs" {
            assert_eq!(policy, WORKER_POLICY);
        } else {
            assert_eq!(policy, POLICY, "{name} keeps the page's policy");
        }
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            hex(&body),
            digest,
            "{name} isn't what the README's steps made"
        );
    }
}

/// The worker may compile WebAssembly and fetch from Melete, and nothing more: no evaluated
/// script, no inline code, no other host.
#[test]
fn only_the_worker_may_compile_webassembly_and_no_more() {
    assert!(WORKER_POLICY.contains("script-src 'self' 'wasm-unsafe-eval';"));
    assert!(WORKER_POLICY.starts_with("default-src 'none';"));
    assert!(WORKER_POLICY.ends_with("connect-src 'self'"));
    for loose in [
        "'unsafe-eval'",
        "'unsafe-inline'",
        "http",
        "*",
        "data:",
        "blob:",
    ] {
        assert!(!WORKER_POLICY.contains(loose), "{loose}");
    }
    assert!(
        !POLICY.contains("wasm"),
        "the page's own policy is untouched"
    );
}

/// The bundles hold no script evaluation and no way to fetch fonts from an outside service:
/// `lfa-ponyfill` is the stub that finds none.
#[test]
fn the_bundles_ask_nothing_of_anywhere_else() {
    for (name, code) in [("jassub.mjs", MODULE), ("jassub-worker.mjs", WORKER)] {
        assert!(
            !code.contains("eval(") && !code.contains("new Function"),
            "{name}"
        );
        for host in ["googleapis", "gstatic", "fonts.google"] {
            assert!(!code.contains(host), "{name}: {host}");
        }
    }
    assert!(WORKER.contains("async function queryRemoteFonts() {\n  return [];\n}"));
    // The page compiles no WebAssembly: its policy wouldn't let it.
    assert!(
        !MODULE.contains("new WebAssembly.Module") && !MODULE.contains("new WebAssembly.Instance")
    );
}

/// The player asks for these files where they're served, and never for fonts beyond Melete's.
#[test]
fn the_player_loads_jassub_from_here_with_font_lookups_off() {
    let player = include_str!("../../../web/player-ass.js");
    assert!(player.contains(&format!("const JASSUB = \"{DIR}\";")));
    for name in [
        "jassub.mjs",
        "jassub-worker.mjs",
        "jassub-worker.wasm",
        "jassub-worker-modern.wasm",
    ] {
        assert!(player.contains(&format!("${{JASSUB}}/{name}")), "{name}");
    }
    assert!(player.contains("queryFonts: false"));
    assert!(player.contains("${streamBase}/fonts"));
    assert!(player.contains("${streamBase}/${track.ass_url}"));
}

/// A browser that can't draw ASS/SSA asks the server to draw a styled track in, by converting
/// with the track named and `renders_ass` false; one that can sends neither, so changing
/// subtitles never restarts its conversion. A track drawn in isn't also shown on top.
#[test]
fn a_browser_that_cant_draw_ass_asks_the_server_to() {
    let ass = include_str!("../../../web/player-ass.js");
    let choice = include_str!("../../../web/player-choice.js");
    let player = include_str!("../../../web/player.js");
    assert!(ass.contains("return track?.ass && !canDrawAss() ? track.track : null;"));
    assert!(choice.contains(
        "...(choice.serverDraws ? { subtitle: choice.serverDraws, renders_ass: false } : {}),"
    ));
    assert!(choice.contains("if (choice.burn || choice.serverDraws || choice.fallback"));
    assert!(player.contains("s.burned = answer.burned_subtitle ?? null;"));
    assert!(player.contains("s.burned && s.burned === s.choice.subtitle"));
}
