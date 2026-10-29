//! The API's routes in groups, merged in order by [`super::server_with`].

use super::*;

/// Music's library, home, pins, likes, mixes, motion covers, playing, plays and transitions.
pub(super) fn music_routes(
    router: Router,
    pool: &SqlitePool,
    data_dir: &std::path::Path,
    artwork_dir: &std::path::Path,
    coverart: Option<Arc<crate::coverart::Client>>,
    converter: Arc<crate::music_convert::Converter>,
) -> Router {
    router
        .merge(music_home::routes(pool.clone(), coverart.is_some()))
        .merge(music_pins::routes(pool.clone(), coverart.is_some()))
        .merge(music_mixes::routes(pool.clone()))
        .merge(music_household::routes(pool.clone()))
        .merge(music_likes::routes(pool.clone()))
        .merge(music::routes(
            pool.clone(),
            artwork_dir.join("music"),
            crate::motion::videos_dir(data_dir),
            coverart,
        ))
        .merge(music_motion::routes(pool.clone()))
        .merge(music_play::routes(pool.clone(), converter))
        .merge(music_plays::routes(pool.clone()))
        .merge(music_transitions::routes(pool.clone()))
}

/// Lyrics and their translation, matching music to MusicBrainz, music profiles, and Aoide's
/// routes.
pub(super) fn lyrics_matching_and_aoide(
    router: Router,
    pool: &SqlitePool,
    data_dir: &std::path::Path,
    lyrics: Option<Arc<crate::lrclib::Client>>,
    translator: Option<Arc<crate::libretranslate::Client>>,
    musicbrainz: &Option<Arc<crate::musicbrainz::Client>>,
) -> Router {
    router
        .merge(music_lyrics::with_translation(
            pool.clone(),
            lyrics,
            translator,
        ))
        .merge(music_match::routes(pool.clone(), musicbrainz.clone()))
        .merge(music_profiles::routes(pool.clone()))
        .merge(aoide::routes(pool.clone(), data_dir.to_path_buf()))
        .merge(aoide_library::routes(pool.clone()))
        .merge(aoide_match::routes(pool.clone()))
        .merge(aoide_sound::routes(pool.clone()))
        .merge(aoide_next::routes(pool.clone()))
}

/// Titles' artwork, media and the Watch home, importing from the arrs, releases, quality
/// profiles, indexers, searching, and songs.
pub(super) fn title_routes(
    router: Router,
    pool: &SqlitePool,
    artwork_dir: PathBuf,
    images: tmdb::Images,
    artwork: title_artwork::ArtworkState,
    tmdb: &Option<Arc<tmdb::Client>>,
    slskd: &Option<Arc<crate::slskd::Client>>,
) -> Router {
    router
        .merge(title_artwork::routes_from(artwork.clone()))
        .merge(watch_home::routes(pool.clone(), artwork.clone()))
        .merge(media::routes_with(
            pool.clone(),
            artwork_dir,
            images,
            artwork,
        ))
        .merge(credits::routes(pool.clone()))
        .merge(import::routes(pool.clone(), tmdb.clone()))
        .merge(releases::routes(pool.clone()))
        .merge(profiles::routes(pool.clone()))
        .merge(indexers::routes(pool.clone()))
        .merge(manage::routes(pool.clone()))
        .merge(search::routes(pool.clone(), tmdb.clone(), slskd.clone()))
        .merge(songs::routes(pool.clone()))
}

/// What a signed-in user keeps and does: progress, languages, favourites, playlists, events,
/// listening together, signing in through a provider, and AI searches; and what each user may
/// see, which admins set.
pub(super) fn account_routes(
    router: Router,
    pool: &SqlitePool,
    kept: &Kept,
    oidc_client: Option<Arc<crate::oidc::Client>>,
    limiter: Arc<crate::auth::LoginLimiter>,
    ai_client: Option<Arc<crate::ai::Client>>,
) -> Router {
    router
        .merge(progress::routes(pool.clone()))
        .merge(languages::routes(pool.clone()))
        .merge(favourites::routes(pool.clone()))
        .merge(access::routes(pool.clone()))
        .merge(playlists::routes(pool.clone()))
        .merge(events::routes(pool.clone()))
        .merge(listen::routes_with(pool.clone(), kept.join_codes.clone()))
        .merge(oidc::routes(pool.clone(), oidc_client, limiter))
        .merge(ai::routes_with(
            pool.clone(),
            ai_client,
            kept.ai_searches.clone(),
        ))
}

/// What's wanted and how it's got: removals, requests, artist and missing searches, RSS,
/// downloads, and import lists.
pub(super) fn wanting_routes(
    router: Router,
    pool: &SqlitePool,
    tmdb: &Option<Arc<tmdb::Client>>,
    musicbrainz: Option<Arc<crate::musicbrainz::Client>>,
    slskd: &Option<Arc<crate::slskd::Client>>,
    downloads: Option<Arc<crate::downloads::Pipeline>>,
    list_sources: Option<crate::lists::Sources>,
) -> Router {
    router
        .merge(wanted_removed::routes(
            pool.clone(),
            tmdb.clone(),
            musicbrainz.clone(),
        ))
        .merge(requests::routes(
            pool.clone(),
            musicbrainz,
            crate::fulfil::Sources {
                tmdb: tmdb.clone(),
                slskd: slskd.clone(),
                pipeline: downloads.clone(),
            },
        ))
        .merge(artist_search::routes(
            pool.clone(),
            crate::artist_search::Sources {
                slskd: slskd.clone(),
                pipeline: downloads.clone(),
            },
        ))
        .merge(missing_searches::routes(
            pool.clone(),
            crate::search_missing::Sources {
                tmdb: tmdb.clone(),
                slskd: slskd.clone(),
                pipeline: downloads.clone(),
                pace: crate::search_missing::PACE,
            },
        ))
        .merge(rss::routes(pool.clone(), tmdb.clone(), downloads.clone()))
        .merge(downloads::routes(pool.clone(), downloads, slskd.clone()))
        .merge(lists::routes(pool.clone(), list_sources))
}

/// The calendar, matching titles, playback, next up, the speed test, diagnostics, the
/// blocklist, admin health, segments, discovery, subtitles, trailers, transcoding, and the web
/// UI.
pub(super) fn watching_routes(
    router: Router,
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    ffprobe: PathBuf,
    ffmpeg: PathBuf,
    transcoder: Arc<crate::transcode::Transcoder>,
    subtitle_cache: PathBuf,
) -> Router {
    router
        .merge(calendar::routes(pool.clone()))
        .merge(matching::routes(pool.clone(), tmdb.clone()))
        .merge(playback::routes(pool.clone(), ffprobe.clone()))
        .merge(next_up::routes(pool.clone()))
        .merge(speedtest::routes(pool.clone()))
        .merge(diagnostics::routes(pool.clone()))
        .merge(blocklist::routes(pool.clone()))
        .merge(admin_health::routes_with(
            pool.clone(),
            Some(transcoder.clone()),
        ))
        .merge(segments::routes(pool.clone()))
        .merge(discover::routes(pool.clone(), tmdb))
        .merge(subtitles::routes(
            pool.clone(),
            ffprobe.clone(),
            ffmpeg,
            subtitle_cache.clone(),
        ))
        .merge(trailers::routes(pool.clone()))
        .merge(web::routes_with(pool.clone()))
        .merge(transcode::routes(pool, ffprobe, transcoder, subtitle_cache))
}
