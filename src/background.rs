//! The work Melete does in the background beside the API: the scheduled housekeeping (scans,
//! matching, probing, the trash) and the loops woken after it (music matching and analysis,
//! intros and credits, composers and works, lyrics, cover colours).

use std::sync::Arc;

use crate::{
    classical, cover_colours, diagnostics, discover_posters, jobs, library, lyrics, metadata,
    music_match, music_spatial, musicbrainz, opensubtitles, probe, search_keys, segments, sound,
    subtitles, tmdb, track_artists, trickplay, upgrades,
};

mod steps;

use steps::*;

/// Scans, subtitle cache cleaning, TMDB matching and probing: at startup, every few hours, and
/// when asked.
pub(crate) async fn housekeeping(
    pool: sqlx::SqlitePool,
    libraries: Vec<library::Library>,
    tmdb: tokio::sync::watch::Receiver<Option<Arc<tmdb::Client>>>,
    ffprobe: std::path::PathBuf,
    subtitle_cache: std::path::PathBuf,
    artwork_dir: std::path::PathBuf,
    trickplay_dir: std::path::PathBuf,
) {
    jobs::repeat(jobs::TICK, &jobs::RESCAN, || async {
        let schedule = scan_schedule(&pool).await;
        let Some(scope) = jobs::scans::take(schedule, crate::db::unix_now()) else {
            return;
        };
        // A run of the library task is a scan and what follows it, not each look at the clock.
        crate::tasks::started("library");
        match scope {
            jobs::scans::Scope::All => scan_all(&pool, libraries.clone()).await,
            jobs::scans::Scope::Some(asked) => scan_some(&pool, &libraries, asked).await,
        }
        wake_after_scanning();
        clean_caches(&pool, &subtitle_cache, &trickplay_dir).await;
        // TMDB is network-bound and probing is disk-bound, so they run side by side.
        tokio::join!(
            match_metadata(&pool, tmdb.borrow().clone()),
            probe_files(&pool, &ffprobe),
            probe_music(&pool, &ffprobe)
        );
        link_and_number(&pool).await;
        wake_after_probing();
        settle_upgrades(&pool).await;
        forget_old(&pool, &artwork_dir).await;
        let next = jobs::scans::next_whole(schedule, crate::db::unix_now());
        crate::tasks::finished("library", None, next);
    })
    .await;
}

/// Matches new music albums to MusicBrainz, woken after each scan.
pub(crate) async fn match_music(pool: sqlx::SqlitePool, mb: Arc<musicbrainz::Client>) {
    jobs::repeat_task("music-matching", jobs::EVERY, &jobs::MUSIC, || async {
        match music_match::match_pending(&pool, &mb).await {
            Ok(report) if report == music_match::Report::default() => {}
            Ok(report) => tracing::info!(
                matched = report.matched,
                unmatched = report.unmatched,
                ambiguous = report.ambiguous,
                errors = report.errors,
                "music matching finished"
            ),
            Err(err) => tracing::error!(error = format!("{err:#}"), "music matching stopped"),
        }
    })
    .await;
}

/// Measures music tracks' audio for Aoide, woken after each scan.
pub(crate) async fn analyse_music(pool: sqlx::SqlitePool, tools: sound::Tools) {
    jobs::repeat_task("music-analysis", jobs::EVERY, &jobs::ANALYSIS, || async {
        match sound::measure_pending(&pool, &tools, &sound::REQUESTS).await {
            Ok(report) if report == sound::Report::default() => {}
            Ok(report) => tracing::info!(
                measured = report.measured,
                failed = report.failed,
                retry_later = report.retry_later,
                "measuring music finished"
            ),
            Err(err) => tracing::error!(error = format!("{err:#}"), "measuring music stopped"),
        }
    })
    .await;
}

/// Looks for intros and credits in episodes, and for credits in movies and episodes
/// alone, woken after each scan and probe.
pub(crate) async fn find_segments(pool: sqlx::SqlitePool, ffmpeg: std::path::PathBuf) {
    jobs::repeat_task("segments", jobs::EVERY, &jobs::SEGMENTS, || async {
        match segments::run_both(&pool, &ffmpeg).await {
            Ok((report, ends))
                if report == segments::Report::default()
                    && ends == segments::ending::Found::default() => {}
            Ok((report, ends)) => tracing::info!(
                scanned = report.scanned,
                intros = report.intros,
                credits = report.credits,
                failed = report.failed,
                ends_scanned = ends.scanned,
                ends_credits = ends.credits,
                ends_failed = ends.failed,
                "looking for intros and credits finished"
            ),
            Err(err) => tracing::error!(
                error = format!("{err:#}"),
                "looking for intros and credits stopped"
            ),
        }
    })
    .await;
}

