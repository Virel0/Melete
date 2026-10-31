use super::*;
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};
use crate::tmdb::testing::{FakeTmdb, Reply};
use axum::http::StatusCode;
use serde_json::json;

fn candidate(tmdb_id: i64, title: &str, year: Option<i64>) -> Candidate {
    Candidate {
        tmdb_id,
        title: title.into(),
        original_title: None,
        year,
    }
}

#[test]
fn exact_title_and_year_wins() {
    let results = [
        candidate(1, "Dune", Some(2021)),
        candidate(2, "Dune", Some(1984)),
    ];
    assert_eq!(choose("Dune", Some(2021), &results), Choice::One(1));
}

#[test]
fn year_may_be_off_by_one() {
    assert_eq!(
        choose("Dune", Some(2020), &[candidate(1, "Dune", Some(2021))]),
        Choice::One(1)
    );
    assert_eq!(
        choose("Dune", Some(2019), &[candidate(1, "Dune", Some(2021))]),
        Choice::Nothing
    );
}

#[test]
fn exact_year_breaks_an_off_by_one_tie() {
    let results = [
        candidate(1, "Film", Some(2010)),
        candidate(2, "Film", Some(2011)),
    ];
    assert_eq!(choose("Film", Some(2011), &results), Choice::One(2));
}

#[test]
fn no_year_and_several_titles_is_ambiguous() {
    let results = [
        candidate(1, "Dune", Some(2021)),
        candidate(2, "Dune", Some(1984)),
    ];
    assert_eq!(
        choose("Dune", None, &results),
        Choice::Ambiguous(results.to_vec())
    );
}

#[test]
fn different_title_is_not_a_match() {
    let results = [candidate(1, "Dune: Part Two", Some(2024))];
    assert_eq!(choose("Dune", Some(2024), &results), Choice::Nothing);
}

#[test]
fn titles_compare_without_punctuation() {
    let results = [
        candidate(1, "Schindler's List", Some(1993)),
        candidate(2, "Star Wars: Episode IV", Some(1977)),
        candidate(3, "Fast & Furious", Some(2009)),
    ];
    assert_eq!(
        choose("Schindlers List", Some(1993), &results),
        Choice::One(1)
    );
    assert_eq!(
        choose("Star Wars Episode IV", Some(1977), &results),
        Choice::One(2)
    );
    assert_eq!(
        choose("Fast and Furious", Some(2009), &results),
        Choice::One(3)
    );
}

#[test]
fn original_title_counts() {
    let mut amelie = candidate(1, "Amélie", Some(2001));
    amelie.original_title = Some("Le Fabuleux Destin d'Amélie Poulain".into());
    assert_eq!(
        choose("Le Fabuleux Destin d'Amélie Poulain", Some(2001), &[amelie]),
        Choice::One(1)
    );
}

async fn library(pool: &SqlitePool, kind: LibraryKind) -> i64 {
    let config = LibraryConfig {
        name: kind.as_str().into(),
        kind,
        path: "/media/x".into(),
    };
    sync_config(pool, &[config]).await.unwrap()[0].id
}

async fn insert_title(
    pool: &SqlitePool,
    table: &str,
    library_id: i64,
    rel_path: &str,
    tags: (Option<i64>, Option<i64>),
) {
    let parsed = crate::library::parse::parse_title(rel_path);
    let sql = format!(
            "INSERT INTO {table} (library_id, rel_path, parsed_title, parsed_year, tmdb_id, seen_generation{})
             VALUES (?, ?, ?, ?, ?, 1{})",
            if table == "series" { ", tvdb_id" } else { "" },
            if table == "series" { ", ?" } else { "" },
        );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(library_id)
        .bind(rel_path)
        .bind(parsed.title)
        .bind(parsed.year)
        .bind(tags.0);
    if table == "series" {
        query = query.bind(tags.1);
    }
    query.execute(pool).await.unwrap();
}

async fn statuses(
    pool: &SqlitePool,
    table: &str,
) -> Vec<(String, Option<String>, Option<i64>, Option<String>)> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT rel_path, match_status, match_tmdb_id, match_method FROM {table} ORDER BY rel_path"
    )))
    .fetch_all(pool)
    .await
    .unwrap()
}

fn movie(id: i64, title: &str, date: &str) -> serde_json::Value {
    json!({ "id": id, "title": title, "original_title": title, "release_date": date,
                "overview": "", "runtime": 120, "poster_path": "/p.jpg", "backdrop_path": null,
                "imdb_id": "", "genres": [{ "id": 1, "name": "Science Fiction" }] })
}

