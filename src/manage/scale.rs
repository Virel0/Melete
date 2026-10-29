//! The managed lists over a library the size of a large household's: every list must answer
//! quickly, and its queries must use the indexes rather than scan once per title.

use std::time::{Duration, Instant};

use super::*;
use crate::db;
use crate::profiles::{self, App};

/// Titles of each kind in the big library.
const MOVIES: i64 = 6000;
const SERIES: i64 = 1200;
/// Episodes of each series, all aired, in two seasons.
const EPISODES: i64 = 40;
const ARTISTS: i64 = 1500;
/// Albums of each artist, and tracks of each album.
const ALBUMS: i64 = 6;
const TRACKS: i64 = 10;

/// How long a list may take here: unoptimised, and beside every other test, which can slow it
/// tenfold. Alone each takes well under a second; the query plans are the closer check.
const AT_MOST: Duration = Duration::from_secs(20);

/// Half of each kind is wanted, a third of the wanted not on disk; downloads come and go.
pub(crate) async fn big_library() -> SqlitePool {
    let pool = db::open_in_memory().await;
    let script = format!(
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'Films', 'movies', '/m', NULL), (2, 'Shows', 'shows', '/s', NULL),
             (3, 'Anime', 'shows', '/a', 'anime'), (4, 'Music', 'music', '/music', NULL);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {MOVIES})
         INSERT INTO tmdb_movies (tmdb_id, title, release_date, year, fetched_at)
             SELECT i, 'Movie ' || i, '2001-01-01', 2001, 0 FROM n;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {MOVIES})
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             SELECT i, 1, 'Movie ' || i, 'Movie ' || i, 1, i FROM n WHERE i % 3 != 0;
         INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             SELECT 1, rel_path || '/Movie.2001.1080p.BluRay.x264.mkv', 1000, 0, id, 1, 1 FROM movies;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {MOVIES})
         INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at)
             SELECT i, 'Movie ' || i, i % 5 != 0, 'manual', i FROM n WHERE i % 2 = 0;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {SERIES})
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at) SELECT i, 'Show ' || i, 0 FROM n;
         WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < {EPISODES} - 1)
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, air_date)
             SELECT s.tmdb_id, 1 + n.i % 2, 1 + n.i / 2, s.tmdb_id * 1000 + n.i, '2002-01-01'
             FROM tmdb_series s, n;
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             SELECT tmdb_id, 2 + tmdb_id % 2, 'Show ' || tmdb_id, 'Show ' || tmdb_id, 1, tmdb_id
             FROM tmdb_series WHERE tmdb_id % 3 != 0;
         INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
                 episode, first_seen_generation, seen_generation)
             SELECT s.library_id, s.rel_path || '/' || e.season || 'x' || e.episode, 500, 0, s.id,
                    e.season, e.episode, 1, 1
             FROM series s JOIN tmdb_episodes e ON e.series_tmdb_id = s.match_tmdb_id
             WHERE e.episode % 7 != 0;
         INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, source,
                 added_at)
             SELECT tmdb_id, tmdb_id, tmdb_id, name, 1,
                    CASE WHEN tmdb_id % 4 = 0 THEN 'anime' ELSE 'standard' END, 'manual', 0
             FROM tmdb_series WHERE tmdb_id % 2 = 0;
         INSERT INTO wanted_seasons (wanted_series_id, season, monitored)
             SELECT id, 1, 1 FROM wanted_series UNION ALL SELECT id, 2, 1 FROM wanted_series;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {ARTISTS})
         INSERT INTO music_artists (id, key, name, mb_artist_id)
             SELECT i, 'artist ' || i, 'Artist ' || i, 'mb-' || i FROM n;
         WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < {ALBUMS} - 1)
         INSERT INTO music_albums (library_id, key, title, artist_id, artist_name, folder,
                 mb_release_group_id, seen_generation)
             SELECT 4, 'k' || a.id || '-' || n.i, 'Album ' || n.i, a.id, a.name,
                    a.name || '/' || n.i, 'rg-' || a.id || '-' || n.i, 1
             FROM music_artists a, n WHERE a.id % 3 != 0;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {TRACKS})
         INSERT INTO music_tracks (library_id, rel_path, size, modified_at, first_seen_generation,
                 seen_generation, album_id, tagged, title)
             SELECT 4, al.folder || '/' || n.i || '.flac', 30, 0, 1, 1, al.id, 1, 'Track ' || n.i
             FROM music_albums al, n;
         INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             SELECT id, mb_artist_id, name, 1, 'all', 'manual', 0 FROM music_artists WHERE id % 2 = 0;
         WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i < {ALBUMS})
         INSERT INTO wanted_albums (wanted_artist_id, mb_release_group_id, title, release_date,
                 monitored, source, added_at)
             SELECT w.id, 'rg-' || w.id || '-' || n.i, 'Album ' || n.i, '2001', 1, 'manual', 0
             FROM wanted_artists w, n;
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 3000)
         INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, added_at,
                 updated_at)
             SELECT printf('%040x', i), 'Release ' || i, 1 + i % 3000,
                    '{{}}', CASE WHEN i % 10 = 0 THEN 'downloading' ELSE 'imported' END, 0, 0
             FROM n;"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(script))
        .execute(&pool)
        .await
        .expect("the big library");
    // Real profiles with their custom formats, so every linked file is judged for its cutoff.
    for app in [App::Radarr, App::Sonarr] {
        profiles::save(&pool, app, &profiles::import::tests::converted(app))
            .await
            .expect("the captured profiles");
    }
    sqlx::raw_sql(
        "UPDATE wanted_movies SET movie_id = tmdb_id, profile_id = (SELECT id FROM quality_profiles
             WHERE source = 'radarr' AND name = '4K Preferred') WHERE tmdb_id % 3 != 0;
         UPDATE wanted_series SET series_id = id, profile_id = (SELECT id FROM quality_profiles
             WHERE source = 'sonarr' AND name = '4K Preferred') WHERE id % 3 != 0;",
    )
    .execute(&pool)
    .await
    .expect("linked and profiled");
    pool
}