/// Makes seek-bar previews while that's turned on, woken after each scan and probe.
pub(crate) async fn make_trickplay(
    pool: sqlx::SqlitePool,
    ffmpeg: std::path::PathBuf,
    dir: std::path::PathBuf,
) {
    jobs::repeat_task("trickplay", jobs::EVERY, &jobs::TRICKPLAY, || async {
        match trickplay::make_pending(&pool, &ffmpeg, &dir).await {
            Ok(report) if report == trickplay::Report::default() => {}
            Ok(report) => tracing::info!(
                made = report.made,
                failed = report.failed,
                "making seek-bar previews finished"
            ),
            Err(err) => tracing::error!(
                error = format!("{err:#}"),
                "making seek-bar previews stopped"
            ),
        }
    })
    .await;
}

/// Fetches subtitles new imports lack from OpenSubtitles while languages are set for them,
/// woken after each scan and probe, so what's inside each file is known.
pub(crate) async fn fetch_import_subtitles(pool: sqlx::SqlitePool, subtitles: std::path::PathBuf) {
    jobs::repeat_task(
        "import-subtitles",
        jobs::EVERY,
        &jobs::IMPORT_SUBTITLES,
        || async {
            match opensubtitles::fetch_for_imports(&pool, &subtitles).await {
                Ok(report) if report == opensubtitles::Report::default() => {}
                Ok(report) => tracing::info!(
                    fetched = report.fetched,
                    none_made = report.none_made,
                    "fetching subtitles for new imports finished"
                ),
                Err(err) => tracing::error!(
                    error = format!("{err:#}"),
                    "fetching subtitles for new imports stopped"
                ),
            }
        },
    )
    .await;
}

/// Works out composers, works and their parts, and each track's artists, woken after each scan
/// and match: both come from the tags a scan reads and the releases matching links.
pub(crate) async fn work_out_works(pool: sqlx::SqlitePool) {
    jobs::repeat_task("works", jobs::EVERY, &jobs::WORKS, || async {
        match classical::refresh(&pool).await {
            Ok((composers, works)) => {
                tracing::debug!(composers, works, "worked out composers and works")
            }
            Err(err) => tracing::error!(
                error = format!("{err:#}"),
                "working out composers and works stopped"
            ),
        }
        match track_artists::refresh(&pool).await {
            Ok(tracks) => tracing::debug!(tracks, "worked out tracks' artists"),
            Err(err) => tracing::error!(
                error = format!("{err:#}"),
                "working out tracks' artists stopped"
            ),
        }
    })
    .await;
}

/// Reads lyrics beside tracks and in their tags for searching, woken after each scan.
pub(crate) async fn read_lyrics(pool: sqlx::SqlitePool) {
    jobs::repeat_task("lyrics", jobs::EVERY, &jobs::LYRICS, || async {
        match lyrics::read_library(&pool).await {
            Ok(report) if report == lyrics::Report::default() => {}
            Ok(report) => tracing::info!(
                found = report.found,
                gone = report.gone,
                "reading lyrics finished"
            ),
            Err(err) => tracing::error!(error = format!("{err:#}"), "reading lyrics stopped"),
        }
    })
    .await;
}

/// Works out album covers' colours for the apps, woken after each scan.
pub(crate) async fn colour_covers(
    pool: sqlx::SqlitePool,
    ffmpeg: std::path::PathBuf,
    covers: std::path::PathBuf,
) {
    jobs::repeat_task("cover-colours", jobs::EVERY, &jobs::COVERS, || async {
        match cover_colours::measure_pending(&pool, &ffmpeg, &covers).await {
            Ok(report) if report == cover_colours::Report::default() => {}
            Ok(report) => tracing::info!(
                measured = report.measured,
                failed = report.failed,
                "colouring covers finished"
            ),
            Err(err) => tracing::error!(error = format!("{err:#}"), "colouring covers stopped"),
        }
    })
    .await;
}

pub(crate) async fn probe_files(pool: &sqlx::SqlitePool, ffprobe: &std::path::Path) {
    match probe::probe_pending(pool, ffprobe).await {
        Ok(report) => tracing::info!(
            probed = report.probed,
            failed = report.failed,
            retry_later = report.retry_later,
            "probing finished"
        ),
        Err(err) => tracing::error!(error = format!("{err:#}"), "probing stopped"),
    }
}

/// Asks ffprobe about music files the tag reader couldn't name: Dolby Atmos and the like.
pub(crate) async fn probe_music(pool: &sqlx::SqlitePool, ffprobe: &std::path::Path) {
    match music_spatial::probe_pending(pool, ffprobe).await {
        Ok(report) if report == music_spatial::Report::default() => {}
        Ok(report) => tracing::info!(
            probed = report.probed,
            failed = report.failed,
            retry_later = report.retry_later,
            "probing music finished"
        ),
        Err(err) => tracing::error!(error = format!("{err:#}"), "probing music stopped"),
    }
}

