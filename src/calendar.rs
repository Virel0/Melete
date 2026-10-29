//! The calendar: episodes of wanted series by air date, and wanted movies by their
//! cinema, digital and physical release dates, from TMDB's cached entries (kept fresh for
//! wanted titles by `metadata::refresh_wanted`). Each says whether it's monitored, already on
//! disk, or downloading.

use serde::Serialize;
use sqlx::SqlitePool;

/// The longest range asked for at once.
pub const MAX_DAYS: i64 = 90;

/// One day's entry: an episode airing, or a movie's release.
#[derive(Debug, Serialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Entry {
    /// YYYY-MM-DD.
    pub date: String,
    /// "episode", or for a movie "cinema", "digital" or "physical".
    pub kind: String,
    /// The wanted movie, for a movie's entry.
    pub wanted_movie_id: Option<i64>,
    /// The wanted series, for an episode's entry.
    pub wanted_series_id: Option<i64>,
    /// The adopted folder, when the title is on disk.
    pub movie_id: Option<i64>,
    /// The adopted series folder, when the series is on disk.
    pub series_id: Option<i64>,
    /// The movie's or series' title.
    pub title: String,
    /// The movie's or series' year.
    pub year: Option<i64>,
    /// The episode's season; `None` for a movie.
    pub season: Option<i64>,
    /// The episode's number in its season; `None` for a movie.
    pub episode: Option<i64>,
    /// The episode's name.
    pub name: Option<String>,
    /// Whether it's monitored: for an episode, its series, season and episode all are.
    pub monitored: bool,
    /// A file for it is in the library.
    pub has_file: bool,
    /// The latest download for it that hasn't failed.
    pub download_id: Option<i64>,
    /// That download's state: "downloading", "completed", "transferring" or "imported".
    pub download_state: Option<String>,
}

/// Everything from `start` to `end` (YYYY-MM-DD, both included), by date.
pub async fn between(pool: &SqlitePool, start: &str, end: &str) -> anyhow::Result<Vec<Entry>> {
    Ok(sqlx::query_as(
        "SELECT * FROM (
             SELECT e.air_date AS date, 'episode' AS kind, NULL AS wanted_movie_id,
                 w.id AS wanted_series_id, NULL AS movie_id, w.series_id, w.title, w.year,
                 e.season, e.episode, e.name,
                 (w.monitored = 1 AND coalesce(ws.monitored, 0) = 1
                     AND coalesce(o.monitored, 1) = 1) AS monitored,
                 EXISTS (SELECT 1 FROM media_files f JOIN series s ON s.id = f.series_id
                         WHERE f.missing_since IS NULL AND f.season = e.season
                           AND f.episode <= e.episode AND coalesce(f.episode_end, f.episode) >= e.episode
                           AND (s.id = w.series_id OR s.match_tmdb_id = t.tmdb_id)) AS has_file,
                 d.id AS download_id, d.state AS download_state
             FROM wanted_series w
             JOIN tmdb_series t
                 ON t.tmdb_id = w.tmdb_id OR (w.tmdb_id IS NULL AND t.tvdb_id = w.tvdb_id)
             JOIN tmdb_episodes e ON e.series_tmdb_id = t.tmdb_id
             LEFT JOIN wanted_seasons ws ON ws.wanted_series_id = w.id AND ws.season = e.season
             LEFT JOIN wanted_episode_overrides o
                 ON o.wanted_series_id = w.id AND o.season = e.season AND o.episode = e.episode
             LEFT JOIN downloads d ON d.id = (
                 SELECT id FROM downloads
                 WHERE wanted_series_id = w.id AND season = e.season
                   AND (episode IS NULL OR episode = e.episode) AND state <> 'failed'
                 ORDER BY id DESC LIMIT 1)
             WHERE e.air_date BETWEEN ?1 AND ?2
           UNION ALL
             SELECT r.date, r.kind, w.id, NULL, w.movie_id, NULL, w.title, w.year, NULL, NULL, NULL,
                 w.monitored = 1,
                 EXISTS (SELECT 1 FROM media_files f JOIN movies m ON m.id = f.movie_id
                         WHERE f.missing_since IS NULL
                           AND (m.id = w.movie_id OR m.match_tmdb_id = w.tmdb_id)),
                 d.id, d.state
             FROM wanted_movies w
             JOIN (SELECT tmdb_id, release_date AS date, 'cinema' AS kind FROM tmdb_movies
                   UNION ALL SELECT tmdb_id, digital_release, 'digital' FROM tmdb_movies
                   UNION ALL SELECT tmdb_id, physical_release, 'physical' FROM tmdb_movies) r
                 ON r.tmdb_id = w.tmdb_id
             LEFT JOIN downloads d ON d.id = (
                 SELECT id FROM downloads WHERE wanted_movie_id = w.id AND state <> 'failed'
                 ORDER BY id DESC LIMIT 1)
             WHERE r.date BETWEEN ?1 AND ?2
         )
         ORDER BY date, title, season, episode, kind",
    )
    .bind(start)
    .bind(end)
    .fetch_all(pool)
    .await?)
}