#[tokio::test]
async fn matches_movies_by_tag_search_and_leaves_doubt_unmatched() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Movies).await;
    for rel in [
        "The Matrix (1999) [tmdbid-603]",
        "Dune (2021)",
        "Dune",
        "Nothing Here (2001)",
        "Flaky (2005)",
    ] {
        insert_title(
            &pool,
            "movies",
            lib,
            rel,
            (crate::library::parse::parse_title(rel).tmdb_id, None),
        )
        .await;
    }
    let dune_results = Reply::ok(json!({ "results": [
        { "id": 438631, "title": "Dune", "original_title": "Dune", "release_date": "2021-09-15" },
        { "id": 841, "title": "Dune", "original_title": "Dune", "release_date": "1984-12-14" }
    ]}));
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603",
        vec![Reply::ok(movie(603, "The Matrix", "1999-03-31"))],
    )
    .reply(
        "/movie/438631",
        vec![Reply::ok(movie(438631, "Dune", "2021-09-15"))],
    )
    .reply(
        "/search/movie?include_adult=false&query=Dune&year=2021",
        vec![dune_results.clone()],
    )
    .reply(
        "/search/movie?include_adult=false&query=Dune",
        vec![dune_results],
    )
    .reply(
        "/search/movie?include_adult=false&query=Nothing Here&year=2001",
        vec![Reply::ok(json!({ "results": [] }))],
    )
    .reply(
        "/search/movie?include_adult=false&query=Nothing Here",
        vec![Reply::ok(json!({ "results": [] }))],
    )
    .reply(
        "/search/movie?include_adult=false&query=Flaky&year=2005",
        vec![Reply::status(StatusCode::BAD_GATEWAY)],
    );
    let tmdb = fake.start().await;

    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        report,
        RefreshReport {
            matched: 2,
            unmatched: 1,
            ambiguous: 1,
            refreshed: 0,
            errors: 1
        }
    );
    assert_eq!(
        statuses(&pool, "movies").await,
        vec![
            ("Dune".into(), Some("ambiguous".into()), None, None),
            (
                "Dune (2021)".into(),
                Some("matched".into()),
                Some(438631),
                Some("search".into())
            ),
            ("Flaky (2005)".into(), None, None, None),
            (
                "Nothing Here (2001)".into(),
                Some("unmatched".into()),
                None,
                None
            ),
            (
                "The Matrix (1999) [tmdbid-603]".into(),
                Some("matched".into()),
                Some(603),
                Some("tmdb_tag".into())
            ),
        ]
    );
    let candidates: String =
        sqlx::query_scalar("SELECT match_candidates FROM movies WHERE rel_path = 'Dune'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&candidates).unwrap(),
        json!([{ "tmdb_id": 438631, "title": "Dune", "year": 2021 },
                   { "tmdb_id": 841, "title": "Dune", "year": 1984 }])
    );
    let cached: (String, Option<i64>, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT title, year, genres, imdb_id, overview FROM tmdb_movies WHERE tmdb_id = 603",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        cached,
        (
            "The Matrix".into(),
            Some(1999),
            r#"["Science Fiction"]"#.into(),
            None,
            None
        )
    );

    // A second run only retries the title that hit an error.
    fake.log.lock().unwrap().clear();
    let again = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        again,
        RefreshReport {
            errors: 1,
            ..RefreshReport::default()
        }
    );
    assert!(
        fake.log
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.contains("Flaky"))
    );
}

#[tokio::test]
async fn matches_series_and_stores_episodes() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Shows).await;
    insert_title(&pool, "series", lib, "Severance (2022)", (None, None)).await;
    insert_title(
        &pool,
        "series",
        lib,
        "Breaking Bad {tvdb-81189}",
        (None, Some(81189)),
    )
    .await;

    let fake = FakeTmdb::default();
    fake.reply(
            "/search/tv?first_air_date_year=2022&include_adult=false&query=Severance",
            vec![Reply::ok(json!({ "results": [
                { "id": 95396, "name": "Severance", "original_name": "Severance", "first_air_date": "2022-02-17" }
            ]}))],
        )
        .reply(
            "/tv/95396?append_to_response=external_ids",
            vec![Reply::ok(json!({
                "id": 95396, "name": "Severance", "first_air_date": "2022-02-17", "status": "Returning Series",
                "genres": [{ "id": 18, "name": "Drama" }],
                "seasons": [{ "season_number": 1 }, { "season_number": 2 }],
                "external_ids": { "imdb_id": "tt11280740", "tvdb_id": 371980 }
            }))],
        )
        .reply(
            "/tv/95396?append_to_response=season/1,season/2",
            vec![Reply::ok(json!({ "id": 95396, "name": "Severance",
                "season/1": { "episodes": [
                    { "id": 1, "season_number": 1, "episode_number": 1, "name": "Good News About Hell", "air_date": "2022-02-18", "runtime": 57 },
                    { "id": 2, "season_number": 1, "episode_number": 2, "name": "Half Loop" }
                ]},
                "season/2": { "episodes": [
                    { "id": 3, "season_number": 2, "episode_number": 1, "name": "Hello, Ms. Cobel" }
                ]}
            }))],
        )
        .reply(
            "/find/81189?external_source=tvdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [
                { "id": 1396, "name": "Breaking Bad", "first_air_date": "2008-01-20" }
            ]}))],
        )
        .reply(
            "/tv/1396?append_to_response=external_ids",
            vec![Reply::ok(json!({ "id": 1396, "name": "Breaking Bad", "seasons": [] }))],
        );
    let tmdb = fake.start().await;

    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        report,
        RefreshReport {
            matched: 2,
            ..RefreshReport::default()
        }
    );
    assert_eq!(
        statuses(&pool, "series").await,
        vec![
            (
                "Breaking Bad {tvdb-81189}".into(),
                Some("matched".into()),
                Some(1396),
                Some("tvdb_tag".into())
            ),
            (
                "Severance (2022)".into(),
                Some("matched".into()),
                Some(95396),
                Some("search".into())
            ),
        ]
    );
    let series: (Option<i64>, Option<String>, String) =
        sqlx::query_as("SELECT tvdb_id, imdb_id, genres FROM tmdb_series WHERE tmdb_id = 95396")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        series,
        (
            Some(371980),
            Some("tt11280740".into()),
            r#"["Drama"]"#.into()
        )
    );
    let episodes: Vec<(i64, i64, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT season, episode, name, runtime_minutes FROM tmdb_episodes
             WHERE series_tmdb_id = 95396 ORDER BY season, episode",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        episodes,
        vec![
            (1, 1, Some("Good News About Hell".into()), Some(57)),
            (1, 2, Some("Half Loop".into()), None),
            (2, 1, Some("Hello, Ms. Cobel".into()), None),
        ]
    );
}

