//! Import lists: charts and lists from TMDB, IMDb or Rotten Tomatoes, whose titles are
//! added to the wanted list every 12 hours. Each title is found on TMDB first (by TMDB id, by
//! IMDb id, or for Rotten Tomatoes by a strict title-and-year search), and anything that
//! can't be pinned to one title is skipped with the reason. Titles already wanted are left as
//! they are (monitored or not), one an admin took off the list in Melete isn't added back, and
//! nothing is ever removed from the wanted list.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::db::unique_violation;
use crate::db::unix_now;
use crate::metadata::{self, Candidate, Choice};
use crate::tmdb::{self, ExternalId};
use crate::{imdb, rottentomatoes, want};

mod adding;
mod editing;
mod pinning;
mod records;
mod syncing;

use adding::*;
pub use editing::*;
use pinning::*;
pub use records::*;
pub use syncing::*;

/// How often lists are synced.
pub const EVERY: Duration = Duration::from_secs(12 * 3600);
/// TMDB pages read at most (20 titles each).
const MAX_PAGES: u32 = 13;

/// What syncing needs.
#[derive(Clone)]
pub struct Sources {
    /// TMDB, which every title is pinned to and some lists are read from.
    pub tmdb: Arc<tmdb::Client>,
    /// IMDb, for its charts and public lists.
    pub imdb: imdb::Client,
    /// Rotten Tomatoes, for its browse pages.
    pub rottentomatoes: rottentomatoes::Client,
}

