//! Titles and TMDB: whether TMDB is set up for the video libraries, and the titles it couldn't
//! settle.

use super::*;

/// TMDB not set up while there are movie or series libraries, which then go unmatched. A note,
/// not a problem: like every outside service, it's off until an admin turns it on.
pub(super) async fn tmdb_off(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    if crate::settings::value(pool, "MELETE_TMDB_TOKEN")
        .await?
        .is_some()
    {
        return Ok(());
    }
    let video: i64 =
        sqlx::query_scalar("SELECT count(*) FROM libraries WHERE kind IN ('movies', 'shows')")
            .fetch_one(pool)
            .await?;
    if video > 0 {
        found.add(
            "system",
            Level::Note,
            "tmdb_off",
            "TMDB isn't set up, so movies and series aren't matched: they go without posters, summaries and episode names. Its token goes under Outside services.".into(),
            0,
            None,
        );
    }
    Ok(())
}

/// Titles TMDB couldn't settle and no one has decided by hand, under their service: a series in
/// a library with the anime category is Anime's.
pub(super) async fn titles_to_match(pool: &SqlitePool, found: &mut Found) -> anyhow::Result<()> {
    let open: Vec<(String, i64)> = sqlx::query_as(
        "SELECT area, count(*) FROM (
             SELECT 'movies' AS area FROM movies
             WHERE missing_since IS NULL AND match_status IN ('unmatched', 'ambiguous')
               AND match_method IS NOT 'manual'
             UNION ALL
             SELECT CASE WHEN l.category = 'anime' THEN 'anime' ELSE 'series' END FROM series s
             JOIN libraries l ON l.id = s.library_id
             WHERE s.missing_since IS NULL AND s.match_status IN ('unmatched', 'ambiguous')
               AND s.match_method IS NOT 'manual')
         GROUP BY area ORDER BY area",
    )
    .fetch_all(pool)
    .await?;
    for (area, n) in open {
        let area = AREAS
            .into_iter()
            .find(|a| *a == area)
            .expect("the query names only areas there are");
        found.add(
            area,
            Level::Note,
            "titles_unmatched",
            if n == 1 {
                "1 title needs a match: TMDB couldn't settle it.".to_owned()
            } else {
                format!("{n} titles need a match: TMDB couldn't settle them.")
            },
            n,
            None,
        );
    }
    Ok(())
}