#[tokio::test]
async fn every_list_answers_quickly_over_a_big_library() {
    let pool = big_library().await;
    let started = Instant::now();
    let movies = movies(&pool).await.unwrap();
    let took = started.elapsed();
    assert!(took < AT_MOST, "movies took {took:?}");
    assert!(movies.len() as i64 >= MOVIES / 2);

    let started = Instant::now();
    let shows = series(&pool, false).await.unwrap();
    let anime = series(&pool, true).await.unwrap();
    let took = started.elapsed();
    assert!(took < AT_MOST, "series and anime took {took:?}");
    assert_eq!(
        shows.len() + anime.len(),
        SERIES as usize - SERIES as usize / 6
    );

    let started = Instant::now();
    let artists = artists(&pool).await.unwrap();
    let took = started.elapsed();
    assert!(took < AT_MOST, "artists took {took:?}");
    assert!(artists.len() as i64 >= ARTISTS / 2);
}

/// What SQLite plans for a query, one step a line.
async fn plan(pool: &SqlitePool, query: &str) -> String {
    let rows: Vec<(i64, i64, i64, String)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {query}")))
            .fetch_all(pool)
            .await
            .unwrap();
    rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn each_list_query_reads_one_table_through_and_joins_the_rest_by_index() {
    let pool = big_library().await;
    let queries = [
        movies::WANTED.as_str(),
        movies::LIBRARY,
        movies::FILES,
        series::WANTED,
        series::LIBRARY,
        series::MONITORED_EPISODES,
        series::EPISODE_FILES,
        artists::WANTED,
        artists::WANTED_ALBUMS,
        artists::ALBUMS,
    ];
    let downloads =
        ["wanted_movie_id", "wanted_series_id", "wanted_album_id"].map(downloads::query);
    for query in queries
        .into_iter()
        .chain(downloads.iter().map(String::as_str))
    {
        let plan = plan(&pool, query).await;
        let scans = plan.lines().filter(|step| step.starts_with("SCAN")).count();
        assert!(scans <= 1, "{query}\n{plan}");
    }
}

#[tokio::test]
async fn a_titles_downloads_are_found_by_index() {
    let pool = big_library().await;
    for column in ["wanted_movie_id", "wanted_series_id", "wanted_album_id"] {
        let plan = plan(
            &pool,
            &format!("SELECT max(id) FROM downloads WHERE {column} = 1"),
        )
        .await;
        assert!(plan.contains("USING"), "{column}: {plan}");
        assert!(!plan.contains("SCAN downloads"), "{column}: {plan}");
    }
}
