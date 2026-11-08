//! Series with a new episode TMDB hasn't filled in yet. TMDB first lists an episode under a
//! placeholder name ("Episode 6") with no summary, and fills them in later; such a series is
//! fetched again sooner than its usual refresh, and at once when a file for an episode still
//! unnamed comes into the library.

use std::collections::HashSet;

use super::*;

/// A series with an episode aired lately that TMDB hasn't filled in is fetched again after
/// this long.
const REFRESH_UNFILLED: i64 = 6 * 3600;

/// The series a refresh covers.
pub(super) enum Among {
    /// Matched library folders still on disk.
    Library,
    /// Wanted series.
    Wanted,
}

/// `due`, with the series among `among` due sooner because TMDB hasn't filled in an episode
/// yet: one aired within the last 14 days or airing within a day that has no name, TMDB's
/// placeholder name or no summary, fetched over 6 hours ago; or, for the library, a file that
/// came in after the series was fetched for an episode with no name or the placeholder.
pub(super) async fn with_unfilled(
    pool: &SqlitePool,
    among: Among,
    language: &str,
    now: i64,
    mut due: Vec<i64>,
) -> sqlx::Result<Vec<i64>> {
    let query = match among {
        Among::Library => LIBRARY,
        Among::Wanted => WANTED,
    };
    let more: Vec<i64> = sqlx::query_scalar(query)
        .bind(now - REFRESH_UNFILLED)
        .bind(now)
        .bind(placeholder(language))
        .fetch_all(pool)
        .await?;
    let known: HashSet<i64> = due.iter().copied().collect();
    due.extend(more.into_iter().filter(|id| !known.contains(id)));
    Ok(due)
}

/// What TMDB names an episode it has no title for in `language`, before the episode's own
/// number: "Episode 6" in English, "Épisode 6" in French. English is always checked as well,
/// since an episode untitled in another language gets its English name.
pub(super) fn placeholder(language: &str) -> &'static str {
    match tmdb::language::code(language) {
        "fr" => "Épisode ",
        "de" => "Folge ",
        "es" | "it" => "Episodio ",
        "pt" => "Episódio ",
        "nl" => "Aflevering ",
        "pl" => "Odcinek ",
        _ => "Episode ",
    }
}

/// Episode `e` has no name or the placeholder with its own number (`?3` the placeholder in the
/// metadata language), so "Episode Zero" or another episode's number isn't one.
macro_rules! unnamed {
    () => {
        "(e.name IS NULL OR e.name = 'Episode ' || e.episode OR e.name = ?3 || e.episode)"
    };
}

/// Series `t` has an episode aired within the last 14 days or airing within a day (`?2` being
/// now) that TMDB hasn't filled in, and was fetched before `?1`.
macro_rules! unfilled {
    () => {
        concat!(
            "(t.fetched_at < ?1 AND EXISTS (SELECT 1 FROM tmdb_episodes e
                 WHERE e.series_tmdb_id = t.tmdb_id
                   AND e.air_date BETWEEN date(?2, 'unixepoch', '-14 days')
                                      AND date(?2, 'unixepoch', '+1 day')
                   AND (e.overview IS NULL OR ",
            unnamed!(),
            ")))"
        )
    };
}

const LIBRARY: &str = concat!(
    "SELECT DISTINCT t.tmdb_id FROM tmdb_series t JOIN series s ON s.match_tmdb_id = t.tmdb_id
     WHERE s.missing_since IS NULL AND (",
    unfilled!(),
    " OR EXISTS (SELECT 1 FROM media_files f
             JOIN tmdb_episodes e ON e.series_tmdb_id = t.tmdb_id AND e.season = f.season
                 AND e.episode BETWEEN f.episode AND coalesce(f.episode_end, f.episode)
             WHERE f.series_id = s.id AND f.missing_since IS NULL
               AND f.first_seen_at > t.fetched_at AND ",
    unnamed!(),
    "))"
);

const WANTED: &str = concat!(
    "SELECT DISTINCT t.tmdb_id FROM tmdb_series t
     JOIN wanted_series w ON t.tmdb_id = w.tmdb_id OR (w.tmdb_id IS NULL AND t.tvdb_id = w.tvdb_id)
     WHERE ",
    unfilled!()
);

#[cfg(test)]
mod tests;