#[tokio::test]
async fn automatic_results_never_overwrite_a_manual_decision() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Movies).await;
    insert_title(&pool, "movies", lib, "Home Video (2010)", (None, None)).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM movies")
        .fetch_one(&pool)
        .await
        .unwrap();
    unmatch_manually(&pool, Kind::Movie, id).await.unwrap();

    // What a refresh that picked this title before the decision would write.
    assert!(
        !set_match(&pool, Kind::Movie, id, Some((1, "search")), None, 1)
            .await
            .unwrap()
    );
    assert!(
        !set_match(&pool, Kind::Movie, id, None, Some("[]"), 1)
            .await
            .unwrap()
    );
    assert!(
        !set_match(&pool, Kind::Movie, id, None, None, 1)
            .await
            .unwrap()
    );
    let row: (Option<String>, Option<String>, Option<i64>) =
        sqlx::query_as("SELECT match_status, match_method, match_tmdb_id FROM movies WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, (Some("unmatched".into()), Some("manual".into()), None));

    // An admin can still change their mind.
    sqlx::query("INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (7, 'Seven', 0)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        set_match(&pool, Kind::Movie, id, Some((7, "manual")), None, 1)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn manual_decisions_are_never_retried() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Movies).await;
    insert_title(&pool, "movies", lib, "Home Video (2010)", (None, None)).await;
    insert_title(&pool, "movies", lib, "Old Guess (2011)", (None, None)).await;
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM movies ORDER BY rel_path")
        .fetch_all(&pool)
        .await
        .unwrap();
    unmatch_manually(&pool, Kind::Movie, ids[0]).await.unwrap();
    sqlx::query("UPDATE movies SET match_status = 'unmatched', match_method = NULL WHERE id = ?")
        .bind(ids[1])
        .execute(&pool)
        .await
        .unwrap();
    // Both attempts are older than the retry interval.
    sqlx::query("UPDATE movies SET match_attempted_at = 0")
        .execute(&pool)
        .await
        .unwrap();

    let fake = FakeTmdb::default();
    fake.reply(
        "/search/movie?include_adult=false&query=Old Guess&year=2011",
        vec![Reply::ok(json!({ "results": [] }))],
    )
    .reply(
        "/search/movie?include_adult=false&query=Old Guess",
        vec![Reply::ok(json!({ "results": [] }))],
    );
    let tmdb = fake.start().await;
    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(report.unmatched, 1, "only the automatic one is retried");
    assert!(
        fake.log
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.contains("Old Guess"))
    );
}

#[tokio::test]
async fn ids_from_radarr_and_sonarr_win_over_searching() {
    let pool = db::open_in_memory().await;
    let movies = library(&pool, LibraryKind::Movies).await;
    let shows = library(&pool, LibraryKind::Shows).await;
    insert_title(&pool, "movies", movies, "Dune", (None, None)).await;
    insert_title(&pool, "series", shows, "Some Show", (None, None)).await;
    sqlx::query("UPDATE movies SET arr_tmdb_id = 438631")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE series SET arr_tvdb_id = 81189")
        .execute(&pool)
        .await
        .unwrap();

    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/438631",
        vec![Reply::ok(
            json!({ "id": 438631, "title": "Dune", "release_date": "2021-09-15" }),
        )],
    )
    .reply(
        "/find/81189?external_source=tvdb_id",
        vec![Reply::ok(
            json!({ "tv_results": [{ "id": 1396, "name": "Breaking Bad" }] }),
        )],
    )
    .reply(
        "/tv/1396?append_to_response=external_ids",
        vec![Reply::ok(json!({ "id": 1396, "name": "Breaking Bad" }))],
    );
    let tmdb = fake.start().await;
    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(report.matched, 2);
    assert!(
        fake.log
            .lock()
            .unwrap()
            .iter()
            .all(|r| !r.starts_with("/search")),
        "no searching needed"
    );
    assert_eq!(statuses(&pool, "movies").await[0].2, Some(438631));
    assert_eq!(statuses(&pool, "series").await[0].2, Some(1396));
}

#[tokio::test]
async fn an_import_during_a_refresh_isnt_overwritten() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Movies).await;
    insert_title(&pool, "movies", lib, "Dune", (None, None)).await;
    // What the refresh read: no Radarr id yet.
    let row: TitleRow = sqlx::query_as(
        "SELECT id, parsed_title, parsed_year, tmdb_id, imdb_id, NULL AS tvdb_id, arr_tmdb_id,
                 NULL AS arr_tvdb_id FROM movies",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // Meanwhile an import gives the folder Radarr's id.
    sqlx::query("UPDATE movies SET arr_tmdb_id = 438631")
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        !set_auto_match(&pool, Kind::Movie, &row, None, Some("[]"), 1)
            .await
            .unwrap()
    );
    let status: Option<String> = sqlx::query_scalar("SELECT match_status FROM movies")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        status, None,
        "left for the next refresh, which has the new id"
    );
}

#[tokio::test]
async fn rejected_token_stops_the_run_without_marking_anything() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Movies).await;
    insert_title(&pool, "movies", lib, "Dune (2021)", (None, None)).await;
    insert_title(&pool, "movies", lib, "Heat (1995)", (None, None)).await;
    let fake = FakeTmdb::default();
    for title in ["Dune&year=2021", "Heat&year=1995"] {
        fake.reply(
            &format!("/search/movie?include_adult=false&query={title}"),
            vec![Reply::status(StatusCode::UNAUTHORIZED)],
        );
    }
    let tmdb = fake.start().await;

    let err = refresh(&pool, &tmdb).await.unwrap_err();
    assert!(err.is::<Unauthorized>());
    assert_eq!(
        fake.log.lock().unwrap().len(),
        1,
        "stopped after the first refusal"
    );
    let tried: i64 =
        sqlx::query_scalar("SELECT count(*) FROM movies WHERE match_status IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tried, 0);
}