/// Syncs every enabled list every [`EVERY`] for as long as the server runs.
pub async fn run(pool: SqlitePool, sources: Sources) {
    loop {
        // After the startup scan, so titles on disk are known.
        crate::tasks::scheduled("import-lists", Duration::from_secs(10 * 60));
        tokio::time::sleep(Duration::from_secs(10 * 60)).await;
        crate::tasks::started("import-lists");
        let mut failures = 0;
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM import_lists WHERE enabled = 1")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
        for id in ids {
            match sync(&pool, &sources, id).await {
                Ok(Some(result)) if result.added > 0 => {
                    tracing::info!(list = id, added = result.added, "import list added titles")
                }
                Ok(_) => {}
                Err(err) => {
                    failures += 1;
                    tracing::warn!(
                        list = id,
                        error = format!("{err:#}"),
                        "import list sync failed"
                    )
                }
            }
        }
        let result =
            (failures > 0).then(|| format!("{failures} list(s) failed; the server's log says why"));
        crate::tasks::finished("import-lists", result, Some(EVERY));
        tokio::time::sleep(EVERY.saturating_sub(Duration::from_secs(10 * 60))).await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db;
    use crate::profiles::import::tests::converted;
    use crate::profiles::{self, App};
    use crate::tmdb::testing::{FakeTmdb, Reply};

    async fn setup() -> SqlitePool {
        let pool = db::open_in_memory().await;
        for app in [App::Radarr, App::Sonarr] {
            profiles::save(&pool, app, &converted(app)).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'), (2, 'shows', 'shows', '/s')",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn input(provider: &str, list: &str, kind: &str) -> ListInput {
        ListInput {
            name: format!("{provider} {list} {kind}"),
            provider: provider.into(),
            list: list.into(),
            kind: kind.into(),
            library_id: if kind == "movies" { 1 } else { 2 },
            monitored: true,
            quality_profile: "4K Preferred".into(),
            max_items: 10,
            monitor_seasons: "latest".into(),
            enabled: true,
        }
    }

    async fn sources(
        fake: &FakeTmdb,
        imdb_ids: Vec<&'static str>,
        page: Option<&'static str>,
    ) -> Sources {
        let rt_base = match page {
            Some(page) => {
                let router = axum::Router::new().fallback(move || async move { page });
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
                Some(url)
            }
            None => None,
        };
        Sources {
            tmdb: Arc::new(fake.start().await),
            imdb: imdb::testing::start(imdb_ids).await.0,
            rottentomatoes: rottentomatoes::Client::with_base(rt_base).unwrap(),
        }
    }

    fn movie(id: i64, title: &str, date: &str) -> Reply {
        Reply::ok(
            json!({ "id": id, "title": title, "release_date": date, "imdb_id": "tt15239678",
                          "original_language": "en", "genres": [] }),
        )
    }

    #[tokio::test]
    async fn settings_are_checked() {
        let pool = setup().await;
        assert!(
            check(&pool, &input("tmdb", "popular", "movies"))
                .await
                .unwrap()
                .is_ok()
        );
        assert!(
            check(&pool, &input("tmdb", "list:8123", "series"))
                .await
                .unwrap()
                .is_ok()
        );
        assert!(
            check(&pool, &input("imdb", "ls055592025", "movies"))
                .await
                .unwrap()
                .is_ok()
        );
        let rt = "https://www.rottentomatoes.com/browse/movies_at_home/sort:popular";
        assert!(
            check(&pool, &input("rottentomatoes", rt, "movies"))
                .await
                .unwrap()
                .is_ok()
        );
        for (bad, why) in [
            (input("tmdb", "upcoming", "series"), "isn't a tmdb list"),
            (input("tmdb", "list:../x", "movies"), "isn't a tmdb list"),
            (input("imdb", "ls1", "movies"), "isn't a imdb list"),
            (
                input("rottentomatoes", "https://example.com/browse/x", "movies"),
                "isn't a rottentomatoes list",
            ),
            (input("trakt", "popular", "movies"), "provider is"),
            (
                ListInput {
                    library_id: 2,
                    ..input("tmdb", "popular", "movies")
                },
                "go in a movies library",
            ),
            (
                ListInput {
                    quality_profile: "Nope".into(),
                    ..input("tmdb", "popular", "movies")
                },
                "no radarr quality profile",
            ),
            (
                ListInput {
                    max_items: 0,
                    ..input("tmdb", "popular", "movies")
                },
                "max_items",
            ),
        ] {
            let got = check(&pool, &bad).await.unwrap().unwrap_err();
            assert!(got.contains(why), "{got}");
        }
        let id = create(&pool, &input("tmdb", "popular", "movies"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            create(&pool, &input("tmdb", "popular", "movies"))
                .await
                .unwrap(),
            Err(Refused::Taken)
        );
        assert!(get(&pool, id).await.unwrap().unwrap().last_result.is_none());
        assert!(delete(&pool, id).await.unwrap());
    }

    /// Title, year, profile, library, folder, source, original language, list.
    type AddedRow = (
        String,
        i64,
        String,
        i64,
        Option<i64>,
        String,
        Option<String>,
        Option<i64>,
    );

    #[tokio::test]
    async fn a_tmdb_chart_adds_its_movies_once() {
        let pool = setup().await;
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             VALUES (5, 1, 'Dune Part Two (2024)', 'Dune Part Two', 1, NULL);
             UPDATE movies SET arr_tmdb_id = 693134 WHERE id = 5;
             INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at) VALUES (603, 'The Matrix', 1, 'radarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let fake = FakeTmdb::default();
        fake.reply(
            "/movie/popular?page=1",
            vec![Reply::ok(json!({ "total_pages": 1, "results": [
                { "id": 693134, "title": "Dune: Part Two" }, { "id": 603, "title": "The Matrix" },
                { "id": 1, "title": "Gone" }
            ]}))],
        )
        .reply(
            "/movie/693134",
            vec![movie(693134, "Dune: Part Two", "2024-02-27")],
        )
        .reply(
            "/movie/1",
            vec![Reply::status(axum::http::StatusCode::NOT_FOUND)],
        );
        let sources = sources(&fake, vec![], None).await;
        let id = create(&pool, &input("tmdb", "popular", "movies"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!(
            (result.found, result.added, result.already_wanted),
            (3, 1, 1)
        );
        assert_eq!(
            result.skipped,
            [Skipped {
                title: "Gone".into(),
                why: "TMDB no longer has it".into()
            }]
        );
        let added: AddedRow = sqlx::query_as(
            "SELECT title, year, quality_profile, library_id, movie_id, source, original_language, list_id
             FROM wanted_movies WHERE tmdb_id = 693134",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            added,
            (
                "Dune: Part Two".into(),
                2024,
                "4K Preferred".into(),
                1,
                Some(5),
                "manual".into(),
                Some("en".into()),
                Some(id)
            )
        );
        let again = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!((again.added, again.already_wanted), (0, 2));
        let listed = get(&pool, id).await.unwrap().unwrap();
        assert!(listed.synced_at.is_some());
        assert_eq!(listed.last_result.unwrap()["already_wanted"], 2);
        // Deleting the list keeps what it added.
        delete(&pool, id).await.unwrap();
        let kept: Option<i64> =
            sqlx::query_scalar("SELECT list_id FROM wanted_movies WHERE tmdb_id = 693134")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(kept, None);
    }

    /// What a sync said of `title`, when it skipped it.
    fn why_skipped<'a>(result: &'a SyncResult, title: &str) -> Option<&'a str> {
        result
            .skipped
            .iter()
            .find(|s| s.title == title)
            .map(|s| s.why.as_str())
    }

    #[tokio::test]
    async fn a_tmdb_list_leaves_off_what_an_admin_took_off_or_unmonitored() {
        let pool = setup().await;
        // Dune was taken off the list in Melete; The Matrix is wanted and unmonitored there.
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
                 VALUES (1, 693134, 'Dune: Part Two', 1, 'manual', 0),
                        (2, 603, 'The Matrix', 1, 'radarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::wanted_edit::remove(&pool, crate::wanted_edit::Kind::Movie, 1)
            .await
            .unwrap()
            .unwrap();
        crate::wanted_edit::set_monitored(&pool, crate::wanted_edit::Kind::Movie, 2, false)
            .await
            .unwrap();
        let fake = FakeTmdb::default();
        fake.reply(
            "/movie/popular?page=1",
            vec![
                Reply::ok(json!({ "total_pages": 1, "results": [
                    { "id": 693134, "title": "Dune: Part Two" }, { "id": 603, "title": "The Matrix" }
                ]}));
                2
            ],
        )
        .reply(
            "/movie/693134",
            vec![movie(693134, "Dune: Part Two", "2024-02-27")],
        );
        let sources = sources(&fake, vec![], None).await;
        let id = create(&pool, &input("tmdb", "popular", "movies"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!((result.added, result.already_wanted), (0, 1), "{result:?}");
        assert!(
            why_skipped(&result, "Dune: Part Two")
                .unwrap()
                .starts_with("taken off the wanted list in Melete")
        );
        let rows: Vec<(i64, bool)> =
            sqlx::query_as("SELECT tmdb_id, monitored FROM wanted_movies ORDER BY tmdb_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows, [(603, false)], "as the admin left them");

        // Wanted again by hand: the list takes it as wanted already.
        sqlx::query(
            "INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at)
             VALUES (693134, 'Dune: Part Two', 1, 'manual', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let again = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!((again.added, again.already_wanted), (0, 2), "{again:?}");
    }

    #[tokio::test]
    async fn a_tmdb_series_list_leaves_off_a_series_taken_off() {
        let pool = setup().await;
        sqlx::query(
            "INSERT INTO wanted_removed (kind, key, title, removed_at)
             VALUES ('series', '424536', 'Frieren', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let fake = FakeTmdb::default();
        fake.reply(
            "/tv/popular?page=1",
            vec![Reply::ok(json!({ "total_pages": 1, "results": [
                { "id": 209867, "name": "Frieren: Beyond Journey's End" }
            ]}))],
        )
        .reply(
            "/tv/209867?append_to_response=external_ids",
            vec![Reply::ok(
                json!({ "id": 209867, "name": "Frieren: Beyond Journey's End",
                "seasons": [{ "season_number": 1 }], "external_ids": { "tvdb_id": 424536 } }),
            )],
        );
        let sources = sources(&fake, vec![], None).await;
        let id = create(&pool, &input("tmdb", "popular", "series"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!(result.added, 0, "{result:?}");
        assert!(why_skipped(&result, "Frieren: Beyond Journey's End").is_some());
        let wanted: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_series")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(wanted, 0);
    }

    #[tokio::test]
    async fn an_imdb_list_leaves_off_a_series_taken_off_and_keeps_one_unmonitored() {
        let pool = setup().await;
        // Frieren taken off by hand in Melete; Severance wanted, unmonitored there, with its
        // seasons as the admin left them.
        sqlx::query(
            "INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, source, added_at)
                 VALUES (1, 424536, 209867, 'Frieren', 1, 'anime', 'manual', 0),
                        (2, 371980, 95396, 'Severance', 1, 'standard', 'manual', 0);
             INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (2, 1, 0), (2, 2, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::wanted_edit::remove(&pool, crate::wanted_edit::Kind::Series, 1)
            .await
            .unwrap()
            .unwrap();
        crate::wanted_edit::set_monitored(&pool, crate::wanted_edit::Kind::Series, 2, false)
            .await
            .unwrap();
        let fake = FakeTmdb::default();
        fake.reply(
            "/find/tt22248376?external_source=imdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [{ "id": 209867, "name": "Frieren" }] }))],
        )
        .reply(
            "/find/tt11280740?external_source=imdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [{ "id": 95396, "name": "Severance" }] }))],
        )
        .reply(
            "/tv/209867?append_to_response=external_ids",
            vec![Reply::ok(json!({ "id": 209867, "name": "Frieren: Beyond Journey's End",
                "seasons": [{ "season_number": 1 }], "external_ids": { "tvdb_id": 424536 } }))],
        );
        let sources = sources(&fake, vec!["tt22248376", "tt11280740"], None).await;
        let id = create(&pool, &input("imdb", "ls0123456", "series"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!((result.added, result.already_wanted), (0, 1), "{result:?}");
        assert!(why_skipped(&result, "Frieren").is_some(), "{result:?}");
        let series: Vec<(i64, bool)> =
            sqlx::query_as("SELECT tvdb_id, monitored FROM wanted_series ORDER BY tvdb_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(series, [(371980, false)]);
        let seasons: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT season, monitored FROM wanted_seasons WHERE wanted_series_id = 2 ORDER BY season",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(seasons, [(1, false), (2, true)]);
    }

    #[tokio::test]
    async fn a_rotten_tomatoes_list_leaves_off_a_movie_taken_off() {
        let pool = setup().await;
        sqlx::query(
            "INSERT INTO wanted_removed (kind, key, title, removed_at) VALUES ('movie', '11', 'Tenzing', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        const PAGE: &str = r#"<script type="application/ld+json">{"@type":"ItemList","itemListElement":
            {"@type":"ItemList","itemListElement":[{"@type":"Movie","name":"Tenzing","dateCreated":"2026"},
            {"@type":"Movie","name":"Arrival","dateCreated":"2016"}]}}</script>"#;
        let fake = FakeTmdb::default();
        fake.reply(
            "/search/movie?include_adult=false&query=Tenzing&year=2026",
            vec![Reply::ok(json!({ "results": [{ "id": 11, "title": "Tenzing", "release_date": "2026-01-30" }] }))],
        )
        .reply(
            "/search/movie?include_adult=false&query=Arrival&year=2016",
            vec![Reply::ok(json!({ "results": [{ "id": 329865, "title": "Arrival", "release_date": "2016-11-10" }] }))],
        )
        .reply("/movie/329865", vec![movie(329865, "Arrival", "2016-11-10")]);
        let sources = sources(&fake, vec![], Some(PAGE)).await;
        let url = "https://www.rottentomatoes.com/browse/movies_at_home/sort:popular";
        let id = create(&pool, &input("rottentomatoes", url, "movies"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!(result.added, 1, "{result:?}");
        assert!(
            why_skipped(&result, "Tenzing (2026)").is_some(),
            "{result:?}"
        );
        let wanted: Vec<i64> = sqlx::query_scalar("SELECT tmdb_id FROM wanted_movies")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(wanted, [329865]);
    }

    #[tokio::test]
    async fn an_imdb_list_of_shows_is_found_through_tmdb() {
        let pool = setup().await;
        let fake = FakeTmdb::default();
        fake.reply(
            "/find/tt22248376?external_source=imdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [{ "id": 209867, "name": "Frieren" }] }))],
        )
        .reply(
            "/find/tt0000001?external_source=imdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [] }))],
        )
        .reply(
            "/find/tt0000002?external_source=imdb_id",
            vec![Reply::ok(json!({ "movie_results": [], "tv_results": [{ "id": 7, "name": "No TVDB" }] }))],
        )
        .reply(
            "/tv/209867?append_to_response=external_ids",
            vec![Reply::ok(json!({ "id": 209867, "name": "Frieren: Beyond Journey's End", "first_air_date": "2023-09-29",
                "original_language": "ja", "genres": [{ "id": 16, "name": "Animation" }],
                "seasons": [{ "season_number": 0 }, { "season_number": 1 }, { "season_number": 2 }],
                "external_ids": { "tvdb_id": 424536, "imdb_id": "tt22248376" } }))],
        )
        .reply(
            "/tv/7?append_to_response=external_ids",
            vec![Reply::ok(json!({ "id": 7, "name": "No TVDB", "seasons": [], "external_ids": {} }))],
        );
        let sources = sources(&fake, vec!["tt22248376", "tt0000001", "tt0000002"], None).await;
        let id = create(&pool, &input("imdb", "ls0123456", "series"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!(result.added, 1, "{result:?}");
        let whys: Vec<&str> = result.skipped.iter().map(|s| s.why.as_str()).collect();
        assert_eq!(
            whys,
            [
                "TMDB knows no series by this IMDb id",
                "TMDB has no TVDB id for it"
            ]
        );
        let (series_type, wanted_id): (String, i64) =
            sqlx::query_as("SELECT series_type, id FROM wanted_series WHERE tvdb_id = 424536")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(series_type, "anime");
        let seasons: Vec<(i64, bool)> =
            sqlx::query_as("SELECT season, monitored FROM wanted_seasons WHERE wanted_series_id = ? ORDER BY season")
                .bind(wanted_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            seasons,
            [(0, false), (1, false), (2, true)],
            "only the latest season"
        );

        // A list IMDb doesn't have: the error is kept as the list's last result.
        let missing = create(
            &pool,
            &ListInput {
                name: "gone".into(),
                ..input("imdb", "ls9999999", "series")
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(sync(&pool, &sources, missing).await.is_err());
        let error = get(&pool, missing)
            .await
            .unwrap()
            .unwrap()
            .last_result
            .unwrap();
        assert!(error["error"].as_str().unwrap().contains("no such list"));
    }

    #[tokio::test]
    async fn rotten_tomatoes_titles_are_matched_strictly() {
        let pool = setup().await;
        const PAGE: &str = r#"<script type="application/ld+json">{"@type":"ItemList","itemListElement":
            {"@type":"ItemList","itemListElement":[{"@type":"Movie","name":"Tenzing","dateCreated":"2026"},
            {"@type":"Movie","name":"Sacrifice","dateCreated":"2025"},{"@type":"TVSeries","name":"Severance"}]}}</script>"#;
        let fake = FakeTmdb::default();
        fake.reply(
            "/search/movie?include_adult=false&query=Tenzing&year=2026",
            vec![Reply::ok(json!({ "results": [{ "id": 11, "title": "Tenzing", "release_date": "2026-01-30" }] }))],
        )
        .reply(
            "/search/movie?include_adult=false&query=Sacrifice&year=2025",
            vec![Reply::ok(json!({ "results": [
                { "id": 21, "title": "Sacrifice", "release_date": "2025-03-01" },
                { "id": 22, "title": "Sacrifice", "release_date": "2025-09-01" }
            ]}))],
        )
        .reply("/movie/11", vec![movie(11, "Tenzing", "2026-01-30")]);
        let sources = sources(&fake, vec![], Some(PAGE)).await;
        let url = "https://www.rottentomatoes.com/browse/movies_at_home/sort:popular";
        let id = create(&pool, &input("rottentomatoes", url, "movies"))
            .await
            .unwrap()
            .unwrap();
        let result = sync(&pool, &sources, id).await.unwrap().unwrap();
        assert_eq!(
            (result.found, result.added),
            (2, 1),
            "the series isn't for a movie list"
        );
        assert_eq!(
            result.skipped,
            [Skipped {
                title: "Sacrifice (2025)".into(),
                why: "more than one TMDB title fits".into()
            }]
        );
    }
}
