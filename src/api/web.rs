//! The web UI: one page and its scripts and styles, built into the binary. The
//! page talks to the same API as the apps, signed in by cookie.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

mod jassub;

const INDEX: &str = include_str!("../web/index.html");
const STYLES: &str = include_str!("../web/app.css");
/// The styles that follow app.css, in the order the page links them: later rules win, so the
/// order matters.
const MORE_STYLES: [(&str, &str); 8] = [
    ("library.css", include_str!("../web/library.css")),
    ("tools.css", include_str!("../web/tools.css")),
    ("shell.css", include_str!("../web/shell.css")),
    ("player.css", include_str!("../web/player.css")),
    ("manage.css", include_str!("../web/manage.css")),
    ("listen.css", include_str!("../web/listen.css")),
    ("watch.css", include_str!("../web/watch.css")),
    ("title.css", include_str!("../web/title.css")),
];
/// The tab's icon, in the page's ink and brass.
const ICON: &str = include_str!("../web/icon.svg");
/// The page's modules, by the name they're served under in `/web/`.
const MODULES: [(&str, &str); 134] = [
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
    ("watch-live.js", include_str!("../web/watch-live.js")),
    ("browse.js", include_str!("../web/browse.js")),
    ("title-page.js", include_str!("../web/title-page.js")),
    ("title-colour.js", include_str!("../web/title-colour.js")),
    (
        "title-episodes.js",
        include_str!("../web/title-episodes.js"),
    ),
    ("title-more.js", include_str!("../web/title-more.js")),
    ("title-trailer.js", include_str!("../web/title-trailer.js")),
    ("title-preview.js", include_str!("../web/title-preview.js")),
    ("people.js", include_str!("../web/people.js")),
    ("lib.js", include_str!("../web/lib.js")),
    ("player.js", include_str!("../web/player.js")),
    ("player-choice.js", include_str!("../web/player-choice.js")),
    ("player-keys.js", include_str!("../web/player-keys.js")),
    ("player-next.js", include_str!("../web/player-next.js")),
    ("player-chips.js", include_str!("../web/player-chips.js")),
    (
        "player-controls.js",
        include_str!("../web/player-controls.js"),
    ),
    ("player-menus.js", include_str!("../web/player-menus.js")),
    ("player-report.js", include_str!("../web/player-report.js")),
    ("player-jump.js", include_str!("../web/player-jump.js")),
    ("player-ass.js", include_str!("../web/player-ass.js")),
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
    ("player-resume.js", include_str!("../web/player-resume.js")),
    (
        "player-subtitle-size.js",
        include_str!("../web/player-subtitle-size.js"),
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
    ("music-flags.js", include_str!("../web/music-flags.js")),
    ("music-hidden.js", include_str!("../web/music-hidden.js")),
    ("music-radio.js", include_str!("../web/music-radio.js")),
    ("music-replay.js", include_str!("../web/music-replay.js")),
    ("music-actions.js", include_str!("../web/music-actions.js")),
    ("music-liked.js", include_str!("../web/music-liked.js")),
    (
        "music-album-likes.js",
        include_str!("../web/music-album-likes.js"),
    ),
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
    ("picture-file.js", include_str!("../web/picture-file.js")),
    (
        "profile-picture.js",
        include_str!("../web/profile-picture.js"),
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
    ("music-decks.js", include_str!("../web/music-decks.js")),
    (
        "music-handover.js",
        include_str!("../web/music-handover.js"),
    ),
    ("music-mix.js", include_str!("../web/music-mix.js")),
    (
        "music-measures.js",
        include_str!("../web/music-measures.js"),
    ),
    (
        "music-settings.js",
        include_str!("../web/music-settings.js"),
    ),
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
    ("search.js", include_str!("../web/search.js")),
    ("trending.js", include_str!("../web/trending.js")),
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
    ("manage-want.js", include_str!("../web/manage-want.js")),
    ("manage-select.js", include_str!("../web/manage-select.js")),
    ("manage-add.js", include_str!("../web/manage-add.js")),
    ("manage-find.js", include_str!("../web/manage-find.js")),
    (
        "manage-add-form.js",
        include_str!("../web/manage-add-form.js"),
    ),
    (
        "manage-activity.js",
        include_str!("../web/manage-activity.js"),
    ),
    (
        "manage-make-room.js",
        include_str!("../web/manage-make-room.js"),
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
    router.merge(jassub::routes())
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
            ("/web/title-trailer.js", "text/javascript", "Play trailer"),
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

    /// Your requests come in stages, each with its poster, who decided and when, and where an
    /// approved one is coming from; Ask results have their posters too.
    #[test]
    fn requests_say_who_decided_where_they_come_from_and_their_stage() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let row = module("requests-row.js");
        assert!(row.contains(r#"[verb, r.decided_by ? ` by ${r.decided_by}` : null"#));
        assert!(row.contains(r#"["Approved at once""#));
        assert!(row.contains(r#"torrent: "From a torrent", soulseek: "From Soulseek""#));
        assert!(row.contains("[stepBar(r.progress), whence(r.progress)]"));
        assert!(row.contains(r#"{ class: "pictured" },"#) && row.contains("picture(r),"));
        let requests = module("requests.js");
        for stage in [
            "On the way",
            "Waiting for approval",
            "In the library",
            "Declined",
        ] {
            assert!(requests.contains(stage), "{stage}");
        }
        assert!(requests.contains("rows.get(stage(r)).push(requestRow("));
        assert!(requests.contains("picture(c),"));
        // An admin's choice of where it goes sits under the words, not in the poster's frame.
        assert!(
            module("requests-admin.js")
                .contains(r#"row.querySelector(":scope > .art + div").append(choose);"#)
        );
        let css = MORE_STYLES
            .iter()
            .find(|(n, _)| *n == "tools.css")
            .unwrap()
            .1;
        assert!(css.contains(".rows > li.pictured"));
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
    fn the_player_has_its_own_controls_over_the_picture() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let player = module("player.js");
        // Melete's controls stand in for the browser's, on the stage, so they go full screen.
        assert!(!player.contains("controls: true"));
        assert!(player.contains("controlBar(s.video, stage, { seek, panels, fullscreen })"));
        assert!(player.contains("controls.stop();") && player.contains("panels.stop();"));
        let controls = module("player-controls.js");
        for label in [
            "\"Play\"",
            "\"Pause\"",
            "\"Mute\"",
            "\"Full screen\"",
            "\"Exit full screen\"",
        ] {
            assert!(controls.contains(label), "{label}");
        }
        // They fade only while playing and idle, and keyboard focus keeps them.
        assert!(controls.contains("const HIDE_AFTER_MS = 3000;"));
        assert!(controls.contains("el.querySelector(\":focus-visible\")"));
        // Each panel is a disclosure: Escape closes it and gives focus back to its button.
        let menus = module("player-menus.js");
        assert!(menus.contains("\"aria-expanded\": \"false\", \"aria-controls\": id"));
        assert!(menus.contains("if (event.key !== \"Escape\") return;"));
        assert!(menus.contains("button.focus();"));
        assert!(menus.contains("document.removeEventListener(\"pointerdown\", outside)"));
        // Fading and moving stop for those who ask for less motion; a phone keeps what fits.
        let css = MORE_STYLES
            .iter()
            .find(|(n, _)| *n == "watch.css")
            .unwrap()
            .1;
        assert!(css.contains(".stage.idle .control-bar { opacity: 0; pointer-events: none; }"));
        assert!(css.contains("  .control-bar, .stage-over { transition: none; }"));
        assert!(css.contains("  .volume, button.control.control-wide { display: none; }"));
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

    /// A collection's film that isn't in the library and that nobody asked for can be asked for
    /// from the collection's page, by its TMDB id, as a movie.
    #[test]
    fn a_collections_missing_films_can_be_asked_for() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let people = module("people.js");
        assert!(people.contains(r#"item.state === "requestable" ? askFor(item, state) : null"#));
        assert!(people.contains(r#"body: { kind: "movie", tmdb_id: item.tmdb_id }"#));
        assert!(people.contains("state.textContent = STATES.requested;"));
    }

    #[test]
    fn subtitles_come_in_four_sizes_kept_by_the_browser_for_plain_and_styled_tracks() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        assert!(
            module("player.js").contains("sizePicker(s.video, (scale) => scaleStyled(s, scale))")
        );
        let size = module("player-subtitle-size.js");
        for label in ["\"Small\"", "\"Medium\"", "\"Large\"", "\"Extra large\""] {
            assert!(size.contains(label), "{label}");
        }
        // Kept per browser, and a browser with storage turned off still plays.
        assert!(size.contains("try {\n    const kept = localStorage.getItem(KEY);"));
        assert!(size.contains("try {\n    localStorage.setItem(KEY, size.size);"));
        assert!(size.contains("announce(`Subtitle size: ${size.label}`)"));
        // Text tracks through ::cue, styled ones by their styles' font sizes.
        let css = MORE_STYLES
            .iter()
            .find(|(n, _)| *n == "watch.css")
            .unwrap()
            .1;
        assert!(css.contains(r#".stage video[data-subtitle-size="extra-large"]::cue"#));
        assert!(module("player-ass.js").contains("FontSize: style.FontSize * scale"));
    }

    #[test]
    fn cast_and_crew_show_their_pictures_or_initials() {
        let people = module("people.js");
        // Each credit's picture, read as it nears the screen, said by the name beside it.
        assert!(people.contains(r#"src, alt: "", loading: "lazy""#));
        assert!(people.contains("personItem(c.name, c.person_id, c.job, c.profile)"));
        assert!(people.contains("personItem(c.name, c.person_id, c.character, c.profile)"));
        // Initials stand in when there's none, or it can't be read.
        assert!(people.contains("if (!src) return letters();"));
        assert!(people.contains(
            r#"img.addEventListener("error", () => img.replaceWith(letters()), { once: true });"#
        ));
    }

    #[test]
    fn a_title_partway_through_can_be_played_from_the_start() {
        let page = module("title-page.js");
        // Beside a movie's Resume, and a series' Resume of the episode partway through.
        assert!(page.contains("fromStart: playHref(file.id, { movie: m.id }, true)"));
        assert!(page.contains(
            "playHref(going.file_id, { series: s.id, season: going.season, episode: going.episode }, true)"
        ));
        // Hidden unless Play resumes, and labelled with the title.
        assert!(page.contains("restart.hidden = !target?.fromStart;"));
        assert!(page.contains("`From the start: ${title}`"));
    }

    #[test]
    fn previews_start_with_sound_only_when_the_browser_asks_and_may() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let prefs = module("playback-prefs.js");
        assert!(prefs.contains("\"Play previews with sound\""));
        assert!(prefs.contains("Kept by this browser only."));
        // Kept per browser, and off when storage is turned off.
        assert!(
            prefs.contains("try {\n    return localStorage.getItem(PREVIEW_SOUND_KEY) === \"on\";")
        );
        assert!(
            prefs.contains("try {\n    if (on) localStorage.setItem(PREVIEW_SOUND_KEY, \"on\");")
        );
        // Sound only after the page has had a click or a key, and muted when refused anyway.
        let hero = module("watch-hero-trailer.js");
        assert!(
            hero.contains(
                "setMuted(!(previewSound() && navigator.userActivation?.hasBeenActive));"
            )
        );
        assert!(hero.contains("setMuted(true);\n        video.play().catch(() => {});"));
    }

    #[test]
    fn recaps_intros_and_previews_are_passed_over_once_a_play_when_the_user_asks() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let next = module("player-next.js");
        // Credits are never passed over this way; the rest once each.
        assert!(next.contains("[\"Skip credits\", segments?.credits, null],"));
        assert!(next.contains("let skipped = !passedOver;"));
        assert!(next.contains("if (within && auto && !skipped) {\n        skipped = true;"));
        // Off until the user's preference says otherwise, asked once for Skip and Up next.
        assert!(next.contains("auto_skip: false };"));
        assert!(next.contains("(auto = Boolean(prefs.auto_skip))"));
        assert!(module("playback-prefs.js").contains("auto_skip: skipping.input.checked"));
    }

    #[test]
    fn the_player_keeps_what_it_saw_and_sends_it_only_when_asked() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let player = module("player.js");
        assert!(player.contains("s.log = playbackLog(video, Number(fileId));"));
        assert!(player.contains("s.log.note(\"mode\", { mode: next });"));
        assert!(player.contains("s.log.note(\"conversion_error\""));
        assert!(player.contains("reportButton(s.log, s.ctx)"));
        assert!(module("account.js").contains("reportGroup(ctx),"));
        let report = module("player-report.js");
        // A short ring, kept under the server's 64 KiB, posted only from the button.
        assert!(report.contains("const MOST = 200;"));
        assert!(report.contains("if (log.events.length > MOST) log.events.shift();"));
        assert!(report.contains("JSON.stringify(body).length > MOST_BYTES"));
        assert_eq!(report.matches("api(\"/me/diagnostics\"").count(), 1);
        assert!(report.contains("button.addEventListener(\"click\", async () => {"));
        // What kind of conversion error, never its address, which can carry a session.
        assert!(!player.contains("url: data.") && !report.contains("currentSrc"));
        for event in [
            "\"waiting\"",
            "\"stalled\"",
            "\"error\"",
            "\"seek\"",
            "\"subtitles\"",
        ] {
            assert!(report.contains(event), "{event}");
        }
    }

    #[test]
    fn the_hero_wide_tiles_and_grid_cards_wear_a_tiles_badge() {
        // Wide tiles as well as posters, by whether the series is being watched.
        let tiles = module("watch-tiles.js");
        assert!(tiles.contains("const badge = badgeOf(item, watching.has(item.id));"));
        assert!(tiles.contains("wideTile(item, playsAt(item), badge)"));
        // The hero, at the end of its facts.
        let hero = module("watch-hero.js");
        assert!(hero.contains("const badge = badgeOf(item, watching.has(item.id));"));
        assert!(hero.contains(r#"h("li", { class: "hero-badge" }, badge)"#));
        // The grids of all shows, anime and movies, which know the series being watched from
        // their page's home; list items don't say their kind, so it's added.
        let browse = module("browse.js");
        assert!(browse.contains(
            r#"badgeOf({ ...item, kind: kind === "movies" ? "movie" : "series" }, kind === "series" && watching.has(item.id))"#
        ));
        assert!(
            module("watch-kind.js")
                .contains("everything(page, params, gen, { genres, libraries, ai, watching })")
        );
        assert!(module("cards.js").contains("export function titleCard(kind, item, badge = null)"));
    }

    #[test]
    fn a_title_pages_header_plays_its_preview_by_the_heros_rules() {
        // The hero's own player, so its rules hold: the setting, the home network, reduced
        // motion, muted with Unmute and Stop.
        let preview = module("title-preview.js");
        assert!(
            preview.contains(r#"const trailer = heroTrailer(header.querySelector(".hero-art"));"#)
        );
        // The trailer only when it's sharp enough to stand in for the title.
        assert!(
            preview
                .contains("const needed = Math.min(1080, own.length ? Math.max(...own) : 1080);")
        );
        assert!(preview.contains("return tall >= needed ? trailer.id : null;"));
        // Only while the header is in view, and gone with the page.
        assert!(preview.contains("else trailer.stop();"));
        assert!(preview.contains("seen.disconnect();\n    trailer.close();"));
        // Closing it, as leaving a page does, lets go of what it listened to.
        let player = module("watch-hero-trailer.js");
        assert!(player.contains(r#"removeEventListener("visibilitychange", hidden);"#));
        assert!(module("watch-hero.js").contains("p.trailer.close();"));
        let page = module("title-page.js");
        assert_eq!(
            page.matches("leaving.push(headerPreview(head.el, item, ")
                .count(),
            2
        );
    }

    #[test]
    fn watch_pages_follow_the_library_and_my_list_live() {
        // One stream, its kinds' first values only saying where things stand (unless first heard
        // well after connecting), closed on leaving.
        let live = module("watch-live.js");
        assert!(live.contains(r#"source = new EventSource("/api/v1/events");"#));
        assert!(live.contains(r#"for (const kind of ["library", "favourites"])"#));
        assert!(live.contains(
            "const changed = before === undefined ? Date.now() - opened > FIRST_MS : before !== version;"
        ));
        assert!(live.contains("source?.close();"));
        // Redrawn in place: the focus and each row's sideways scroll stay.
        assert!(live.contains("again?.focus({ preventScroll: true });"));
        assert!(live.contains("if (left) shelf.scrollLeft = left;"));
        for page in ["home.js", "watch-kind.js", "favourites.js"] {
            assert!(module(page).contains("followWatch(gen, {"), "{page}");
        }
        assert!(module("browse.js").contains("follow?.(() => reload(v, { quietly: true })"));
        // My List's own Remove and Undo aren't undone by their echo.
        assert!(
            module("favourites.js")
                .contains("if (again && drawn && sameList(drawn, next)) return;")
        );
    }

    #[test]
    fn home_draws_its_saved_copy_first_and_forgets_it_with_the_session() {
        let home = module("home.js");
        // Kept under the user's id, read and written with storage allowed to fail.
        assert!(home.contains(r#"const SAVED = "melete.home.";"#));
        assert!(
            home.contains("return JSON.parse(localStorage.getItem(SAVED + user) ?? \"null\");")
        );
        assert!(home.contains("localStorage.setItem(SAVED + user, JSON.stringify(answer));"));
        assert_eq!(home.matches("  } catch {\n").count(), 3);
        // Drawn first; a failed refresh leaves it up.
        assert!(home.contains("if (saved) draw(page, saved);"));
        assert!(home.contains("if (!saved) show(\"Home\", h(\"h1\", {}, \"Home\"), failed("));
        // Every saved copy goes whenever the sign-in form comes back: signing out, an ended
        // session, or opening the page signed out.
        let app = module("app.js");
        assert!(app.contains("  stopShell();\n  // The next to sign in here may be someone else.\n  forgetSavedHomes();"));
        assert!(app.contains("if (!kind) return home(gen, me.id);"));
    }

    #[test]
    fn picking_up_starts_as_many_seconds_early_as_the_browser_keeps() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        assert!(module("player.js").contains("const start = startAt(play, restart);"));
        let resume = module("player-resume.js");
        // Never before the start, and nothing taken off when starting over.
        assert!(resume.contains("if (restart || !(play.resume_seconds > 0)) return 0;"));
        assert!(resume.contains("Math.max(play.resume_seconds - resumeEarlier(), 0)"));
        let prefs = module("playback-prefs.js");
        for label in [
            "\"Where I stopped\"",
            "\"5 seconds earlier\"",
            "\"30 seconds earlier\"",
        ] {
            assert!(prefs.contains(label), "{label}");
        }
        // Kept per browser, and a browser with storage turned off picks up where it stopped.
        assert!(
            prefs.contains("try {\n    const kept = Number(localStorage.getItem(RESUME_KEY));")
        );
        assert!(prefs.contains("try {\n    localStorage.setItem(RESUME_KEY, String(seconds));"));
        assert!(prefs.contains("Kept by this browser only."));
    }

    #[test]
    fn a_quality_limit_kept_by_the_browser_converts_a_file_over_it() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let menus = module("player-menus.js");
        for label in ["\"Original\"", "\"Up to 40 Mbps\"", "\"Up to 3 Mbps\""] {
            assert!(menus.contains(label), "{label}");
        }
        // Kept per browser, and a browser with storage turned off still plays.
        assert!(
            menus.contains("try {\n    const kept = Number(localStorage.getItem(QUALITY_KEY));")
        );
        assert!(
            menus.contains("try {\n    if (bits) localStorage.setItem(QUALITY_KEY, String(bits));")
        );
        let player = module("player.js");
        assert!(player.contains("quality: keptQuality()"));
        assert!(player.contains("qualityPicker(s.choice.quality, requality)"));
        // Over the viewer's limit the video is converted, held to it less the sound's share; the
        // way in's own limit still counts.
        let choice = module("player-choice.js");
        assert!(choice.contains("...(held && codec !== \"copy\" ? { max_bitrate: held } : {}),"));
        assert!(
            choice.contains("overLimit(info, info.max_bitrate) || held) return request(\"h264\")")
        );
        assert!(choice.contains("Math.max(quality - AUDIO_ALLOWANCE, 100_000)"));
    }

    #[test]
    fn each_jump_shows_how_far_it_went_and_adds_up() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        // The buttons and the keys jump the same way, so both show it.
        let controls = module("player-controls.js");
        assert!(
            controls.contains("() => jump(video, -STEP)")
                && controls.contains("() => jump(video, STEP)")
        );
        assert!(
            controls.contains("const flash = jumpFlash(video);")
                && controls.contains("flash.stop();")
        );
        assert!(module("player-keys.js").contains("const by = (seconds) => jump(video, seconds);"));
        let flash = module("player-jump.js");
        // A run adds up, said once to screen readers when it settles; the picture's flash is
        // hidden from them. Reduced motion keeps the flash and drops its animation.
        assert!(flash.contains("total += seconds;"));
        assert!(flash.contains(
            "announce(`${total > 0 ? \"Forward\" : \"Back\"} ${Math.abs(total)} seconds`)"
        ));
        assert!(flash.contains("\"aria-hidden\": \"true\""));
        assert!(flash.contains("if (!stillness.matches) el.animate("));
    }

    #[test]
    fn a_tile_shown_in_the_hero_plays_its_preview_there() {
        // Picks and tiles alike go through the hero's preview player, with its rules.
        let hero = module("watch-hero.js");
        assert!(hero.contains("  aimPlay(p, item);\n  // Its preview"));
        assert!(
            hero.contains("  if (p.inView) p.trailer.show(item);\n  else p.trailer.stop();\n}")
        );
        assert!(!hero.contains("p.picks.includes(item)"));
        // Only while the hero can be seen, so a tile low on the page plays nothing off screen.
        assert!(hero.contains("if (!p.inView) p.trailer.stop();"));
        assert!(hero.contains("seen.disconnect();"));
        let player = module("watch-hero-trailer.js");
        assert!(player.contains("if (stillness.matches || document.hidden || prefs.hero_trailers === \"off\") return false;"));
        assert!(player.contains("if (!home) return null;"));
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

    /// The search page asks TMDB for titles not in the library only once discovery says it's
    /// on, and the palette's last result leads there.
    #[test]
    fn search_asks_tmdb_only_while_discovery_is_on() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let search = module("search.js");
        let status = search.find("api(\"/discover/status\")").unwrap();
        let gate = search
            .find("v.discovery && q.length >= 2 ? findOutside")
            .unwrap();
        let tmdb = search.find("/discover/search?").unwrap();
        assert!(status < gate && search.contains("discovery: false,"));
        assert!(tmdb > search.find("async function findOutside").unwrap());
        assert_eq!(search.matches("/discover/search").count(), 1);
        assert!(search.contains("candidateRow(ctx, candidate(d))"));
        assert!(module("requests.js").contains("export function candidateRow"));
        assert!(module("palette.js").contains("#/search?${new URLSearchParams({ q: typed })}"));
        assert!(module("app.js").contains("searchPage(params, gen)"));
    }

    /// Trending is asked for only once discovery says it's on (the page's own answer, or
    /// /discover/status), on the search page before anything is typed and on the Ask page.
    #[test]
    fn trending_is_asked_for_only_while_discovery_is_on() {
        let module = |name: &str| MODULES.iter().find(|(n, _)| *n == name).unwrap().1;
        let trending = module("trending.js");
        let status = trending
            .find("known ?? (await api(\"/discover/status\")).on")
            .unwrap();
        let gate = trending.find("if (!on) return;").unwrap();
        let chart = trending.find("/discover/trending?").unwrap();
        assert!(status < gate && gate < chart);
        assert!(trending.contains("body: { kind: item.kind, tmdb_id: item.tmdb_id }"));
        let search = module("search.js");
        assert!(search.contains("if (v.discovery) v.start.append(trendingSection("));
        assert!(search.contains("v.start.hidden = Boolean(q);"));
        assert!(module("requests.js").contains("trendingSection(ctx, { id: \"ask-trending\""));
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
            ("/web/manage-want.js", "As Where they go says"),
            ("/web/manage-title.js", "/web/manage-want.js"),
            ("/web/manage-artist.js", "/web/manage-want.js"),
            ("/web/manage-album.js", "/web/manage-want.js"),
            ("/web/manage-select.js", "/selection"),
            ("/web/manage-library.js", "all: () => selectAll(v)"),
            ("/web/manage-find.js", "/requests/search"),
            ("/web/manage-find.js", r#"["song", "Songs"]"#),
            ("/web/manage-find.js", "/requests/soulseek"),
            ("/web/manage-add-form.js", "find-add"),
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

    /// Add new looks for songs on Soulseek while slskd is set up, only once typing has stopped
    /// for longer than MusicBrainz's pause and for three letters or more, and ranks its songs
    /// with MusicBrainz's by the server's closeness, each row saying where it's from.
    #[tokio::test]
    async fn add_new_mixes_soulseeks_songs_with_musicbrainzs_by_closeness() {
        for (uri, needle) in [
            ("/web/manage-find.js", r#"s.key === "slskd" && s.on"#),
            ("/web/manage-find.js", "const SEEK_PAUSE_MS = 1200;"),
            ("/web/manage-find.js", "const SEEK_FEWEST = 3;"),
            ("/web/manage-find.js", "/requests/soulseek/candidates"),
            ("/web/manage-find.js", "if (v.q.value.trim() !== words) {"),
            (
                "/web/manage-find.js",
                "[...found, ...heard].sort((a, b) => b.closeness - a.closeness)",
            ),
            (
                "/web/manage-add-form.js",
                r#"h("span", { class: "badge" }, c.source)"#,
            ),
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

    /// Add new's one Soulseek search per pause serves albums too: MusicBrainz's albums for the
    /// words go with it to Melete, an album row says who shares it on Soulseek, and the Albums
    /// list is ranked by the server's closeness once Soulseek's users share any.
    #[tokio::test]
    async fn add_new_marks_musicbrainzs_albums_that_soulseeks_users_share() {
        for (uri, needle) in [
            (
                "/web/manage-find.js",
                r#"api("/requests/soulseek/candidates", { method: "POST", body: { q: words, albums } })"#,
            ),
            (
                "/web/manage-find.js",
                "asked.albums = new Map(heard.albums.map((a) => [a.mb_release_group_id, a.detail]));",
            ),
            (
                "/web/manage-find.js",
                r#"g.kind === "album" && shares && g.found ? { ...g, found: ranked(g.found, []) }"#,
            ),
            (
                "/web/manage-find.js",
                r#"row.querySelector(".find-shared")"#,
            ),
            (
                "/web/manage-add-form.js",
                r#"c.kind === "album" ? h("p", { class: "note find-shared", hidden: true }) : null"#,
            ),
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
    fn a_hand_over_keeps_one_listen_per_play() {
        let player = module("music-player.js");
        // The song handed over from ends its listen, heard to the end, before the next one's
        // deck takes over; the next one's starts once, whichever way it began.
        let handed = &player[player.find("function handedOver(").unwrap()..];
        let ended = handed.find("endListen({ completed: true });").unwrap();
        assert!(ended < handed.find("swap();").unwrap());
        assert!(handed.contains("if (sounding) beginListen();"));
        assert!(player.contains("if (!track || listen) return;"));
        assert!(player.contains(r#"audio.addEventListener("playing", beginListen);"#));
        // A song's end taken by a hand-over isn't also ended and loaded the plain way.
        assert!(player.contains("if (finish()) return;\n  endListen({ completed: true });"));
        // The pages, and the player, hear only the deck playing.
        assert!(
            module("music-decks.js")
                .contains("deck === active() && audio.dispatchEvent(new Event(type))")
        );
    }

    #[test]
    fn crossfade_follows_the_apps_rules_and_is_kept_by_the_browser() {
        let mix = module("music-mix.js");
        for rule in [
            "MATCHED_BLEND = 8;",
            "UNMATCHED_BLEND = 4;",
            "SHARE = 0.25;",
            "SHORTEST = 2;",
            "TOLERANCE = 0.06;",
            "STEADY = 0.5;",
            r#"if (keepAlbums && albumRun(from, to)) return { kind: "gapless", overlap: 0 };"#,
            r#"if (!crossfade) return { kind: "cut", overlap: 0 };"#,
        ] {
            assert!(mix.contains(rule), "{rule}");
        }
        let settings = module("music-settings.js");
        assert!(settings.contains("const DEFAULTS = { crossfade: false, keepAlbums: true };"));
        // A browser with its storage turned off still plays, with the defaults.
        for read in [
            "try {\n    const kept = JSON.parse(localStorage.getItem(KEY)",
            "try {\n    localStorage.setItem(KEY, JSON.stringify(choices));",
        ] {
            assert!(settings.contains(read), "{read}");
        }
        assert!(module("account.js").contains("listeningGroup(),"));
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
        // Albums are liked on their page and listed on a page of their own.
        assert!(
            music.contains(r#"section === "liked-albums" && !id) return likedAlbumsPage(ctx);"#)
        );
        assert!(
            module("sides.js")
                .contains(r##"{ name: "Liked albums", href: "#/music/liked-albums""##)
        );
        assert!(liked.contains(r#"show("Liked albums", title, emptyState(LIKES_CLOSED))"#));
        assert!(module("music-album.js").contains("seesAllMusic() ? albumLike(album) : null"));
        let actions = module("music-actions.js");
        assert!(actions.contains(
            r#"api(`/music/albums/${album.id}/like`, { method: want ? "PUT" : "DELETE" })"#
        ));
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
    fn album_cards_have_a_heart_that_follows_the_albums_like() {
        assert!(module("music-parts.js").contains("    cardLike(album),\n"));
        let hearts = module("music-album-likes.js");
        for part in [
            "if (!seesAllMusic()) return null;",
            r#""aria-label": `Like ${album.title}`"#,
            r#"button.setAttribute("aria-pressed", String(liked));"#,
            r#"api(`/music/albums/${album.id}/like`, { method: want ? "PUT" : "DELETE" })"#,
            "err.status === 403 ? LIKES_CLOSED",
            // Shown once the likes are known, never saying "not liked" of a liked album.
            "button.hidden = false;",
        ] {
            assert!(hearts.contains(part), "{part}");
        }
        // The album page's Like moves its cards' hearts too, and signing out forgets the likes.
        assert!(module("music-actions.js").contains("albumLikeChanged(album.id, want);"));
        assert!(module("shell.js").contains("forgetAlbumLikes();"));
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
    fn hidden_songs_are_closed_to_a_user_limited_to_some_music() {
        assert!(module("music-hidden.js").contains(
            r#"if (!seesAllMusic()) return show("Hidden songs", title, emptyState(SYNC_CLOSED));"#
        ));
        // The strip offers the flags only where likes are open too.
        assert!(module("music-actions.js").contains("open ? flagButtons(track, said) : null,"));
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

    #[test]
    fn the_pages_know_the_check_by_the_servers_own_words() {
        // The pages tell a download being checked against its title from its message.
        let progress = MODULES
            .iter()
            .find(|(name, _)| *name == "progress.js")
            .map(|(_, body)| *body)
            .unwrap();
        let opening: String = crate::transfer::CHECKING.chars().take(29).collect();
        assert_eq!(opening, "checking it's the right title");
        assert!(progress.contains(&format!("startsWith(\"{opening}\")")));
    }
}