/// Checks a range: both real dates, in order, at most [`MAX_DAYS`] apart. Without them, from
/// yesterday to two weeks on.
pub async fn range(
    pool: &SqlitePool,
    start: Option<&str>,
    end: Option<&str>,
) -> anyhow::Result<Result<(String, String), &'static str>> {
    // SQLite's date() gives back a real date unchanged, and anything else differently.
    let (start, end, valid, days): (String, String, bool, f64) = sqlx::query_as(
        "SELECT s, e, date(s) IS s AND date(e) IS e, julianday(e) - julianday(s)
         FROM (SELECT coalesce(?1, date('now', '-1 day')) AS s,
                      coalesce(?2, date(coalesce(?1, date('now', '-1 day')), '+14 days')) AS e)",
    )
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await?;
    if !valid {
        return Ok(Err("start and end are dates, as YYYY-MM-DD"));
    }
    if !(0.0..=MAX_DAYS as f64).contains(&days) {
        return Ok(Err("end is on or after start, at most 90 days on"));
    }
    Ok(Ok((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup() -> SqlitePool {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s'), (2, 'movies', 'movies', '/m');
             -- Frieren: known to Sonarr by TVDB id only, its folder matched to TMDB.
             INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, status, fetched_at)
             VALUES (209867, 424536, 'Frieren', 'Returning Series', 0);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, air_date) VALUES
                 (209867, 2, 1, 1, 'One', '2026-11-01'),
                 (209867, 2, 2, 2, 'Two', '2026-11-08'),
                 (209867, 2, 3, 3, 'Three', '2026-11-15'),
                 (209867, 3, 1, 4, 'Next season', '2026-11-20'),
                 (209867, 2, 4, 5, 'Too late', '2026-12-31');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             VALUES (1, 1, 'Frieren', 'Frieren', 1, NULL);
             UPDATE series SET match_tmdb_id = 209867 WHERE id = 1;
             INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season, episode,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Frieren/Frieren.S02E01.mkv', 1, 0, 1, 2, 1, 1, 1);
             INSERT INTO wanted_series (id, tvdb_id, title, year, monitored, series_type, source, added_at)
             VALUES (7, 424536, 'Frieren', 2023, 1, 'anime', 'sonarr', 0);
             INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (7, 2, 1), (7, 3, 0);
             INSERT INTO wanted_episode_overrides (wanted_series_id, season, episode, monitored)
             VALUES (7, 2, 3, 0);
             INSERT INTO downloads (infohash, title, wanted_series_id, season, episode, decision, state,
                 added_at, updated_at)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Frieren.S02E02', 7, 2, 2, '{}',
                 'downloading', 0, 0);
             -- A movie out in cinemas before the range, digitally within it.
             INSERT INTO tmdb_movies (tmdb_id, title, release_date, digital_release, physical_release, fetched_at)
             VALUES (1, 'Some Film', '2026-09-01', '2026-11-03', '2027-01-10', 0);
             INSERT INTO wanted_movies (id, tmdb_id, title, year, monitored, source, added_at)
             VALUES (3, 1, 'Some Film', 2026, 1, 'radarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// Date, kind, episode, monitored, on disk, download state.
    type Seen<'a> = (&'a str, &'a str, Option<i64>, bool, bool, Option<&'a str>);

    #[tokio::test]
    async fn lists_episodes_and_releases_with_where_they_are() {
        let pool = setup().await;
        let entries = between(&pool, "2026-11-01", "2026-11-30").await.unwrap();
        let summary: Vec<Seen> = entries
            .iter()
            .map(|e| {
                (
                    e.date.as_str(),
                    e.kind.as_str(),
                    e.episode,
                    e.monitored,
                    e.has_file,
                    e.download_state.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("2026-11-01", "episode", Some(1), true, true, None),
                ("2026-11-03", "digital", None, true, false, None),
                (
                    "2026-11-08",
                    "episode",
                    Some(2),
                    true,
                    false,
                    Some("downloading")
                ),
                ("2026-11-15", "episode", Some(3), false, false, None),
                ("2026-11-20", "episode", Some(1), false, false, None),
            ]
        );
        let first = &entries[0];
        assert_eq!(
            (first.wanted_series_id, first.series_id, first.season),
            (Some(7), None, Some(2)),
            "no adopted folder recorded on the wanted entry, but the file is found by TMDB id"
        );
        assert_eq!(first.name.as_deref(), Some("One"));
        assert_eq!(entries[1].wanted_movie_id, Some(3));
        assert_eq!(entries[1].title, "Some Film");
    }

    #[tokio::test]
    async fn ranges_are_checked() {
        let pool = setup().await;
        let ok = |s: &str, e: &str| (s.to_owned(), e.to_owned());
        assert_eq!(
            range(&pool, Some("2026-11-01"), Some("2026-11-30"))
                .await
                .unwrap(),
            Ok(ok("2026-11-01", "2026-11-30"))
        );
        assert_eq!(
            range(&pool, Some("2026-11-01"), None).await.unwrap(),
            Ok(ok("2026-11-01", "2026-11-15")),
            "two weeks on"
        );
        let (start, end) = range(&pool, None, None).await.unwrap().unwrap();
        let days: f64 = sqlx::query_scalar("SELECT julianday(?) - julianday(?)")
            .bind(&end)
            .bind(&start)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(days, 14.0);
        for (start, end) in [
            ("2026-02-30", "2026-03-01"),
            ("26-11-01", "2026-11-02"),
            ("2026-11-01", "tomorrow"),
            ("2026-11-02", "2026-11-01"),
            ("2026-01-01", "2026-06-01"),
        ] {
            assert!(
                range(&pool, Some(start), Some(end)).await.unwrap().is_err(),
                "{start}..{end}"
            );
        }
    }
}