#[tokio::test]
async fn matched_titles_with_a_stale_cache_are_fetched_again() {
    let pool = db::open_in_memory().await;
    let movies = library(&pool, LibraryKind::Movies).await;
    let shows = library(&pool, LibraryKind::Shows).await;
    let now = unix_now();
    // Cached 31 days ago (stale for a movie), 20 days ago (not yet), and 31 days ago for a
    // folder gone from disk; a series cached 8 days ago (stale for a series).
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, title, fetched_at)
             VALUES (438631, 'Old Dune', ?1 - 31 * 86400), (949, 'Old Heat', ?1 - 20 * 86400),
                    (603, 'Old Matrix', ?1 - 31 * 86400);
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at)
             VALUES (95396, 'Old Severance', ?1 - 8 * 86400);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name)
             VALUES (95396, 1, 1, 1, 'Old name')",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    for (rel, tmdb_id) in [
        ("Dune (2021)", 438631),
        ("Heat (1995)", 949),
        ("The Matrix (1999)", 603),
    ] {
        insert_title(&pool, "movies", movies, rel, (None, None)).await;
        sqlx::query(
            "UPDATE movies SET match_status = 'matched', match_tmdb_id = ?, match_method = 'search',
                 match_attempted_at = ? WHERE rel_path = ?",
        )
        .bind(tmdb_id)
        .bind(now)
        .bind(rel)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE movies SET missing_since = ? WHERE rel_path = 'The Matrix (1999)'")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    insert_title(&pool, "series", shows, "Severance (2022)", (None, None)).await;
    sqlx::query(
        "UPDATE series SET match_status = 'matched', match_tmdb_id = 95396, match_method = 'search',
             match_attempted_at = ?",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();

    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/438631",
        vec![Reply::ok(movie(438631, "Dune", "2021-09-15"))],
    )
    .reply(
        "/tv/95396?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 95396, "name": "Severance", "first_air_date": "2022-02-17",
            "seasons": [{ "season_number": 1 }]
        }))],
    )
    .reply(
        "/tv/95396?append_to_response=season/1",
        vec![Reply::ok(json!({ "id": 95396, "name": "Severance",
            "season/1": { "episodes": [
                { "id": 1, "season_number": 1, "episode_number": 1, "name": "Good News About Hell" },
                { "id": 2, "season_number": 1, "episode_number": 2, "name": "Half Loop" }
            ]}
        }))],
    );
    let tmdb = fake.start().await;

    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        report,
        RefreshReport {
            refreshed: 2,
            ..RefreshReport::default()
        }
    );
    let movies: Vec<(i64, String, bool)> =
        sqlx::query_as("SELECT tmdb_id, title, fetched_at >= ? FROM tmdb_movies ORDER BY tmdb_id")
            .bind(now)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        movies,
        vec![
            (603, "Old Matrix".into(), false),
            (949, "Old Heat".into(), false),
            (438631, "Dune".into(), true),
        ],
        "only the stale one still on disk"
    );
    let series: (String, Option<i64>, bool) =
        sqlx::query_as("SELECT name, year, fetched_at >= ? FROM tmdb_series WHERE tmdb_id = 95396")
            .bind(now)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(series, ("Severance".into(), Some(2022), true));
    let episodes: Vec<(i64, String)> = sqlx::query_as(
        "SELECT episode, name FROM tmdb_episodes WHERE series_tmdb_id = 95396 ORDER BY episode",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        episodes,
        vec![(1, "Good News About Hell".into()), (2, "Half Loop".into())]
    );
    let log = fake.log.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|r| r.starts_with("/movie/949") || r.starts_with("/movie/603")),
        "{log:?}"
    );
    assert!(
        log.iter().all(|r| !r.starts_with("/search")),
        "matched titles aren't searched again: {log:?}"
    );
}

#[tokio::test]
async fn wanted_titles_are_fetched_for_the_calendar() {
    let pool = db::open_in_memory().await;
    sqlx::query(
            "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at)
             VALUES (693134, 'Dune: Part Two', 2024, 1, 'radarr', 0),
                    (603, 'The Matrix', 1999, 1, 'radarr', 0),
                    (1, 'Unwanted', 2024, 0, 'radarr', 0);
             INSERT INTO wanted_series (tvdb_id, tmdb_id, title, monitored, series_type, source, added_at)
             VALUES (424536, NULL, 'Frieren', 1, 'anime', 'sonarr', 0),
                    (81189, 1396, 'Breaking Bad', 1, 'standard', 'sonarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
            "/movie/693134",
            vec![Reply::ok(movie(693134, "Dune: Part Two", "2099-02-27"))],
        )
        .reply(
            "/movie/603",
            vec![Reply::ok(movie(603, "The Matrix", "1999-03-31"))],
        )
        .reply(
            "/movie/693134/release_dates",
            vec![Reply::ok(json!({ "id": 693134, "results": [
                { "iso_3166_1": "US", "release_dates": [
                    { "release_date": "2099-03-01T00:00:00.000Z", "type": 3 },
                    { "release_date": "2099-04-16T00:00:00.000Z", "type": 4 },
                    { "release_date": "2099-05-14T00:00:00.000Z", "type": 5 }
                ]},
                { "iso_3166_1": "GB", "release_dates": [
                    { "release_date": "2099-04-15T00:00:00.000Z", "type": 4 },
                    { "release_date": "", "type": 5 }
                ]}
            ]}))],
        )
        .reply(
            "/find/424536?external_source=tvdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [
                { "id": 209867, "name": "Frieren: Beyond Journey's End", "first_air_date": "2023-09-29" }
            ]}))],
        )
        .reply(
            "/tv/209867?append_to_response=external_ids",
            vec![Reply::ok(json!({
                "id": 209867, "name": "Frieren: Beyond Journey's End", "status": "Returning Series",
                "seasons": [{ "season_number": 1 }],
                "external_ids": { "tvdb_id": 424536 }
            }))],
        )
        .reply(
            "/tv/209867?append_to_response=season/1",
            vec![Reply::ok(json!({ "id": 209867, "name": "Frieren",
                "season/1": { "episodes": [
                    { "id": 11, "season_number": 1, "episode_number": 1, "name": "The Journey's End", "air_date": "2023-09-29" }
                ]}
            }))],
        )
        .reply(
            "/tv/1396?append_to_response=external_ids",
            vec![Reply::ok(json!({ "id": 1396, "name": "Breaking Bad", "status": "Ended",
                "seasons": [], "external_ids": { "tvdb_id": 81189 } }))],
        );
    let tmdb = fake.start().await;

    let report = refresh_wanted(&pool, &tmdb).await.unwrap();
    assert_eq!((report.refreshed, report.errors), (5, 0), "{report:?}");
    let dates: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT release_date, digital_release, physical_release FROM tmdb_movies WHERE tmdb_id = 693134",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        dates,
        (
            Some("2099-02-27".into()),
            Some("2099-04-15".into()),
            Some("2099-05-14".into())
        ),
        "the earliest of each kind anywhere"
    );
    let log = fake.log.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|r| r.starts_with("/movie/603/release_dates")),
        "an old film's dates aren't asked for: {log:?}"
    );
    assert!(
        !log.iter().any(|r| r == "/movie/1"),
        "not monitored: {log:?}"
    );
    let episodes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tmdb_episodes WHERE series_tmdb_id = 209867")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(episodes, 1);

    // Nothing is due again at once; a day on, only the series still airing is.
    let again = refresh_wanted(&pool, &tmdb).await.unwrap();
    assert_eq!(again.refreshed, 0);
    sqlx::query("UPDATE tmdb_series SET fetched_at = fetched_at - 2 * 86400")
        .execute(&pool)
        .await
        .unwrap();
    fake.log.lock().unwrap().clear();
    let day_on = refresh_wanted(&pool, &tmdb).await.unwrap();
    assert_eq!(day_on.refreshed, 1);
    let log = fake.log.lock().unwrap().clone();
    assert!(log.iter().any(|r| r.starts_with("/tv/209867")), "{log:?}");
    assert!(
        !log.iter().any(|r| r.starts_with("/tv/1396")),
        "ended: {log:?}"
    );
}

