//! The steps of the scheduled housekeeping after a scan, each one function, in the order
//! `housekeeping` runs them.

use super::*;

/// Wakes the loops that work on what a scan found: music matching, analysis, cover colours,
/// composers and works, and lyrics.
pub(super) fn wake_after_scanning() {
    // New albums are matched in their own loop: MusicBrainz's pace mustn't hold this one up.
    jobs::MUSIC.notify_one();
    jobs::ANALYSIS.notify_one();
    jobs::COVERS.notify_one();
    jobs::WORKS.notify_one();
    jobs::LYRICS.notify_one();
}

/// Cleans the subtitle cache, fetched subtitles and seek-bar previews of what the scan found
/// gone or changed.
pub(super) async fn clean_caches(
    pool: &sqlx::SqlitePool,
    subtitle_cache: &std::path::Path,
    trickplay_dir: &std::path::Path,
) {
    // After the scan, so it judges against what's on disk now.
    match subtitles::clean_cache(pool, subtitle_cache).await {
        Ok(removed) if removed > 0 => tracing::info!(removed, "cleaned the subtitle cache"),
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't clean the subtitle cache"
        ),
    }
    match opensubtitles::clean_fetched(pool, subtitle_cache).await {
        Ok(removed) if removed > 0 => {
            tracing::info!(removed, "removed outdated fetched subtitles")
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't remove outdated fetched subtitles"
        ),
    }
    match trickplay::clean(pool, trickplay_dir).await {
        Ok(removed) if removed > 0 => {
            tracing::info!(removed, "removed outdated seek-bar previews")
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't remove outdated seek-bar previews"
        ),
    }
}

/// Links wanted titles to folders that are new, then gives seasons to anime numbered from the
/// first episode.
pub(super) async fn link_and_number(pool: &sqlx::SqlitePool) {
    // New folders now scanned and matched: wanted titles that had none get theirs.
    match crate::want::link_folders(pool).await {
        Ok(linked) if linked > 0 => {
            tracing::info!(linked, "linked wanted titles to their new folders")
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't link wanted titles to their folders"
        ),
    }
    // With shows matched and their episode lists in, anime numbered from the first episode
    // can be given seasons.
    match library::absolute::number(pool).await {
        Ok(numbered) if numbered > 0 => {
            tracing::info!(numbered, "numbered episodes from TMDB's lists")
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(error = format!("{err:#}"), "couldn't number episodes"),
    }
}

/// Wakes the loops that need probed and numbered episodes: intros and credits, seek-bar
/// previews, and subtitles for new imports.
pub(super) fn wake_after_probing() {
    // Episodes now probed and numbered, so their lengths and seasons are known: look for
    // intros and credits.
    jobs::SEGMENTS.notify_one();
    jobs::TRICKPLAY.notify_one();
    jobs::IMPORT_SUBTITLES.notify_one();
}

/// Moves files replaced by upgrades to the trash, and empties the trash of old ones.
pub(super) async fn settle_upgrades(pool: &sqlx::SqlitePool) {
    // Upgrades now scanned, probed and numbered: what they replaced goes to the trash.
    match upgrades::retire(pool).await {
        Ok(report) if report.trashed + report.kept > 0 => tracing::info!(
            trashed = report.trashed,
            kept = report.kept,
            "dealt with files replaced by upgrades"
        ),
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(error = format!("{err:#}"), "couldn't retire replaced files")
        }
    }
    match upgrades::purge(pool).await {
        Ok(deleted) if deleted > 0 => tracing::info!(deleted, "emptied the trash of old files"),
        Ok(_) => {}
        Err(err) => tracing::warn!(error = format!("{err:#}"), "couldn't empty the trash"),
    }
}

/// Forgets discovery posters and diagnostics reports past their time.
pub(super) async fn forget_old(pool: &sqlx::SqlitePool, artwork_dir: &std::path::Path) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    match discover_posters::prune(pool, artwork_dir, now).await {
        Ok(forgotten) if forgotten > 0 => {
            tracing::info!(forgotten, "forgot posters of titles not in the library")
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't forget posters of titles not in the library"
        ),
    }
    match diagnostics::prune(pool, now).await {
        Ok(deleted) if deleted > 0 => {
            tracing::info!(deleted, "deleted old diagnostics reports")
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't delete old diagnostics reports"
        ),
    }
}
