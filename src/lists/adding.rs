//! Adding a pinned title to the wanted list where the list says, and whether it was added,
//! already wanted or skipped.

use super::*;

pub(super) enum Outcome {
    Added,
    Already,
    Skipped(String),
}

/// Where a list's titles go and how they're looked for.
pub(super) fn placement(list: &List) -> want::Placement<'_> {
    want::Placement {
        monitored: list.monitored,
        quality_profile: Some(&list.quality_profile),
        profile_id: None,
        library_id: list.library_id,
        seasons: want::Seasons::named(&list.monitor_seasons),
        list_id: Some(list.id),
        // A title an admin took off the list stays off, as imports keep it.
        keep_removals: true,
    }
}

pub(super) fn outcome(wanted: want::Wanted) -> Outcome {
    match wanted {
        want::Wanted::Added(_) => Outcome::Added,
        want::Wanted::Already(_) => Outcome::Already,
        want::Wanted::Skipped(why) => Outcome::Skipped(why),
    }
}

pub(super) async fn add_movie(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    list: &List,
    tmdb_id: i64,
) -> anyhow::Result<Outcome> {
    Ok(outcome(
        want::movie(pool, tmdb, tmdb_id, &placement(list)).await?,
    ))
}

pub(super) async fn add_series(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    list: &List,
    tmdb_id: i64,
) -> anyhow::Result<Outcome> {
    Ok(outcome(
        want::series(pool, tmdb, tmdb_id, &placement(list)).await?,
    ))
}