#[tokio::test]
async fn stale_wanted_entries_are_fetched_again() {
    let pool = db::open_in_memory().await;
    let now = unix_now();
    // Release dates asked for 8 days ago (due) and yesterday (not yet); a series still airing
    // cached 2 days ago (due) and an ended one cached as long ago (never due).
    sqlx::query(
        "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at)
             VALUES (693134, 'Dune: Part Two', 2099, 1, 'radarr', 0),
                    (1011985, 'Asked Yesterday', 2099, 1, 'radarr', 0);
         INSERT INTO tmdb_movies (tmdb_id, title, release_date, fetched_at, release_dates_at)
             VALUES (693134, 'Dune: Part Two', '2099-02-27', ?1, ?1 - 8 * 86400),
                    (1011985, 'Asked Yesterday', '2099-02-27', ?1, ?1 - 86400);
         INSERT INTO wanted_series (tvdb_id, tmdb_id, title, monitored, series_type, source, added_at)
             VALUES (424536, 209867, 'Frieren', 1, 'anime', 'sonarr', 0),
                    (81189, 1396, 'Breaking Bad', 1, 'standard', 'sonarr', 0);
         INSERT INTO tmdb_series (tmdb_id, name, status, fetched_at)
             VALUES (209867, 'Old Frieren', 'Returning Series', ?1 - 2 * 86400),
                    (1396, 'Breaking Bad', 'Ended', ?1 - 2 * 86400)",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/693134/release_dates",
        vec![Reply::ok(json!({ "id": 693134, "results": [
            { "iso_3166_1": "US", "release_dates": [
                { "release_date": "2099-04-16T00:00:00.000Z", "type": 4 },
                { "release_date": "2099-05-14T00:00:00.000Z", "type": 5 }
            ]}
        ]}))],
    )
    .reply(
        "/tv/209867?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 209867, "name": "Frieren: Beyond Journey's End", "status": "Returning Series",
            "seasons": [{ "season_number": 1 }]
        }))],
    )
    .reply(
        "/tv/209867?append_to_response=season/1",
        vec![Reply::ok(json!({ "id": 209867, "name": "Frieren",
            "season/1": { "episodes": [
                { "id": 11, "season_number": 1, "episode_number": 1, "name": "The Journey's End" }
            ]}
        }))],
    );
    let tmdb = fake.start().await;

    let report = refresh_wanted(&pool, &tmdb).await.unwrap();
    assert_eq!((report.refreshed, report.errors), (2, 0), "{report:?}");
    let dates: Vec<(i64, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT tmdb_id, digital_release, physical_release, release_dates_at >= ?
             FROM tmdb_movies ORDER BY tmdb_id",
    )
    .bind(now)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        dates,
        vec![
            (
                693134,
                Some("2099-04-16".into()),
                Some("2099-05-14".into()),
                true
            ),
            (1011985, None, None, false),
        ]
    );
    let series: Vec<(i64, String, bool)> =
        sqlx::query_as("SELECT tmdb_id, name, fetched_at >= ? FROM tmdb_series ORDER BY tmdb_id")
            .bind(now)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        series,
        vec![
            (1396, "Breaking Bad".into(), false),
            (209867, "Frieren: Beyond Journey's End".into(), true),
        ]
    );
    let episodes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tmdb_episodes WHERE series_tmdb_id = 209867")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(episodes, 1);
    let log = fake.log.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|r| r.starts_with("/movie/1011985") || r.starts_with("/tv/1396")),
        "{log:?}"
    );
}

