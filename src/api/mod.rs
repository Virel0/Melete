//! The HTTP API under `/api/v1`, the Aoide sidecar's routes under `/aoide`, and the web UI,
//! put together into one router. The contract the apps are built against is `openapi.json`,
//! served at `/api/v1/openapi.json`.

mod access;
mod admin_health;
mod ai;
mod aoide;
mod aoide_library;
mod aoide_match;
mod aoide_next;
mod aoide_sound;
mod artist_search;
mod auth;
mod blocklist;
mod calendar;
mod credits;
mod diagnostics;
mod discover;
mod downloads;
mod events;
mod favourites;
mod import;
mod indexers;
mod languages;
mod listen;
mod lists;
mod local_artwork;
mod manage;
mod matching;
mod media;
mod missing_searches;
mod music;
mod music_classical;
mod music_home;
mod music_household;
mod music_likes;
mod music_lyrics;
mod music_match;
mod music_mixes;
mod music_motion;
mod music_pins;
mod music_play;
mod music_plays;
mod music_profiles;
mod music_stems;
mod music_transitions;
mod next_up;
mod offline;
mod oidc;
mod playback;
mod playlists;
mod preferences;
mod profiles;
mod progress;
mod releases;
mod requests;
mod rss;
mod runtime_checks;
mod search;
mod segments;
mod services;
mod songs;
mod speedtest;
mod stems;
mod subtitle_search;
mod subtitles;
mod system;
mod title_artwork;
mod trailers;
mod transcode;
mod trickplay;
mod wanted_removed;
mod watch_home;
mod web;
mod wrong_file;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::tmdb;

mod errors;
mod libraries;
mod root_routes;
mod route_groups;

use errors::*;
use libraries::*;
pub use root_routes::*;
use route_groups::*;

/// The API description, kept next to the code it describes. Tests check that every route in
/// it exists and that responses carry exactly the fields it lists.
const OPENAPI: &str = include_str!("openapi.json");

/// The outside services the API reaches itself; each is off (`None`) unless configured.
pub struct Outside {
    /// TMDB's image server, for artwork.
    pub images: tmdb::Images,
    /// TMDB, for metadata, discovery and matching by hand.
    pub tmdb: Option<Arc<tmdb::Client>>,
    /// The Cover Art Archive, for album covers; on with MusicBrainz matching.
    pub coverart: Option<Arc<crate::coverart::Client>>,
    /// MusicBrainz, shared with the background matching, for matching albums by hand.
    pub musicbrainz: Option<Arc<crate::musicbrainz::Client>>,
    /// slskd, for searching Soulseek; on when it's set up.
    pub slskd: Option<Arc<crate::slskd::Client>>,
    /// Where tracks' parts are kept, when the stem separator is set up.
    pub stems: Option<PathBuf>,
    /// LRCLIB, for tracks' lyrics, when lyrics are turned on.
    pub lyrics: Option<Arc<crate::lrclib::Client>>,
    /// LibreTranslate, for translating lyrics, when it's set up.
    pub translator: Option<Arc<crate::libretranslate::Client>>,
    /// An identity provider to sign in through, when one is set up.
    pub oidc: Option<Arc<crate::oidc::Client>>,
    /// OpenRouter, for AI features, when its key is set.
    pub ai: Option<Arc<crate::ai::Client>>,
}

/// What the API keeps in memory that must outlast it being set up afresh when outside
/// services' settings are saved: made once at startup and handed to every API built
/// after. Without it a save would forgive every lockout and drop every sign-in under way.
#[derive(Clone, Default)]
pub struct Kept {
    /// Wrong passwords, counted for every password check together.
    pub limiter: Arc<crate::auth::LoginLimiter>,
    /// Wrong listening-together codes, counted as wrong passwords are, apart from them.
    pub join_codes: Arc<crate::auth::LoginLimiter>,
    /// Quick Connect's codes waiting for approval.
    pub quick_connect: Arc<crate::quick_connect::QuickConnect>,
    /// Sign-ins under way at an identity provider; [`crate::oidc::Client`]s keep them here.
    pub oidc: Arc<crate::oidc::Pending>,
    /// Each user's AI searches in the last hour.
    pub ai_searches: Arc<ai::Searches>,
    /// What AI picks may spend: each user's calls in the last hour, and those running.
    pub ai_budget: Arc<crate::ai::Budget>,
}

/// Every route the server answers, keeping nothing from an API built before.
#[cfg(test)]
pub fn server(
    pool: SqlitePool,
    data_dir: &std::path::Path,
    outside: Outside,
    ffprobe: PathBuf,
    transcoder: Arc<crate::transcode::Transcoder>,
    downloads: Option<Arc<crate::downloads::Pipeline>>,
    reload: crate::live::Reload,
) -> Router {
    server_with(
        pool,
        data_dir,
        outside,
        ffprobe,
        transcoder,
        downloads,
        reload,
        &Kept::default(),
    )
}

