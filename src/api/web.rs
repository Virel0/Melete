//! The web UI: one page and its scripts and styles, built into the binary. The
//! page talks to the same API as the apps, signed in by cookie.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

const INDEX: &str = include_str!("../web/index.html");
const STYLES: &str = include_str!("../web/app.css");
/// The styles that follow app.css, in the order the page links them: later rules win, so the
/// order matters.
const MORE_STYLES: [(&str, &str); 7] = [
    ("library.css", include_str!("../web/library.css")),
    ("tools.css", include_str!("../web/tools.css")),
    ("shell.css", include_str!("../web/shell.css")),
    ("player.css", include_str!("../web/player.css")),
    ("manage.css", include_str!("../web/manage.css")),
    ("listen.css", include_str!("../web/listen.css")),
    ("watch.css", include_str!("../web/watch.css")),
];
/// The tab's icon, in the page's ink and brass.
const ICON: &str = include_str!("../web/icon.svg");
/// The page's modules, by the name they're served under in `/web/`.
const MODULES: [(&str, &str); 106] = [
    ("app.js", include_str!("../web/app.js")),
    ("shell.js", include_str!("../web/shell.js")),
    ("sides.js", include_str!("../web/sides.js")),
    ("page-parts.js", include_str!("../web/page-parts.js")),
    ("sidebar.js", include_str!("../web/sidebar.js")),
    (
        "manage-services.js",
        include_str!("../web/manage-services.js"),
    ),
    ("manage-parts.js", include_str!("../web/manage-parts.js")),
    ("indexers.js", include_str!("../web/indexers.js")),
    ("release-try.js", include_str!("../web/release-try.js")),
    ("system-status.js", include_str!("../web/system-status.js")),
    ("system-upkeep.js", include_str!("../web/system-upkeep.js")),
    ("user-keys.js", include_str!("../web/user-keys.js")),
    (
        "library-rescan.js",
        include_str!("../web/library-rescan.js"),
    ),
    ("page-state.js", include_str!("../web/page-state.js")),
    ("cards.js", include_str!("../web/cards.js")),
    ("home.js", include_str!("../web/home.js")),
    ("watch-hero.js", include_str!("../web/watch-hero.js")),
    (
        "watch-hero-trailer.js",
        include_str!("../web/watch-hero-trailer.js"),
    ),
    ("watch-tiles.js", include_str!("../web/watch-tiles.js")),
    ("watch-play.js", include_str!("../web/watch-play.js")),
    ("watch-kind.js", include_str!("../web/watch-kind.js")),
    ("browse.js", include_str!("../web/browse.js")),
    ("title-page.js", include_str!("../web/title-page.js")),
    ("title-colour.js", include_str!("../web/title-colour.js")),
    (
        "title-episodes.js",
        include_str!("../web/title-episodes.js"),
    ),
    ("title-more.js", include_str!("../web/title-more.js")),
    ("title-trailer.js", include_str!("../web/title-trailer.js")),
    ("people.js", include_str!("../web/people.js")),
    ("lib.js", include_str!("../web/lib.js")),
    ("player.js", include_str!("../web/player.js")),
    ("player-choice.js", include_str!("../web/player-choice.js")),
    ("player-keys.js", include_str!("../web/player-keys.js")),
    ("player-next.js", include_str!("../web/player-next.js")),
    ("player-chips.js", include_str!("../web/player-chips.js")),
    (
        "player-previews.js",
        include_str!("../web/player-previews.js"),
    ),
    (
        "player-offline.js",
        include_str!("../web/player-offline.js"),
    ),
    (
        "player-subtitle-search.js",
        include_str!("../web/player-subtitle-search.js"),
    ),
    ("settings.js", include_str!("../web/settings.js")),
    ("account.js", include_str!("../web/account.js")),
    ("two-factor.js", include_str!("../web/two-factor.js")),
    ("languages.js", include_str!("../web/languages.js")),
    (
        "playback-prefs.js",
        include_str!("../web/playback-prefs.js"),
    ),
    ("favourites.js", include_str!("../web/favourites.js")),
    ("user-access.js", include_str!("../web/user-access.js")),
    ("users.js", include_str!("../web/users.js")),
    ("forms.js", include_str!("../web/forms.js")),
    ("sources.js", include_str!("../web/sources.js")),
    ("sources-arrs.js", include_str!("../web/sources-arrs.js")),
    ("sources-lists.js", include_str!("../web/sources-lists.js")),
    (
        "sources-profiles.js",
        include_str!("../web/sources-profiles.js"),
    ),
    ("wanted-page.js", include_str!("../web/wanted-page.js")),
    ("wanted.js", include_str!("../web/wanted.js")),
    (
        "wanted-missing.js",
        include_str!("../web/wanted-missing.js"),
    ),
    (
        "wanted-downloads.js",
        include_str!("../web/wanted-downloads.js"),
    ),
    ("wanted-rss.js", include_str!("../web/wanted-rss.js")),
    (
        "wanted-set-aside.js",
        include_str!("../web/wanted-set-aside.js"),
    ),
    ("music.js", include_str!("../web/music.js")),
    ("music-list.js", include_str!("../web/music-list.js")),
    ("music-browse.js", include_str!("../web/music-browse.js")),
    ("music-radio.js", include_str!("../web/music-radio.js")),
    ("music-actions.js", include_str!("../web/music-actions.js")),
    ("music-liked.js", include_str!("../web/music-liked.js")),
    ("music-sidebar.js", include_str!("../web/music-sidebar.js")),
    ("music-access.js", include_str!("../web/music-access.js")),
    (
        "music-playlists.js",
        include_str!("../web/music-playlists.js"),
    ),
    (
        "music-playlist.js",
        include_str!("../web/music-playlist.js"),
    ),
    (
        "music-playlist-pick.js",
        include_str!("../web/music-playlist-pick.js"),
    ),
    ("music-parts.js", include_str!("../web/music-parts.js")),
    ("music-home.js", include_str!("../web/music-home.js")),
    ("music-play.js", include_str!("../web/music-play.js")),
    ("music-pins.js", include_str!("../web/music-pins.js")),
    ("music-album.js", include_str!("../web/music-album.js")),
    ("music-works.js", include_str!("../web/music-works.js")),
    ("music-player.js", include_str!("../web/music-player.js")),
    ("music-queue.js", include_str!("../web/music-queue.js")),
    (
        "music-controls.js",
        include_str!("../web/music-controls.js"),
    ),
    ("music-bar.js", include_str!("../web/music-bar.js")),
    ("music-panel.js", include_str!("../web/music-panel.js")),
    ("music-lyrics.js", include_str!("../web/music-lyrics.js")),
    ("wanted-music.js", include_str!("../web/wanted-music.js")),
    ("progress.js", include_str!("../web/progress.js")),
    ("requests.js", include_str!("../web/requests.js")),
    ("requests-row.js", include_str!("../web/requests-row.js")),
    (
        "requests-imports.js",
        include_str!("../web/requests-imports.js"),
    ),
    (
        "requests-admin.js",
        include_str!("../web/requests-admin.js"),
    ),
    ("services.js", include_str!("../web/services.js")),
    ("diagnostics.js", include_str!("../web/diagnostics.js")),
    ("admin.js", include_str!("../web/admin.js")),
    ("manage.js", include_str!("../web/manage.js")),
    (
        "manage-library.js",
        include_str!("../web/manage-library.js"),
    ),
    ("manage-title.js", include_str!("../web/manage-title.js")),
    ("manage-search.js", include_str!("../web/manage-search.js")),
    (
        "manage-history.js",
        include_str!("../web/manage-history.js"),
    ),
    (
        "manage-seasons.js",
        include_str!("../web/manage-seasons.js"),
    ),
    ("manage-artist.js", include_str!("../web/manage-artist.js")),
    ("manage-album.js", include_str!("../web/manage-album.js")),
    ("manage-add.js", include_str!("../web/manage-add.js")),
    (
        "manage-activity.js",
        include_str!("../web/manage-activity.js"),
    ),
    (
        "manage-settings.js",
        include_str!("../web/manage-settings.js"),
    ),
    ("admin-music.js", include_str!("../web/admin-music.js")),
    (
        "admin-overviews.js",
        include_str!("../web/admin-overviews.js"),
    ),
    ("health.js", include_str!("../web/health.js")),
    ("calendar.js", include_str!("../web/calendar.js")),
    ("palette.js", include_str!("../web/palette.js")),
    (
        "profile-editor.js",
        include_str!("../web/profile-editor.js"),
    ),
    ("editor-parts.js", include_str!("../web/editor-parts.js")),
    (
        "condition-editor.js",
        include_str!("../web/condition-editor.js"),
    ),
];
/// hls.js 1.7.3's `dist/hls.light.min.mjs` from its npm release, unchanged (a test checks its
/// SHA-256). Its licence is beside it.
const HLS: &str = include_str!("../web/vendor/hls-1.7.3.mjs");

