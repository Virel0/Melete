//! Anime episodes without styled (ASS/SSA) subtitles: a note for each series in Anime, as their
//! subtitles show as plain words or not at all, and a release that has them is worth a search.

use sqlx::SqlitePool;

use super::{Found, Level, plural};

/// A note for each anime series with episodes that have no styled subtitles, in the file or
/// beside it.
pub(super) async fn anime_unstyled_subtitles(
    pool: &SqlitePool,
    found: &mut Found,
) -> anyhow::Result<()> {
    let episodes = crate::subtitles::unstyled(pool, None).await?;
    for series in episodes.chunk_by(|a, b| a.series_id == b.series_id) {
        let n = i64::try_from(series.len())?;
        let (one, every) = (&series[0], n == 1);
        found.add(
            "anime",
            Level::Note,
            "anime_unstyled_subtitles",
            format!(
                "{}: {} no styled subtitles, so {} plain words or none. Search for a release \
                 that has them.",
                one.series,
                plural(n, "episode has", "episodes have"),
                if every { "it shows" } else { "they show" },
            ),
            n,
            Some(one.series_id),
        );
    }
    Ok(())
}
