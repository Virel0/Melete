//! The work between scans: series due for a TMDB refresh, and pictures of things not in the
//! library past their most kept.

use std::sync::Arc;

use super::*;

/// How often, between scans, series due for a TMDB refresh are fetched again.
const SERIES_DUE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// Fetches again, every hour between scans, the series due for it (a new episode TMDB hasn't
/// named yet, a wanted series still airing, a stale entry), without scanning or matching, so
/// a scan schedule longer than their 6-hour or daily refresh doesn't hold them back. The scan's
/// own refresh still runs after each scan; the two never overlap.
pub(crate) async fn refresh_series_between_scans(
    pool: sqlx::SqlitePool,
    tmdb: tokio::sync::watch::Receiver<Option<Arc<tmdb::Client>>>,
) {
    loop {
        tokio::time::sleep(SERIES_DUE_EVERY).await;
        let Some(client) = tmdb.borrow().clone() else {
            continue;
        };
        match metadata::refresh_series_due(&pool, &client).await {
            Ok(report) if report.refreshed > 0 || report.errors > 0 => tracing::info!(
                fetched = report.refreshed,
                errors = report.errors,
                "series due refreshed from TMDB between scans"
            ),
            Ok(_) => {}
            Err(err) => tracing::warn!(
                error = format!("{err:#}"),
                "refreshing series due between scans stopped"
            ),
        }
    }
}

/// Forgets pictures of things not in the library as soon as a listing takes a kind over its
/// most kept ([`crate::picture_listings::PRUNE`]), rather than at the next housekeeping.
pub(crate) async fn forget_pictures_when_over(
    pool: sqlx::SqlitePool,
    artwork_dir: std::path::PathBuf,
) {
    loop {
        crate::picture_listings::PRUNE.notified().await;
        forget_pictures(&pool, &artwork_dir, crate::db::unix_now()).await;
    }
}