/// Only Melete's own files, and nothing inline, so text that came from a file name or TMDB
/// can't run as script even if it reached the page as markup. Media may also come from `blob:`
/// URLs, which is how hls.js hands converted video to the player.
const POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
     media-src 'self' blob:; connect-src 'self'; form-action 'self'; base-uri 'none'; \
     frame-ancestors 'none'";

const JS: &str = "text/javascript; charset=utf-8";
/// Small files on a home network: always ask, so an upgrade shows at once.
const ALWAYS_ASK: &str = "no-cache";
/// A file whose name carries its version never changes.
const FOREVER: &str = "public, max-age=31536000, immutable";

/// The web UI's page, scripts and styles, open to all: the page signs in itself.
pub fn routes() -> Router {
    let mut router = Router::new()
        .route(
            "/",
            get(|| async { file("text/html; charset=utf-8", ALWAYS_ASK, INDEX) }),
        )
        .route(
            "/web/app.css",
            get(|| async { file("text/css; charset=utf-8", ALWAYS_ASK, STYLES) }),
        )
        .route(
            "/web/icon.svg",
            get(|| async { file("image/svg+xml", ALWAYS_ASK, ICON) }),
        )
        .route(
            "/web/hls-1.7.3.mjs",
            get(|| async { file(JS, FOREVER, HLS) }),
        );
    for (name, body) in MODULES {
        router = router.route(
            &format!("/web/{name}"),
            get(move || async move { file(JS, ALWAYS_ASK, body) }),
        );
    }
    for (name, body) in MORE_STYLES {
        router = router.route(
            &format!("/web/{name}"),
            get(move || async move { file("text/css; charset=utf-8", ALWAYS_ASK, body) }),
        );
    }
    router
}