/// The whole API, with what it keeps in memory held in `kept`. Artwork and converted
/// subtitles are cached in `data_dir`.
#[allow(clippy::too_many_arguments)]
pub fn server_with(
    pool: SqlitePool,
    data_dir: &std::path::Path,
    outside: Outside,
    ffprobe: PathBuf,
    transcoder: Arc<crate::transcode::Transcoder>,
    downloads: Option<Arc<crate::downloads::Pipeline>>,
    reload: crate::live::Reload,
    kept: &Kept,
) -> Router {
    let Outside {
        images,
        tmdb,
        coverart,
        musicbrainz,
        slskd,
        stems: stems_dir,
        lyrics,
        translator,
        oidc: oidc_client,
        ai: ai_client,
    } = outside;
    let list_sources = list_sources(tmdb.clone());
    let ffmpeg = transcoder.ffmpeg().to_path_buf();
    let system = system::routes(
        pool.clone(),
        data_dir.to_path_buf(),
        transcoder.root().to_path_buf(),
    );
    let converter = Arc::new(crate::music_convert::Converter::new(
        ffmpeg.clone(),
        transcoder.root(),
    ));
    let artwork_dir = data_dir.join("artwork");
    let artwork = title_artwork::ArtworkState::new(
        pool.clone(),
        tmdb.clone(),
        images.clone(),
        artwork_dir.clone(),
        ffmpeg.clone(),
    );
    let subtitle_cache = data_dir.join("subtitles");
    // One limiter for every password check, so guesses add up wherever they're made.
    let limiter = kept.limiter.clone();
    let router = app_kept(pool.clone(), kept);
    let router = music_routes(router, &pool, data_dir, &artwork_dir, coverart, converter);
    let router =
        lyrics_matching_and_aoide(router, &pool, data_dir, lyrics, translator, &musicbrainz);
    let router = router.merge(watch_home::routes_with_ai(
        pool.clone(),
        artwork.clone(),
        ai_client.clone(),
    ));
    let router = title_routes(router, &pool, artwork_dir, images, artwork, &tmdb, &slskd);
    let router = account_routes(router, &pool, kept, oidc_client, limiter, ai_client)
        .merge(stems::routes(pool.clone()))
        .merge(music_stems::routes(pool.clone(), stems_dir))
        .merge(services::routes(pool.clone(), reload))
        .merge(system)
        .merge(trickplay::routes(
            pool.clone(),
            crate::trickplay::folder(data_dir),
        ))
        .merge(subtitle_search::routes(
            pool.clone(),
            ffprobe.clone(),
            data_dir.join("subtitles"),
        ))
        .merge(offline::routes(
            pool.clone(),
            ffprobe.clone(),
            crate::offline::folder(transcoder.root()),
        ));
    let router = wanting_routes(
        router,
        &pool,
        &tmdb,
        musicbrainz,
        &slskd,
        downloads,
        list_sources,
    );
    watching_routes(
        router,
        pool,
        tmdb,
        ffprobe,
        ffmpeg,
        transcoder,
        subtitle_cache,
    )
    .layer(axum::middleware::map_response(json_errors))
}

/// What import lists need, when TMDB is set up; they can't tell titles apart without it.
pub fn list_sources(tmdb: Option<Arc<tmdb::Client>>) -> Option<crate::lists::Sources> {
    let tmdb = tmdb?;
    let (imdb, rottentomatoes) = match (
        crate::imdb::Client::new(),
        crate::rottentomatoes::Client::new(),
    ) {
        (Ok(imdb), Ok(rottentomatoes)) => (imdb, rottentomatoes),
        (Err(err), _) | (_, Err(err)) => {
            tracing::error!(
                error = format!("{err:#}"),
                "import lists can't reach IMDb or Rotten Tomatoes"
            );
            return None;
        }
    };
    Some(crate::lists::Sources {
        tmdb,
        imdb,
        rottentomatoes,
    })
}

/// axum answers malformed requests itself (bad JSON, a missing content type, a path or query
/// value of the wrong type, unknown routes) in plain text. Rewrite those into the
/// `{"error": …}` shape every other error has, so clients parse one format.
async fn json_errors(response: Response) -> Response {
    use axum::http::{HeaderValue, header};

    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"));
    if !response.status().is_client_error() || is_json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let text = axum::body::to_bytes(body, 16 * 1024)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default();
    let message = if text.is_empty() {
        parts
            .status
            .canonical_reason()
            .unwrap_or("bad request")
            .to_lowercase()
    } else {
        text
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Response::from_parts(
        parts,
        axum::body::Body::from(json!({ "error": message }).to_string()),
    )
}

#[cfg(test)]
mod tests;