#[tokio::test]
async fn nfo_ids_match_below_folder_tags_and_disagreeing_ones_stay_ambiguous() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Movies).await;
    for rel in ["Matrix", "Mixed Up", "Tagged [tmdbid-438631]", "Unknown"] {
        let tag = crate::library::parse::parse_title(rel).tmdb_id;
        insert_title(&pool, "movies", lib, rel, (tag, None)).await;
    }
    sqlx::raw_sql(
        r#"UPDATE movies SET nfo_ids = '{"tmdb":[603],"imdb":["tt0133093"]}' WHERE rel_path = 'Matrix';
           UPDATE movies SET nfo_ids = '{"tmdb":[603],"imdb":["tt1375666"]}' WHERE rel_path = 'Mixed Up';
           UPDATE movies SET nfo_ids = '{"tmdb":[603]}' WHERE rel_path LIKE 'Tagged%';
           UPDATE movies SET nfo_ids = '{"tmdb":[999999]}' WHERE rel_path = 'Unknown';"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut matrix = movie(603, "The Matrix", "1999-03-31");
    matrix["imdb_id"] = json!("tt0133093");
    let fake = FakeTmdb::default();
    fake.reply("/movie/603", vec![Reply::ok(matrix)])
        .reply(
            "/movie/438631",
            vec![Reply::ok(movie(438631, "Dune", "2021-09-15"))],
        )
        .reply(
            "/find/tt1375666?external_source=imdb_id",
            vec![Reply::ok(
                json!({ "movie_results": [{ "id": 27205, "title": "Inception" }] }),
            )],
        )
        .reply(
            "/movie/27205",
            vec![Reply::ok(movie(27205, "Inception", "2010-07-15"))],
        )
        .reply(
            "/search/movie?include_adult=false&query=Unknown",
            vec![Reply::ok(json!({ "results": [] }))],
        );
    let tmdb = fake.start().await;
    refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        statuses(&pool, "movies").await,
        vec![
            (
                "Matrix".into(),
                Some("matched".into()),
                Some(603),
                Some("nfo".into())
            ),
            ("Mixed Up".into(), Some("ambiguous".into()), None, None),
            (
                "Tagged [tmdbid-438631]".into(),
                Some("matched".into()),
                Some(438631),
                Some("tmdb_tag".into())
            ),
            ("Unknown".into(), Some("unmatched".into()), None, None),
        ]
    );
    let log = fake.log.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|r| r == "/find/tt0133093?external_source=imdb_id"),
        "an IMDb id the TMDB entry already has isn't looked up: {log:?}"
    );
    assert!(
        !log.iter().any(|r| r.contains("query=Matrix")),
        "an .nfo id needs no search: {log:?}"
    );
    let candidates: String =
        sqlx::query_scalar("SELECT match_candidates FROM movies WHERE rel_path = 'Mixed Up'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let ids: Vec<i64> = serde_json::from_str::<Vec<serde_json::Value>>(&candidates)
        .unwrap()
        .iter()
        .map(|c| c["tmdb_id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [603, 27205]);
}

#[tokio::test]
async fn series_nfo_ids_match_through_tvdb() {
    let pool = db::open_in_memory().await;
    let lib = library(&pool, LibraryKind::Shows).await;
    insert_title(&pool, "series", lib, "Bad", (None, None)).await;
    sqlx::query(r#"UPDATE series SET nfo_ids = '{"tvdb":[81189]}'"#)
        .execute(&pool)
        .await
        .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/find/81189?external_source=tvdb_id",
        vec![Reply::ok(
            json!({ "tv_results": [{ "id": 1396, "name": "Breaking Bad" }] }),
        )],
    )
    .reply(
        "/tv/1396?append_to_response=external_ids",
        vec![Reply::ok(json!({ "id": 1396, "name": "Breaking Bad" }))],
    );
    let tmdb = fake.start().await;
    refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        statuses(&pool, "series").await,
        vec![(
            "Bad".into(),
            Some("matched".into()),
            Some(1396),
            Some("nfo".into())
        )]
    );
}

#[tokio::test]
async fn metadata_comes_in_the_chosen_language_with_english_where_missing() {
    let pool = db::open_in_memory().await;
    let movies = library(&pool, LibraryKind::Movies).await;
    insert_title(&pool, "movies", movies, "The Matrix (1999)", (None, None)).await;
    insert_title(&pool, "movies", movies, "Dune (2021)", (None, None)).await;
    // The Matrix was matched and fetched in English a moment ago.
    let now = crate::db::unix_now();
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, title, overview, fetched_at, language)
         VALUES (603, 'The Matrix', 'A hacker learns the truth.', ?, 'en-US')",
    )
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE movies SET match_status = 'matched', match_tmdb_id = 603, match_method = 'search'
         WHERE rel_path LIKE 'The Matrix%'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let english = json!({ "iso_639_1": "en", "iso_3166_1": "US",
        "data": { "title": "The Matrix", "overview": "A hacker learns the truth." } });
    let mut matrix = movie(603, "Matrix", "1999-03-31");
    matrix["overview"] = json!("");
    matrix["original_language"] = json!("en");
    matrix["translations"] = json!({ "translations": [english,
        { "iso_639_1": "fr", "iso_3166_1": "FR", "data": { "title": "Matrix", "overview": "" } }] });
    let dune = Reply::ok(json!({ "results": [
        { "id": 438631, "title": "Dune", "original_title": "Dune", "release_date": "2021-09-15" }
    ]}));
    let fake = FakeTmdb::default();
    fake.language("fr-FR")
        .reply(
            "/movie/603?append_to_response=translations#fr-FR",
            vec![Reply::ok(matrix)],
        )
        .reply(
            "/search/movie?include_adult=false&query=Dune&year=2021",
            vec![dune],
        )
        .reply(
            "/movie/438631?append_to_response=translations#fr-FR",
            vec![Reply::ok(movie(438631, "Dune", "2021-09-15"))],
        );
    let tmdb = fake.start().await.with_language("fr-FR".into());
    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!((report.matched, report.refreshed), (1, 1), "{report:?}");

    let matrix: (String, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT title, overview, language, english_fields FROM tmdb_movies WHERE tmdb_id = 603",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        matrix,
        (
            "Matrix".into(),
            Some("A hacker learns the truth.".into()),
            Some("fr-FR".into()),
            Some(r#"["overview"]"#.into())
        ),
        "fetched again for the new language, the untranslated summary in English"
    );
    let log = fake.log.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|r| r == "/search/movie?include_adult=false&query=Dune&year=2021"),
        "searches stay in English: {log:?}"
    );

    // Fetched in French now, it isn't fetched again.
    let again = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(again.refreshed, 0);
}