/// The web UI's files as [`routes`] serves them, their policy letting YouTube's privacy-enhanced
/// player in as a frame (and nothing else besides) while trailers online is on.
pub fn routes_with(pool: sqlx::SqlitePool) -> Router {
    routes().layer(axum::middleware::map_response_with_state(
        pool,
        with_trailer_frames,
    ))
}

/// Adds `frame-src` for YouTube's privacy-enhanced player to a response's policy while trailers
/// online is on; leaves it as it is otherwise, or when the setting can't be read.
async fn with_trailer_frames(
    axum::extract::State(pool): axum::extract::State<sqlx::SqlitePool>,
    mut response: axum::response::Response,
) -> axum::response::Response {
    if crate::settings::trailers_online(&pool)
        .await
        .unwrap_or(false)
    {
        let policy = format!("{POLICY}; frame-src {}", super::trailers::PLAYER);
        if let Ok(value) = header::HeaderValue::from_str(&policy) {
            response
                .headers_mut()
                .insert(header::CONTENT_SECURITY_POLICY, value);
        }
    }
    response
}

fn file(content_type: &'static str, cache: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_SECURITY_POLICY, POLICY),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn serves_the_calendar_and_links_it_from_the_top_bar() {
        let request = Request::get("/web/calendar.js")
            .body(Body::empty())
            .unwrap();
        let response = routes().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("/calendar?start="));
        assert!(INDEX.contains(r##"href="#/calendar""##));
    }

    #[tokio::test]
    async fn serves_the_diagnostics_module() {
        let request = Request::get("/web/diagnostics.js")
            .body(Body::empty())
            .unwrap();
        let response = routes().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("/diagnostics?limit="));
    }

    #[tokio::test]
    async fn serves_the_page_and_its_files_with_a_strict_policy() {
        for (uri, kind, needle) in [
            ("/", "text/html", "/web/app.js"),
            ("/web/app.js", "text/javascript", "web-login"),
            ("/web/page-state.js", "text/javascript", "setSignIn"),
            ("/web/lib.js", "text/javascript", "X-Melete-Web"),
            ("/web/player.js", "text/javascript", "/transcode"),
            (
                "/web/player-keys.js",
                "text/javascript",
                "requestFullscreen",
            ),
            ("/web/player-next.js", "text/javascript", "Up next"),
            ("/web/settings.js", "text/javascript", "/metadata/unmatched"),
            ("/web/forms.js", "text/javascript", "confirmInline"),
            ("/web/sources.js", "text/javascript", "/indexers/presets/"),
            ("/web/sources-arrs.js", "text/javascript", "/import/"),
            ("/web/sources-lists.js", "text/javascript", "/lists/"),
            (
                "/web/sources-profiles.js",
                "text/javascript",
                "/custom-formats/",
            ),
            ("/web/editor-parts.js", "text/javascript", "aria-invalid"),
            (
                "/web/condition-editor.js",
                "text/javascript",
                "release_title",
            ),
            ("/web/wanted-page.js", "text/javascript", "#/wanted"),
            ("/web/wanted.js", "text/javascript", "/releases/"),
            (
                "/web/wanted-missing.js",
                "text/javascript",
                "/missing-searches",
            ),
            (
                "/web/wanted-downloads.js",
                "text/javascript",
                "/downloads/clear-failed",
            ),
            ("/web/wanted-rss.js", "text/javascript", "/rss/sync"),
            ("/web/wanted-set-aside.js", "text/javascript", "/trash/"),
            ("/web/music.js", "text/javascript", "/music/albums"),
            ("/web/music-player.js", "text/javascript", "mediaSession"),
            (
                "/web/wanted-music.js",
                "text/javascript",
                "/artist-searches/",
            ),
            (
                "/web/progress.js",
                "text/javascript",
                "/api/v1/progress/events",
            ),
            ("/web/requests.js", "text/javascript", "/requests/search"),
            ("/web/services.js", "text/javascript", "/settings/services"),
            ("/web/hls-1.7.3.mjs", "text/javascript", "isSupported"),
            ("/web/home.js", "text/javascript", "\"/home\""),
            (
                "/web/watch-hero.js",
                "text/javascript",
                "aria-roledescription",
            ),
            ("/web/watch-kind.js", "text/javascript", "anime=only"),
            ("/web/title-episodes.js", "text/javascript", "/watched"),
            ("/web/title-more.js", "text/javascript", "/extras"),
            ("/web/title-colour.js", "text/javascript", "getImageData"),
            ("/web/player-chips.js", "text/javascript", "aria-pressed"),
            (
                "/web/title-trailer.js",
                "text/javascript",
                "\"https://www.youtube-nocookie.com\"",
            ),
            ("/web/browse.js", "text/javascript", "\"exclude\""),
            ("/web/watch.css", "text/css", ".hero"),
            ("/web/app.css", "text/css", "--bg"),
            ("/web/icon.svg", "image/svg+xml", "<svg"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let headers = response.headers();
            assert!(
                headers[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with(kind)
            );
            let policy = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            assert!(policy.contains("script-src 'self';") && !policy.contains("unsafe"));
            assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains(needle), "{uri}");
        }
    }

    #[test]
    fn the_page_has_no_inline_script_or_style() {
        // The policy would block them; this says so before a browser does.
        assert!(!INDEX.contains("<style") && !INDEX.contains("style="));
        assert!(INDEX.matches("<script").count() == INDEX.matches("<script src=").count());
        for (name, body) in MODULES {
            assert!(
                !body.contains("innerHTML") && !body.contains("eval("),
                "{name}"
            );
        }
    }

    /// The top-level arguments of each `replaceChildren(` call in `body`, with the line it's on.
    /// Quoted text is skipped, so a bracket inside a string doesn't count.
    fn replace_children_arguments(body: &str) -> Vec<(usize, String)> {
        let mut found = Vec::new();
        for (at, _) in body.match_indices("replaceChildren(") {
            let line = body[..at].matches('\n').count() + 1;
            let (mut depth, mut quote, mut arg) = (1, None, String::new());
            for c in body[at + "replaceChildren(".len()..].chars() {
                match (quote, c) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, '"' | '\'' | '`') => quote = Some(c),
                    (None, '(' | '[' | '{') => depth += 1,
                    (None, ')' | ']' | '}') => depth -= 1,
                    (None, ',') if depth == 1 => {
                        found.push((line, std::mem::take(&mut arg)));
                        continue;
                    }
                    _ => {}
                }
                if depth == 0 {
                    break;
                }
                arg.push(c);
            }
            found.push((line, arg));
        }
        found
    }

    #[test]
    fn the_palette_finds_artists_and_songs_and_a_song_opens_its_album() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let palette = module("palette.js");
        assert!(palette.contains("music(`/music/artists?${query}`)"));
        assert!(palette.contains("music(`/music/tracks?${query}&q_in=track&sort=title`)"));
        assert!(palette.contains("#/music/albums/${t.album_id}?track=${t.id}"));
        assert!(module("music.js").contains(r#"albumPage(id, ctx, params.get("track"))"#));
        assert!(module("music-album.js").contains(r#"row.classList.add("found")"#));
        assert!(module("music-album.js").contains(r#"row.querySelector(".track-play")?.focus("#));
    }

    #[test]
    fn full_screen_belongs_to_the_stage_however_it_is_asked_for() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        // The video's own button stays off where browsers honour that, a double-click on the
        // picture goes full screen, and the video's own request hands over to the stage.
        assert!(module("player.js").contains(r#"controlslist: "nofullscreen""#));
        assert!(
            module("player.js").contains(r#"addEventListener("fullscreenchange", onFullscreen)"#)
        );
        assert!(
            module("player.js")
                .contains(r#"removeEventListener("fullscreenchange", onFullscreen)"#)
        );
        let keys = module("player-keys.js");
        assert!(keys.contains(r#"video.addEventListener("dblclick""#));
        assert!(keys.contains("stage.requestFullscreen"));
    }

    #[test]
    fn no_module_hands_replace_children_a_null() {
        // The DOM writes a null given to replaceChildren as the text "null", unlike the page's
        // own `h` and `append`, which leave it out.
        let mut nullable = Vec::new();
        for (name, body) in MODULES {
            for (line, arg) in replace_children_arguments(body) {
                let arg = arg.trim();
                if arg == "null" || arg.ends_with(": null") || arg.ends_with("?? null") {
                    nullable.push(format!("{name}:{line}"));
                }
            }
        }
        assert!(nullable.is_empty(), "{nullable:?}");
        // The scan itself sees a nullable argument where there is one.
        let sample = r#"nav.replaceChildren(a ? h("a", {}, "(") : null, b);"#;
        let args = replace_children_arguments(sample);
        assert_eq!(args.len(), 2);
        assert!(args[0].1.ends_with(": null"));
    }

    /// Of `dist/hls.light.min.mjs` in https://registry.npmjs.org/hls.js/-/hls.js-1.7.3.tgz, whose
    /// sha512 matched the registry's when it was vendored.
    const HLS_SHA256: &str = "c60e98c83e5b2ce6875ea6ca27e51f63b7a6f00e9d9b64b588c92261bd69d439";

    #[test]
    fn hls_js_is_the_release_it_says_it_is() {
        let digest = Sha256::digest(HLS.as_bytes());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, HLS_SHA256, "src/web/vendor/hls-1.7.3.mjs changed");
        assert!(!HLS.contains("eval(") && !HLS.contains("new Function"));
    }

    #[tokio::test]
    async fn serves_the_admin_shell_and_the_command_palette() {
        for (uri, needle) in [
            ("/web/admin.js", "#/admin/"),
            ("/web/admin-music.js", "/wanted/artists/follows"),
            ("/web/admin-overviews.js", "/requests/settings"),
            ("/web/health.js", "/admin/health"),
            ("/web/palette.js", "showModal"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert!(
                response.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("text/javascript")
            );
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains(needle), "{uri}");
        }
    }

    #[tokio::test]
    async fn serves_the_indexers_pages() {
        for (uri, needle) in [
            (
                "/web/manage-services.js",
                r#"{ key: "soulseek", name: "Soulseek" }"#,
            ),
            ("/web/manage-services.js", r#"moved: { overview: "list" }"#),
            (
                "/web/indexers.js",
                r#"["Name", "Kind", "Used by", "On", "Last result", ""]"#,
            ),
            ("/web/indexers.js", "It must share nothing."),
            ("/web/manage-parts.js", r#"role: "switch""#),
            ("/web/release-try.js", "/releases/parse?"),
            ("/web/services.js", "#/admin/indexers/soulseek"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(
                String::from_utf8_lossy(&body).contains(needle),
                "{uri}: {needle}"
            );
        }
    }

    #[tokio::test]
    async fn serves_the_system_pages() {
        for (uri, needle) in [
            (
                "/web/manage-services.js",
                r#"moved: { overview: "status" }"#,
            ),
            (
                "/web/manage-services.js",
                r#"{ key: "downloading", name: "Downloading" }"#,
            ),
            ("/web/system-status.js", "/admin/system/disk"),
            ("/web/system-upkeep.js", "/admin/system/tasks/${t.name}/run"),
            ("/web/system-upkeep.js", "/admin/system/backups"),
            ("/web/system-upkeep.js", "melete.db-wal"),
            ("/web/user-keys.js", "/users/${u.id}/keys"),
            ("/web/library-rescan.js", "/rescan"),
            ("/web/services.js", "#/admin/system/downloading"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(
                String::from_utf8_lossy(&body).contains(needle),
                "{uri}: {needle}"
            );
        }
    }

    #[tokio::test]
    async fn serves_the_shell_and_the_pieces_pages_share() {
        for (uri, needle) in [
            ("/web/shell.js", "sessionStorage"),
            ("/web/sides.js", "export function sideOf"),
            ("/web/page-parts.js", "aria-busy"),
            ("/web/sidebar.js", "showModal"),
            ("/web/manage-services.js", r#"key: "anime""#),
            ("/web/shell.css", ".state-missing .mark"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains(needle), "{uri}");
        }
    }

    #[tokio::test]
    async fn serves_each_manage_services_library() {
        for (uri, needle) in [
            ("/web/manage.js", "export const MANAGE"),
            ("/web/manage-parts.js", "/manage/anime"),
            ("/web/manage-library.js", "cutoff_unmet"),
            ("/web/manage-services.js", r#"key: "library""#),
            ("/web/manage.css", ".library-grid"),
            ("/web/manage-title.js", "Fix match"),
            ("/web/manage-search.js", "/releases/"),
            ("/web/manage-history.js", "/searches"),
            ("/web/manage-seasons.js", "/episodes/"),
            ("/web/manage-artist.js", "monitor_new"),
            ("/web/manage-album.js", "track_list"),
            ("/web/manage-add.js", "/requests/search"),
            ("/web/manage-activity.js", "/manage/anime"),
            ("/web/manage-settings.js", "How files are handled"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains(needle), "{uri}");
        }
        assert!(INDEX.contains(r#"<link rel="stylesheet" href="/web/manage.css">"#));
    }

    #[test]
    fn the_header_switches_sides_and_shows_manage_only_once_it_is_open() {
        for side in ["watch", "listen"] {
            assert!(INDEX.contains(&format!(r#"data-side="{side}">"#)), "{side}");
        }
        // Hidden until the admin health answer says this way in opens admin pages.
        assert!(INDEX.contains(r#"data-side="manage" hidden>"#));
        assert!(INDEX.contains(r##"<a href="#/requests">"##));
    }

    #[test]
    fn the_page_sits_in_a_frame_with_a_sidebar_and_a_menu_for_narrow_screens() {
        let frame = INDEX.find(r#"<div class="frame" id="frame">"#).unwrap();
        let sidebar = INDEX.find(r#"<nav class="sidebar" id="sidebar""#).unwrap();
        let toggle = INDEX.find(r#"id="fold-toggle""#).unwrap();
        let main = INDEX.find(r#"<main id="main""#).unwrap();
        assert!(frame < sidebar && sidebar < toggle && toggle < main);
        assert!(INDEX.contains(r#"aria-expanded="false" aria-controls="fold-menu""#));
    }

    #[tokio::test]
    async fn serves_the_music_bar_the_queue_and_the_lyrics() {
        for (uri, needle) in [
            ("/web/music-queue.js", "export function clearAfter"),
            ("/web/music-controls.js", "Repeat: all"),
            ("/web/music-bar.js", "Stop and close the player"),
            ("/web/music-panel.js", r#"role: "tablist""#),
            ("/web/music-lyrics.js", "/lyrics"),
            ("/web/player.css", ".music-panel:modal"),
        ] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains(needle), "{uri}");
        }
    }

    #[test]
    fn lyrics_are_translated_only_when_asked() {
        // A translation sends the song's lines to the translator, so the page asks for one only
        // after a press, never on its own.
        let lyrics = MODULES
            .iter()
            .find(|(name, _)| *name == "music-lyrics.js")
            .unwrap()
            .1;
        assert!(lyrics.contains("const options = { translate: false, pronounce: false };"));
    }

    fn module(name: &str) -> &'static str {
        MODULES.iter().find(|(n, _)| *n == name).unwrap().1
    }

    #[test]
    fn listen_opens_on_home_with_its_rows_in_order() {
        let home = module("music-home.js");
        let rows = [
            "Jump back in",
            "Discover",
            "Pinned",
            "Recently added",
            "Recently played",
            "Most played lately",
            "Made for you",
            "Moods",
            "Genres",
            "In the house",
        ];
        let at: Vec<usize> = rows
            .iter()
            .map(|r| home.find(&format!(r#", "{r}", "#)).expect(r))
            .collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{rows:?} at {at:?}");
        // `#/music` is Home; the album list has an address of its own, and its old ones lead
        // there.
        let music = module("music.js");
        assert!(
            music.contains(r#"if ((!section || section === "home") && !id) return homePage(ctx);"#)
        );
        assert!(music.contains("location.replace(`#/music/albums?${params}`)"));
        assert!(module("sides.js").contains(r##"name: "Home", href: "#/music","##));
        assert!(module("sides.js").contains(r##"name: "Albums", href: "#/music/albums","##));
    }

    #[test]
    fn listen_lists_songs_and_genres_and_starts_radio() {
        let music = module("music.js");
        for route in [
            r#"section === "songs" && !id) return songList"#,
            r#"section === "genres") return id ? genrePage"#,
            r#"section === "radio" && !id) return radioPage"#,
        ] {
            assert!(music.contains(route), "{route}");
        }
        for entry in ["Songs", "Genres", "Radio"] {
            assert!(
                module("sides.js").contains(&format!(r##"name: "{entry}", href: "#/music/"##)),
                "{entry}"
            );
        }
        // Radio asks Aoide's route what follows the seed, then the music API for those tracks.
        let radio = module("music-radio.js");
        assert!(radio.contains(r#"api("/aoide/next", { method: "POST""#));
        assert!(radio.contains("/music/tracks?${new URLSearchParams({ id: ids.join(\",\")"));
        assert!(radio.contains("err.status === 403) return RADIO_CLOSED"));
        assert!(
            module("lib.js").contains(r#"path.startsWith("/aoide/") ? path : `/api/v1${path}`"#)
        );
        for page in ["music-album.js", "music-radio.js"] {
            assert!(module(page).contains("radioButton("), "{page}");
        }
    }

    #[test]
    fn a_track_opens_its_actions_under_its_row() {
        let actions = module("music-actions.js");
        for button in [
            r#""Play next")"#,
            r#""Play last")"#,
            r#""Add to playlist")"#,
            r#""Like")"#,
            "radioButton(",
            "`Go to ${a.name}`",
            r#""Go to the album")"#,
        ] {
            assert!(actions.contains(button), "{button}");
        }
        // A like is written through the music API, and a refusal said in a sentence.
        assert!(actions.contains(r#"/like`, { method: want ? "PUT" : "DELETE" }"#));
        assert!(actions.contains("err.status === 403 ? LIKES_CLOSED"));
        // Escape closes the strip and focus goes back to the row's button.
        assert!(actions.contains(r#"if (event.key !== "Escape") return;"#));
        // Only the caller's own hand-made playlists are offered; a new one can be made.
        let pick = module("music-playlist-pick.js");
        assert!(pick.contains("all.filter((p) => p.mine && !p.smart)"));
        assert!(pick.contains(r#"api("/playlists", { method: "POST", body: { name } })"#));
        assert!(pick.contains("/items`, { method: \"POST\", body: { track_ids: trackIds } }"));
        // The album page: its colours on its head, each track with its actions.
        let album = module("music-album.js");
        assert!(album.contains(r#"head.style.setProperty("--album-bg", colours.background)"#));
        assert!(album.contains("trackActions(t, queue[at], { onAlbum: true })"));
        for button in [r#""Play")"#, r#""Shuffle")"#, r#""Add to playlist")"#] {
            assert!(album.contains(button), "{button}");
        }
    }

    #[test]
    fn liked_songs_and_playlists_are_listened_to_and_edited() {
        let music = module("music.js");
        assert!(music.contains(r#"section === "liked" && !id) return likedPage(ctx);"#));
        assert!(music.contains(
            r#"section === "playlists") return id ? playlistPage(decodeURIComponent(id), ctx) : playlistsPage(params, ctx);"#
        ));
        assert!(module("sides.js").contains(r##"{ name: "Liked songs", href: "#/music/liked""##));
        // Each page says in a sentence that the sync log is closed to a user limited to some
        // music libraries, rather than failing.
        let liked = module("music-liked.js");
        assert!(liked.contains(r#"api("/music/likes")"#));
        assert!(liked.contains("emptyState(LIKES_CLOSED)"));
        let list = module("music-playlists.js");
        assert!(list.contains("emptyState(PLAYLISTS_CLOSED)"));
        assert!(list.contains("all.filter((p) => p.folder === folder)"));
        // Only the owner's hand-made playlist is edited: rename, delete, move and remove.
        let one = module("music-playlist.js");
        assert!(one.contains("const editable = now.mine && !now.smart;"));
        for edit in [
            r#"api(path(p), { method: "PUT", body: { name: name.value.trim(), notes: notes.value.trim() || null } })"#,
            r#"api(path(p), { method: "DELETE" })"#,
            r#"change("up", { method: "PUT", body: { index: i - 1 } })"#,
            r#"change("down", { method: "PUT", body: { index: i + 1 } })"#,
            r#"change("remove", { method: "DELETE" })"#,
        ] {
            assert!(one.contains(edit), "{edit}");
        }
        // Deleting is said plainly first, with keeping it the choice that has focus.
        assert!(one.contains("setTimeout(() => keep.focus());"));
        assert!(one.contains("Its songs are worked out from its rules on your Aoide devices"));
    }

    #[test]
    fn listens_sidebar_lists_pins_and_playlists_and_forgets_them_on_sign_out() {
        let sides = module("sides.js");
        assert!(sides.contains(
            "more: listenLists().then((lists) => listenGroups(lists, hash, items(yoursEntries()))),"
        ));
        let lists = module("music-sidebar.js");
        for group in [
            r#"closed ? null : { name: "Yours", items: yours }"#,
            r#"{ name: "Pinned", items: pinned }"#,
            r#"{ name: "Playlists", items: lists }"#,
            "`Shared by ${p.owner}: ${p.name}`",
        ] {
            assert!(lists.contains(group), "{group}");
        }
        // Changes made on a page reach the sidebar at once, and the next person in the tab
        // doesn't see the last one's.
        for event in ["melete:pins", "melete:playlists"] {
            assert!(
                lists.contains(&format!(r#"document.addEventListener("{event}", forget);"#)),
                "{event}"
            );
        }
        assert!(module("music-parts.js").contains(r#"new CustomEvent("melete:pins")"#));
        let shell = module("shell.js");
        assert!(shell.contains("forgetListenLists();\n  placeSidebar(null);"));
        assert!(shell.contains(r#"for (const name of ["melete:pins", "melete:playlists"])"#));
    }

    #[test]
    fn busy_buttons_keep_the_keyboards_focus() {
        // A disabled button drops focus; one busy with what it started is held instead.
        assert!(
            module("page-parts.js").contains(r#"button.setAttribute("aria-disabled", "true");"#)
        );
        for name in [
            "music-actions.js",
            "music-radio.js",
            "music-playlist-pick.js",
            "music-playlist.js",
            "music-album.js",
            "music-browse.js",
            "music-home.js",
        ] {
            let body = module(name);
            assert!(body.contains("if (!hold("), "{name}");
            assert!(!body.contains("button.disabled = true;"), "{name}");
        }
    }

    #[test]
    fn listen_leaves_out_what_a_user_limited_to_some_music_would_be_refused() {
        // The app reads /me's answer once; the pages ask it before asking the server.
        assert!(module("app.js").contains("setMusicAccess(user);"));
        assert!(module("music-access.js").contains("all = user?.sees_all_music !== false;"));
        for (name, guard) in [
            ("music-radio.js", "if (!seesAllMusic()) return [];"),
            (
                "music-radio.js",
                r#"if (!seesAllMusic()) return show("Radio", title, emptyState(RADIO_CLOSED));"#,
            ),
            (
                "music-liked.js",
                r#"if (!seesAllMusic()) return show("Liked songs", title, emptyState(LIKES_CLOSED));"#,
            ),
            ("music-playlists.js", "if (!seesAllMusic()) return show("),
            ("music-playlist.js", "if (!seesAllMusic()) {"),
            (
                "music-home.js",
                r#"seesAllMusic() ? quiet(api("/music/household"), null) : null,"#,
            ),
            ("music-sidebar.js", r#"closed ? [] : api("/playlists")"#),
            (
                "music-actions.js",
                "open ? [pick, likeButton(track, mark, said), radio] : null,",
            ),
            ("music-album.js", "seesAllMusic() ? pick : null,"),
        ] {
            assert!(module(name).contains(guard), "{name}: {guard}");
        }
    }

    #[test]
    fn every_state_colour_is_set_for_both_themes() {
        for state in ["have", "coming", "waiting", "missing", "off", "notyet"] {
            let token = format!("--state-{state}:");
            assert_eq!(STYLES.matches(&token).count(), 2, "{token}");
        }
    }

    #[tokio::test]
    async fn serves_the_stylesheets_linked_in_order() {
        let mut last = INDEX.find(r#"href="/web/app.css""#).unwrap();
        for (name, _) in MORE_STYLES {
            let uri = format!("/web/{name}");
            let request = Request::get(&uri).body(Body::empty()).unwrap();
            let response = routes().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert!(
                response.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("text/css")
            );
            // Linked after the one before it, so its rules come later in the cascade.
            let at = INDEX.find(&format!(r#"<link rel="stylesheet" href="{uri}">"#));
            assert!(at.is_some_and(|at| at > last), "{uri}");
            last = at.unwrap();
        }
    }

    #[tokio::test]
    async fn serves_the_profile_editor() {
        let request = Request::get("/web/profile-editor.js")
            .body(Body::empty())
            .unwrap();
        let response = routes().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("/custom-formats"));
    }
}