pub(crate) async fn match_metadata(pool: &sqlx::SqlitePool, tmdb: Option<Arc<tmdb::Client>>) {
    let Some(tmdb) = tmdb else {
        // Nothing to fetch, but a country set in .env since the last run still counts.
        if let Err(err) = crate::ratings::reread(pool).await {
            tracing::warn!(error = %err, "couldn't judge content ratings again");
        }
        return;
    };
    match metadata::refresh(pool, &tmdb).await {
        Ok(report) => tracing::info!(
            matched = report.matched,
            unmatched = report.unmatched,
            ambiguous = report.ambiguous,
            refreshed = report.refreshed,
            errors = report.errors,
            "metadata refresh finished"
        ),
        Err(err) => tracing::error!(error = format!("{err:#}"), "metadata refresh stopped"),
    }
    match metadata::refresh_wanted(pool, &tmdb).await {
        Ok(report) => tracing::info!(
            fetched = report.refreshed,
            errors = report.errors,
            "wanted titles' metadata refreshed"
        ),
        Err(err) => tracing::error!(
            error = format!("{err:#}"),
            "wanted titles' metadata refresh stopped"
        ),
    }
    match crate::ratings::refresh(pool, &tmdb).await {
        Ok(report) => tracing::info!(
            fetched = report.fetched,
            errors = report.errors,
            "content ratings refreshed"
        ),
        Err(err) => tracing::error!(
            error = format!("{err:#}"),
            "content ratings refresh stopped"
        ),
    }
    match metadata::refresh_credits(pool, &tmdb).await {
        Ok(fetched) if fetched > 0 => tracing::info!(fetched, "cast and crew fetched"),
        Ok(_) => {}
        Err(err) => tracing::warn!(error = format!("{err:#}"), "fetching cast and crew stopped"),
    }
    match metadata::refresh_trailers(pool, &tmdb).await {
        Ok(fetched) if fetched > 0 => tracing::info!(fetched, "titles' trailers fetched"),
        Ok(_) => {}
        Err(err) => tracing::warn!(error = format!("{err:#}"), "fetching trailers stopped"),
    }
}

/// The scan schedule as set now, the default when it's unset or can't be read.
async fn scan_schedule(pool: &sqlx::SqlitePool) -> jobs::scans::Schedule {
    let set = crate::settings::value(pool, "MELETE_SCAN_SCHEDULE").await;
    match set.ok().flatten().map(|v| jobs::scans::Schedule::parse(&v)) {
        Some(Ok(schedule)) => schedule,
        Some(Err(why)) => {
            tracing::warn!(
                why,
                "the scan schedule can't be read; scanning every 6 hours"
            );
            jobs::scans::Schedule::DEFAULT
        }
        None => jobs::scans::Schedule::DEFAULT,
    }
}

/// Scans the libraries and title folders asked for, each whole or only its folder.
async fn scan_some(
    pool: &sqlx::SqlitePool,
    libraries: &[library::Library],
    asked: Vec<(i64, Option<String>)>,
) {
    for (id, folder) in asked {
        let Some(library) = libraries.iter().find(|l| l.id == id) else {
            continue;
        };
        let scanned = match &folder {
            Some(folder) => library::scan_folder(pool, library, folder).await,
            None => library::scan(pool, library).await,
        };
        log_scan(library, folder.as_deref(), scanned);
    }
    fill_search_keys(pool).await;
}

fn log_scan(
    library: &library::Library,
    folder: Option<&str>,
    scanned: anyhow::Result<library::ScanReport>,
) {
    match scanned {
        Ok(report) => tracing::info!(
            library = library.name,
            folder,
            files = report.files,
            added = report.added,
            newly_missing = report.newly_missing,
            unparsed = report.unparsed,
            skipped = report.skipped,
            "scan finished"
        ),
        Err(err) => tracing::error!(
            library = library.name,
            folder,
            error = format!("{err:#}"),
            "scan failed; nothing was changed"
        ),
    }
}

/// Scans each library in turn. A failed library is logged and left unchanged.
pub(crate) async fn scan_all(pool: &sqlx::SqlitePool, libraries: Vec<library::Library>) {
    for library in libraries {
        let scanned = library::scan(pool, &library).await;
        log_scan(&library, None, scanned);
    }
    fill_search_keys(pool).await;
}

async fn fill_search_keys(pool: &sqlx::SqlitePool) {
    // New and renamed titles get their search and sort keys.
    match search_keys::fill(pool).await {
        Ok(filled) if filled > 0 => tracing::info!(filled, "worked out titles' search keys"),
        Ok(_) => {}
        Err(err) => tracing::warn!(error = %err, "couldn't work out titles' search keys"),
    }
}