#[tokio::test]
async fn episodes_without_a_translation_get_english_names() {
    let pool = db::open_in_memory().await;
    let shows = library(&pool, LibraryKind::Shows).await;
    insert_title(
        &pool,
        "series",
        shows,
        "Show [tmdbid-95396]",
        (Some(95396), None),
    )
    .await;
    let details = json!({ "id": 95396, "name": "Série", "first_air_date": "2022-02-17",
        "seasons": [{ "season_number": 1 }] });
    let season = |first: &str, second: &str| {
        json!({ "id": 95396, "name": "x", "season/1": { "episodes": [
            { "id": 1, "season_number": 1, "episode_number": 1, "name": first, "overview": "o" },
            { "id": 2, "season_number": 1, "episode_number": 2, "name": second, "overview": "o" }
        ]}})
    };
    let fake = FakeTmdb::default();
    fake.language("de-DE")
        .reply(
            "/tv/95396?append_to_response=external_ids,translations#de-DE",
            vec![Reply::ok(details)],
        )
        .reply(
            "/tv/95396?append_to_response=season/1#de-DE",
            vec![Reply::ok(season("Gute Nachrichten", ""))],
        )
        .reply(
            "/tv/95396?append_to_response=season/1",
            vec![Reply::ok(season("Good News About Hell", "Half Loop"))],
        );
    let tmdb = fake.start().await.with_language("de-DE".into());
    refresh(&pool, &tmdb).await.unwrap();
    let names: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT name, english_fields FROM tmdb_episodes ORDER BY episode")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        names,
        [
            ("Gute Nachrichten".into(), None),
            ("Half Loop".into(), Some(r#"["name"]"#.into()))
        ]
    );
}

#[tokio::test]
async fn cast_crew_and_collections_come_with_library_titles() {
    let pool = db::open_in_memory().await;
    let movies = library(&pool, LibraryKind::Movies).await;
    let shows = library(&pool, LibraryKind::Shows).await;
    insert_title(
        &pool,
        "movies",
        movies,
        "The Matrix (1999) [tmdbid-603]",
        (Some(603), None),
    )
    .await;
    insert_title(
        &pool,
        "series",
        shows,
        "Breaking Bad [tmdbid-1396]",
        (Some(1396), None),
    )
    .await;
    let mut matrix = movie(603, "The Matrix", "1999-03-31");
    matrix["belongs_to_collection"] = json!({ "id": 2344, "name": "The Matrix Collection",
        "poster_path": "/coll.jpg", "backdrop_path": null });
    let mut cast: Vec<serde_json::Value> = (0..25)
        .map(|n| {
            json!({ "id": 1000 + n, "name": format!("Extra {n}"), "character": "Agent",
            "order": n + 5, "profile_path": null })
        })
        .collect();
    // Two actors with one name are two people.
    cast.push(
        json!({ "id": 6384, "name": "Keanu Reeves", "character": "Neo", "order": 0,
        "profile_path": "/keanu.jpg" }),
    );
    cast.push(json!({ "id": 7777, "name": "Keanu Reeves", "character": "", "order": 1 }));
    let crew = json!([
        { "id": 9339, "name": "Lana Wachowski", "job": "Director" },
        { "id": 9339, "name": "Lana Wachowski", "job": "Writer" },
        { "id": 5, "name": "Someone", "job": "Gaffer" }
    ]);
    let fake = FakeTmdb::default();
    fake.reply("/movie/603", vec![Reply::ok(matrix)])
        .reply(
            "/movie/603/credits",
            vec![Reply::ok(json!({ "cast": cast, "crew": crew }))],
        )
        .reply(
            "/tv/1396?append_to_response=external_ids",
            vec![Reply::ok(
                json!({ "id": 1396, "name": "Breaking Bad", "seasons": [] }),
            )],
        )
        .reply(
            "/tv/1396?append_to_response=credits",
            vec![Reply::ok(json!({ "id": 1396,
                "created_by": [{ "id": 66633, "name": "Vince Gilligan" }],
                "credits": { "cast": [{ "id": 17419, "name": "Bryan Cranston",
                    "character": "Walter White", "order": 0 }], "crew": [] } }))],
        );
    let tmdb = fake.start().await;
    refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(refresh_credits(&pool, &tmdb).await.unwrap(), 2);

    let collection: (i64, String) = sqlx::query_as(
        "SELECT c.id, c.name FROM tmdb_movies t JOIN tmdb_collections c ON c.id = t.collection_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(collection, (2344, "The Matrix Collection".into()));
    let matrix: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT person_id, role, character, job FROM tmdb_credits
         WHERE movie_tmdb_id = 603 ORDER BY position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(matrix.len(), 2 + 20, "the key crew and the top 20 billed");
    assert_eq!(
        matrix[..4],
        [
            (9339, "crew".into(), None, Some("Director".into())),
            (9339, "crew".into(), None, Some("Writer".into())),
            (6384, "cast".into(), Some("Neo".into()), None),
            (7777, "cast".into(), None, None),
        ]
    );
    let keanus: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tmdb_people WHERE name = 'Keanu Reeves'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(keanus, 2);
    let bad: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT p.name, coalesce(c.job, c.character) FROM tmdb_credits c
         JOIN tmdb_people p ON p.id = c.person_id WHERE series_tmdb_id = 1396 ORDER BY position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        bad,
        [
            ("Vince Gilligan".into(), Some("Creator".into())),
            ("Bryan Cranston".into(), Some("Walter White".into()))
        ]
    );

    // Fetched once with the details: not again until they are.
    fake.log.lock().unwrap().clear();
    assert_eq!(refresh_credits(&pool, &tmdb).await.unwrap(), 0);
    assert!(fake.log.lock().unwrap().is_empty());
}

/// A download imported for a wanted title, into `destination`.
async fn imported_into(
    pool: &SqlitePool,
    column: &str,
    wanted: i64,
    destination: &str,
    state: &str,
) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO downloads (infohash, title, {column}, decision, state, destination, added_at,
             updated_at)
         VALUES (lower(hex(randomblob(20))), 'Some.Release', ?, '{{}}', ?, ?, 0, 0)"
    )))
    .bind(wanted)
    .bind(state)
    .bind(destination)
    .execute(pool)
    .await
    .unwrap();
}

/// Severance's folder in a shows library, wanted, with an episode imported into its season folder.
async fn an_imported_episode(pool: &SqlitePool) {
    let shows: i64 = sqlx::query_scalar(
        "INSERT INTO libraries (name, kind, path) VALUES ('shows', 'shows', '/media/s') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    insert_title(pool, "series", shows, "Severance (2022)", (None, None)).await;
    sqlx::query(
        "INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, source,
             added_at)
         VALUES (1, 371980, 95396, 'Severance', 1, 'standard', 'manual', 0)",
    )
    .execute(pool)
    .await
    .unwrap();
    let season = "/media/s/Severance (2022)/Season 01";
    imported_into(pool, "wanted_series_id", 1, season, "imported").await;
}

