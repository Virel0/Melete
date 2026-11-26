//! Libraries whose disk is nearly full, each measured off the async threads within the library
//! check's time limit.

use super::*;

/// Libraries whose disk is nearly full: less than [`crate::disk::NEARLY_FULL_PERCENT`] free. A
/// library that can't be measured is left to the scan, which says so, and one not responding to
/// its own finding (`library_unreachable`); each is measured off the async threads, in time.
pub(super) async fn disks(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let libraries: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, path FROM libraries ORDER BY name")
            .fetch_all(pool)
            .await?;
    for (id, name, path) in libraries {
        let path = std::path::PathBuf::from(path);
        let at = path.clone();
        let measured =
            crate::library::health::within(&path, crate::library::health::LIMIT, move || {
                crate::disk::space(&at)
            });
        let Ok(Ok(space)) = measured.await else {
            continue;
        };
        if let Some(message) = nearly_full(&name, space) {
            found.add(
                "system",
                Level::Problem,
                "disk_nearly_full",
                message,
                1,
                Some(id),
            );
        }
    }
    Ok(())
}

/// What's said of a library whose disk is nearly full; None when it isn't.
pub(super) fn nearly_full(name: &str, space: crate::disk::Space) -> Option<String> {
    space.nearly_full().then(|| {
        format!(
            "The disk the {name} library is on is nearly full: {} GB free ({}%).",
            space.free / 1_000_000_000,
            space.free * 100 / space.total
        )
    })
}