/// The matched folders of `table`, by path, with their TMDB id and how they were matched.
async fn matched(pool: &SqlitePool, table: &str) -> Vec<(String, Option<i64>, Option<String>)> {
    statuses(pool, table)
        .await
        .into_iter()
        .filter(|(_, status, ..)| status.as_deref() == Some("matched"))
        .map(|(rel, _, id, method)| (rel, id, method))
        .collect()
}

#[tokio::test]
async fn imported_titles_are_matched_to_what_they_were_grabbed_for_at_once() {
    let pool = db::open_in_memory().await;
    let movies = library(&pool, LibraryKind::Movies).await;
    for rel in ["Dune (2021)", "Two Titles (2020)", "Still Copying (2019)"] {
        insert_title(&pool, "movies", movies, rel, (None, None)).await;
    }
    sqlx::query(
        "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at) VALUES
             (1, 438631, 'Dune', 1, 'manual', 0), (2, 1, 'One', 1, 'manual', 0),
             (3, 2, 'Two', 1, 'manual', 0), (4, 3, 'Still Copying', 1, 'manual', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (wanted, folder, state) in [
        (1, "Dune (2021)", "imported"),
        // Downloads for two different titles in one folder: which it is isn't sure.
        (2, "Two Titles (2020)", "imported"),
        (3, "Two Titles (2020)", "imported"),
        // Not imported yet.
        (4, "Still Copying (2019)", "transferring"),
    ] {
        let destination = format!("/media/x/{folder}");
        imported_into(&pool, "wanted_movie_id", wanted, &destination, state).await;
    }
    an_imported_episode(&pool).await;

    // TMDB answers only for the titles themselves: nothing is searched for.
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/438631",
        vec![Reply::ok(movie(438631, "Dune", "2021-09-15"))],
    )
    .reply(
        "/tv/95396?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 95396, "name": "Severance", "first_air_date": "2022-02-17",
            "seasons": [{ "season_number": 1 }],
            "external_ids": { "imdb_id": "tt11280740", "tvdb_id": 371980 }
        }))],
    )
    .reply(
        "/tv/95396?append_to_response=season/1",
        vec![Reply::ok(json!({ "id": 95396, "name": "Severance",
            "season/1": { "episodes": [
                { "id": 1, "season_number": 1, "episode_number": 1, "name": "Good News About Hell" }
            ]}
        }))],
    );
    let tmdb = fake.start().await;
    refresh(&pool, &tmdb).await.unwrap();

    assert_eq!(
        matched(&pool, "movies").await,
        [("Dune (2021)".into(), Some(438631), Some("import".into()))]
    );
    assert_eq!(
        matched(&pool, "series").await,
        [(
            "Severance (2022)".into(),
            Some(95396),
            Some("import".into())
        )]
    );
    let episode: String =
        sqlx::query_scalar("SELECT name FROM tmdb_episodes WHERE series_tmdb_id = 95396")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(episode, "Good News About Hell", "its details came with it");
}

#[tokio::test]
async fn a_tv_films_premiere_is_kept_with_its_release_dates() {
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at)
         VALUES (77, 'A TV Film', 2099, 1, 'radarr', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/77",
        vec![Reply::ok(movie(77, "A TV Film", "2099-06-01"))],
    )
    .reply(
        "/movie/77/release_dates",
        vec![Reply::ok(json!({ "id": 77, "results": [
            { "iso_3166_1": "US", "release_dates": [
                { "release_date": "2099-06-03T00:00:00.000Z", "type": 6 }
            ]},
            { "iso_3166_1": "GB", "release_dates": [
                { "release_date": "2099-06-01T00:00:00.000Z", "type": 6 }
            ]}
        ]}))],
    );
    let tmdb = fake.start().await;
    refresh_wanted(&pool, &tmdb).await.unwrap();
    let dates: (Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT digital_release, tv_release, release_dates_at IS NOT NULL FROM tmdb_movies
         WHERE tmdb_id = 77",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(dates, (None, Some("2099-06-01".into()), true));
}

#[tokio::test]
async fn keeps_a_titles_own_language_with_its_details() {
    let pool = db::open_in_memory().await;
    let movies = library(&pool, LibraryKind::Movies).await;
    let shows = library(&pool, LibraryKind::Shows).await;
    insert_title(
        &pool,
        "movies",
        movies,
        "Spirited Away (2001)",
        (Some(129), None),
    )
    .await;
    insert_title(&pool, "movies", movies, "Untold (2003)", (Some(130), None)).await;
    insert_title(
        &pool,
        "series",
        shows,
        "Squid Game (2021)",
        (Some(93405), None),
    )
    .await;
    let mut spirited = movie(129, "Spirited Away", "2001-07-20");
    spirited["original_language"] = json!("ja");
    let fake = FakeTmdb::default();
    fake.reply("/movie/129", vec![Reply::ok(spirited)])
        .reply(
            "/movie/130",
            vec![Reply::ok(movie(130, "Untold", "2003-01-01"))],
        )
        .reply(
            "/tv/93405?append_to_response=external_ids",
            vec![Reply::ok(json!({
                "id": 93405, "name": "Squid Game", "first_air_date": "2021-09-17",
                "original_language": "ko", "genres": [], "seasons": [{ "season_number": 1 }]
            }))],
        )
        .reply(
            "/tv/93405?append_to_response=season/1",
            vec![Reply::ok(
                json!({ "id": 93405, "season/1": { "episodes": [] } }),
            )],
        );
    let tmdb = fake.start().await;
    refresh(&pool, &tmdb).await.unwrap();
    let movies: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT tmdb_id, original_language FROM tmdb_movies ORDER BY tmdb_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(movies, [(129, Some("ja".into())), (130, None)]);
    let series: Option<String> =
        sqlx::query_scalar("SELECT original_language FROM tmdb_series WHERE tmdb_id = 93405")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(series.as_deref(), Some("ko"));
}
