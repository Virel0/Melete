use serde_json::json;

use super::testing::FakeArr;
use super::*;
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};

fn lib(id: i64, name: &str, path: &str) -> LibraryRef {
    LibraryRef {
        id,
        name: name.into(),
        path: path.into(),
    }
}

#[test]
fn root_folders_map_to_libraries() {
    let libraries = [
        lib(1, "movies", "/media/movies"),
        lib(2, "kids", "/media/films-kids"),
    ];
    let none = HashMap::new();
    assert_eq!(
        library_for(Some("/data/movies"), &libraries, &none),
        Some(1),
        "by name"
    );
    assert_eq!(
        library_for(Some("/data/movies/"), &libraries, &none),
        Some(1)
    );
    assert_eq!(
        library_for(Some("/data/films-kids"), &libraries, &none),
        Some(2),
        "by path"
    );
    assert_eq!(
        library_for(Some("/data/other"), &libraries, &none),
        None,
        "two libraries: no guess"
    );
    assert_eq!(
        library_for(Some("/data/other"), &libraries[..1], &none),
        Some(1),
        "the only one"
    );
    let explicit = HashMap::from([("/data/stuff".to_string(), "kids".to_string())]);
    assert_eq!(
        library_for(Some("/data/stuff/"), &libraries, &explicit),
        Some(2)
    );
    assert_eq!(library_for(None, &libraries, &none), None);
}

async fn setup() -> SqlitePool {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: "/media/movies".into(),
    };
    sync_config(&pool, &[config]).await.unwrap();
    sqlx::raw_sql(
            "INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (603, 'The Matrix', 0), (1, 'Wrong', 0), (7, 'Manual', 0);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, match_status, match_tmdb_id, match_method) VALUES
                 (1, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603, 'search'),
                 (2, 1, 'Dune (2021)', 'Dune', 1, 'matched', 1, 'search'),
                 (3, 1, 'Home Video', 'Home Video', 1, 'matched', 7, 'manual'),
                 (4, 1, 'Heat (1995)', 'Heat', 1, 'ambiguous', NULL, NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
    pool
}

async fn radarr(movies: serde_json::Value) -> String {
    let fake = FakeArr::default();
    fake.reply(
        "/api/v3/system/status",
        json!({ "appName": "Radarr", "version": "5.0" }),
    )
    .reply(
        "/api/v3/qualityprofile",
        json!([{ "id": 4, "name": "HD-1080p" }]),
    )
    .reply("/api/v3/movie", movies);
    fake.start().await
}

fn movie(id: i64, title: &str, tmdb: i64, folder: &str, monitored: bool) -> serde_json::Value {
    json!({ "id": id, "title": title, "year": 2000, "tmdbId": tmdb, "imdbId": "", "monitored": monitored,
                "minimumAvailability": "released", "qualityProfileId": 4,
                "path": format!("/data/movies/{folder}"), "rootFolderPath": "/data/movies/" })
}

#[tokio::test]
async fn imports_movies_and_hands_their_ids_to_the_matcher() {
    let pool = setup().await;
    let url = radarr(json!([
        movie(10, "The Matrix", 603, "The Matrix (1999)", true),
        movie(11, "Dune", 438631, "Dune (2021)", true),
        movie(12, "Home Video", 999, "Home Video", false),
        movie(13, "Heat", 949, "Heat (1995)", true),
        movie(14, "Wanted Only", 5, "Wanted Only (2030)", true),
        movie(15, "No TMDB", 0, "x", true),
    ]))
    .await;
    let client = ArrClient::new(&url, "key".into()).unwrap();

    let report = import_radarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(
        report,
        RadarrReport {
            movies: 5,
            monitored: 4,
            on_disk: 4,
            rematching: 2,
            without_library: 0,
            removed: 0
        }
    );

    // (folder, matched TMDB id, match status, Radarr's TMDB id)
    type MatchRow = (String, Option<i64>, Option<String>, Option<i64>);
    let matches: Vec<MatchRow> = sqlx::query_as(
        "SELECT rel_path, match_tmdb_id, match_status, arr_tmdb_id FROM movies ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        matches,
        vec![
            (
                "The Matrix (1999)".into(),
                Some(603),
                Some("matched".into()),
                Some(603)
            ),
            ("Dune (2021)".into(), None, None, Some(438631)),
            (
                "Home Video".into(),
                Some(7),
                Some("matched".into()),
                Some(999)
            ),
            ("Heat (1995)".into(), None, None, Some(949)),
        ],
        "agreeing and manual matches are kept; others are redone with Radarr's id"
    );

    // (tmdb id, title, monitored, quality profile, library, adopted folder)
    type WantedRow = (i64, String, bool, Option<String>, Option<i64>, Option<i64>);
    let wanted: Vec<WantedRow> = sqlx::query_as(
            "SELECT tmdb_id, title, monitored, quality_profile, library_id, movie_id FROM wanted_movies ORDER BY tmdb_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(wanted.len(), 5);
    assert_eq!(
        wanted[0],
        (
            5,
            "Wanted Only".into(),
            true,
            Some("HD-1080p".into()),
            Some(1),
            None
        )
    );
    assert!(wanted.iter().any(|w| w.0 == 999 && !w.2 && w.5 == Some(3)));
}

#[tokio::test]
async fn importing_again_updates_rather_than_duplicates() {
    let pool = setup().await;
    let first = radarr(json!([movie(
        10,
        "The Matrix",
        603,
        "The Matrix (1999)",
        true
    )]))
    .await;
    import_radarr(
        &pool,
        &ArrClient::new(&first, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let second = radarr(json!([movie(
        10,
        "The Matrix",
        603,
        "The Matrix (1999)",
        false
    )]))
    .await;
    import_radarr(
        &pool,
        &ArrClient::new(&second, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let rows: Vec<(i64, bool)> = sqlx::query_as("SELECT tmdb_id, monitored FROM wanted_movies")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows, vec![(603, false)]);
}

#[tokio::test]
async fn refuses_the_wrong_app_and_a_bad_key() {
    let pool = setup().await;
    let fake = FakeArr::default();
    fake.reply("/api/v3/system/status", json!({ "appName": "Sonarr" }));
    let sonarr = fake.start().await;
    let err = import_radarr(
        &pool,
        &ArrClient::new(&sonarr, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("Sonarr, not Radarr"), "{err}");

    let url = radarr(json!([])).await;
    let err = import_radarr(
        &pool,
        &ArrClient::new(&url, "wrong".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap_err();
    assert!(err.is::<crate::arr::Rejected>());
    let wanted: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_movies")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(wanted, 0);
}

async fn setup_shows() -> SqlitePool {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "shows".into(),
        kind: LibraryKind::Shows,
        path: "/media/shows".into(),
    };
    sync_config(&pool, &[config]).await.unwrap();
    sqlx::raw_sql(
            "INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, fetched_at) VALUES
                 (95396, 371980, 'Severance', 0), (1, 999, 'Wrong', 0), (7, 70, 'Mine', 0);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_status, match_tmdb_id, match_method) VALUES
                 (1, 1, 'Severance (2022)', 'Severance', 1, 'matched', 95396, 'search'),
                 (2, 1, 'Breaking Bad', 'Breaking Bad', 1, 'matched', 1, 'search'),
                 (3, 1, 'Manual Show', 'Manual Show', 1, 'matched', 7, 'manual');",
        )
        .execute(&pool)
        .await
        .unwrap();
    pool
}

#[tokio::test]
async fn keeps_the_original_language() {
    let pool = setup().await;
    let mut japanese = movie(10, "Spirited Away", 129, "Spirited Away (2001)", true);
    japanese["originalLanguage"] = json!({ "id": 8, "name": "Japanese" });
    let mut brazilian = movie(11, "City of God", 598, "City of God (2002)", true);
    brazilian["originalLanguage"] = json!({ "id": 30, "name": "Portuguese (Brazil)" });
    let mut unknown = movie(12, "Mystery", 7, "Mystery", true);
    unknown["originalLanguage"] = json!({ "id": 0, "name": "Unknown" });
    let without = movie(13, "Older Radarr", 8, "Older Radarr", true);
    let url = radarr(json!([japanese, brazilian, unknown, without])).await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_radarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let languages: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT title, original_language FROM wanted_movies ORDER BY source_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let expected = [
        ("Spirited Away", Some("ja")),
        ("City of God", Some("pt-br")),
        ("Mystery", None),
        ("Older Radarr", None),
    ]
    .map(|(t, l)| (t.to_owned(), l.map(str::to_owned)));
    assert_eq!(languages, expected);
}

fn show(
    id: i64,
    title: &str,
    tvdb: i64,
    tmdb: Option<i64>,
    folder: &str,
    seasons: serde_json::Value,
) -> serde_json::Value {
    json!({ "id": id, "title": title, "year": 2020, "tvdbId": tvdb, "tmdbId": tmdb, "imdbId": null,
                "monitored": true, "seriesType": if title == "Frieren" { "anime" } else { "standard" },
                "qualityProfileId": 2, "path": format!("/tv/shows/{folder}"), "rootFolderPath": "/tv/shows",
                "seasons": seasons })
}

async fn sonarr(series: serde_json::Value, episodes: &[(i64, serde_json::Value)]) -> String {
    let fake = FakeArr::default();
    fake.reply("/api/v3/system/status", json!({ "appName": "Sonarr" }))
        .reply(
            "/api/v3/qualityprofile",
            json!([{ "id": 2, "name": "WEB-1080p" }]),
        )
        .reply("/api/v3/series", series);
    for (id, list) in episodes {
        fake.reply(&format!("/api/v3/episode?seriesId={id}"), list.clone());
    }
    fake.start().await
}

#[tokio::test]
async fn imports_series_seasons_and_unmonitored_episodes() {
    let pool = setup_shows().await;
    let seasons = json!([{ "seasonNumber": 0, "monitored": false }, { "seasonNumber": 1, "monitored": true }]);
    let episodes = json!([
        { "seasonNumber": 0, "episodeNumber": 1, "monitored": false },
        { "seasonNumber": 1, "episodeNumber": 1, "monitored": true },
        { "seasonNumber": 1, "episodeNumber": 2, "monitored": false },
    ]);
    let url = sonarr(
        json!([
            show(
                1,
                "Severance",
                371980,
                None,
                "Severance (2022)",
                seasons.clone()
            ),
            show(
                2,
                "Breaking Bad",
                81189,
                Some(1396),
                "Breaking Bad",
                json!([])
            ),
            show(3, "Manual Show", 12345, None, "Manual Show", json!([])),
            show(4, "Frieren", 424536, Some(209867), "Frieren", json!([])),
        ]),
        &[
            (1, episodes),
            (2, json!([])),
            (3, json!([])),
            (4, json!([])),
        ],
    )
    .await;
    let report = import_sonarr(
        &pool,
        &ArrClient::new(&url, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        report,
        SonarrReport {
            series: 4,
            monitored: 4,
            on_disk: 3,
            rematching: 1,
            without_library: 0,
            unmonitored_episodes: 1,
            removed: 0
        }
    );

    // (folder, matched TMDB id, Sonarr's TVDB id, Sonarr's TMDB id)
    type SeriesRow = (String, Option<i64>, Option<i64>, Option<i64>);
    let rows: Vec<SeriesRow> = sqlx::query_as(
        "SELECT rel_path, match_tmdb_id, arr_tvdb_id, arr_tmdb_id FROM series ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("Severance (2022)".into(), Some(95396), Some(371980), None),
            ("Breaking Bad".into(), None, Some(81189), Some(1396)),
            ("Manual Show".into(), Some(7), Some(12345), None),
        ],
        "agreeing by TVDB id and manual matches are kept"
    );

    let types: Vec<(String, String)> =
        sqlx::query_as("SELECT title, series_type FROM wanted_series ORDER BY title")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(types.contains(&("Frieren".into(), "anime".into())));
    let seasons: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT s.season, s.monitored FROM wanted_seasons s JOIN wanted_series w ON w.id = s.wanted_series_id
             WHERE w.tvdb_id = 371980 ORDER BY season",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(seasons, vec![(0, false), (1, true)]);
    let overrides: Vec<(i64, i64)> =
        sqlx::query_as("SELECT season, episode FROM wanted_episode_overrides")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        overrides,
        vec![(1, 2)],
        "only episodes left out of monitored seasons"
    );
}

#[tokio::test]
async fn importing_series_again_replaces_their_seasons() {
    let pool = setup_shows().await;
    let first = sonarr(
        json!([show(
            1,
            "Severance",
            371980,
            None,
            "Severance (2022)",
            json!([{ "seasonNumber": 1, "monitored": true }])
        )]),
        &[(
            1,
            json!([{ "seasonNumber": 1, "episodeNumber": 3, "monitored": false }]),
        )],
    )
    .await;
    import_sonarr(
        &pool,
        &ArrClient::new(&first, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let second = sonarr(
        json!([show(
            1,
            "Severance",
            371980,
            None,
            "Severance (2022)",
            json!([{ "seasonNumber": 1, "monitored": false }])
        )]),
        &[(1, json!([]))],
    )
    .await;
    import_sonarr(
        &pool,
        &ArrClient::new(&second, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let seasons: Vec<(i64, bool)> = sqlx::query_as("SELECT season, monitored FROM wanted_seasons")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(seasons, vec![(1, false)]);
    let overrides: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_episode_overrides")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(overrides, 0);
    let series: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(series, 1);
}

#[tokio::test]
async fn a_failed_episode_fetch_writes_nothing() {
    let pool = setup_shows().await;
    // No episode list for series 2: Sonarr answers 404.
    let url = sonarr(
        json!([
            show(1, "Severance", 371980, None, "Severance (2022)", json!([])),
            show(2, "Other", 5, None, "Other", json!([]))
        ]),
        &[(1, json!([]))],
    )
    .await;
    assert!(
        import_sonarr(
            &pool,
            &ArrClient::new(&url, "key".into()).unwrap(),
            &HashMap::new()
        )
        .await
        .is_err()
    );
    let series: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(series, 0);
    let arr_ids: i64 = sqlx::query_scalar("SELECT count(arr_tvdb_id) FROM series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(arr_ids, 0);
}

#[tokio::test]
async fn removed_from_radarr_means_no_longer_wanted() {
    let pool = setup().await;
    sqlx::query(
            "INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at) VALUES (77, 'Mine', 1, 'manual', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let both = radarr(json!([
        movie(10, "A", 603, "A", true),
        movie(11, "B", 5, "B", true)
    ]))
    .await;
    import_radarr(
        &pool,
        &ArrClient::new(&both, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let one = radarr(json!([movie(10, "A", 603, "A", true)])).await;
    let report = import_radarr(
        &pool,
        &ArrClient::new(&one, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.removed, 1);
    let left: Vec<i64> = sqlx::query_scalar("SELECT tmdb_id FROM wanted_movies ORDER BY tmdb_id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![77, 603], "entries added by hand stay");

    let empty = radarr(json!([])).await;
    let err = import_radarr(
        &pool,
        &ArrClient::new(&empty, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("refusing to remove all 1"),
        "{err}"
    );
}

#[tokio::test]
async fn imports_never_overwrite_entries_added_by_hand() {
    let pool = setup().await;
    sqlx::query(
        "INSERT INTO wanted_movies (tmdb_id, title, monitored, quality_profile, source, added_at)
             VALUES (603, 'My Matrix', 0, 'Mine', 'manual', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let url = radarr(json!([movie(
        10,
        "The Matrix",
        603,
        "The Matrix (1999)",
        true
    )]))
    .await;
    import_radarr(
        &pool,
        &ArrClient::new(&url, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let row: (String, bool, String, String) = sqlx::query_as(
        "SELECT title, monitored, quality_profile, source FROM wanted_movies WHERE tmdb_id = 603",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        ("My Matrix".into(), false, "Mine".into(), "manual".into())
    );
}

#[tokio::test]
async fn folders_link_only_in_their_own_library() {
    let pool = db::open_in_memory().await;
    let configs = [
        LibraryConfig {
            name: "uk".into(),
            kind: LibraryKind::Shows,
            path: "/media/uk".into(),
        },
        LibraryConfig {
            name: "us".into(),
            kind: LibraryKind::Shows,
            path: "/media/us".into(),
        },
    ];
    sync_config(&pool, &configs).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES
                 (1, 1, 'Shameless', 'Shameless', 1), (2, 2, 'Shameless', 'Shameless', 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let shameless = |root: &str| {
        json!([{ "id": 1, "title": "Shameless (US)", "tvdbId": 161511, "monitored": true,
                     "path": format!("{root}/Shameless"), "rootFolderPath": root, "seasons": [] }])
    };
    // An unmapped root folder and a name both libraries have: no link.
    let url = sonarr(shameless("/tv"), &[(1, json!([]))]).await;
    let report = import_sonarr(
        &pool,
        &ArrClient::new(&url, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(report.on_disk, 0);
    // Mapped to the US library: the US folder.
    let url = sonarr(shameless("/data/us"), &[(1, json!([]))]).await;
    import_sonarr(
        &pool,
        &ArrClient::new(&url, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let ids: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, arr_tvdb_id FROM series ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids, vec![(1, None), (2, Some(161511))]);
}

#[tokio::test]
async fn duplicate_season_numbers_from_sonarr_are_tolerated() {
    let pool = setup_shows().await;
    let seasons = json!([{ "seasonNumber": 1, "monitored": true }, { "seasonNumber": 1, "monitored": false }]);
    let url = sonarr(
        json!([show(
            1,
            "Severance",
            371980,
            None,
            "Severance (2022)",
            seasons
        )]),
        &[(1, json!([]))],
    )
    .await;
    import_sonarr(
        &pool,
        &ArrClient::new(&url, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
    let seasons: Vec<(i64, bool)> = sqlx::query_as("SELECT season, monitored FROM wanted_seasons")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(seasons, vec![(1, false)]);
}

#[tokio::test]
async fn redirects_are_not_followed_so_the_key_stays_home() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Another host that would receive the key if the redirect were followed.
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let elsewhere = axum::Router::new().fallback(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        async { "{}" }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let elsewhere_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, elsewhere).await.unwrap() });
    let target = elsewhere_url.clone();
    let proxy = axum::Router::new().fallback(move || {
        let target = target.clone();
        async move { axum::response::Redirect::to(&format!("{target}/login")) }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, proxy).await.unwrap() });

    let err = ArrClient::new(&proxy_url, "key".into())
        .unwrap()
        .system_status()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("redirected"), "{err}");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the other host was never contacted"
    );
}

#[test]
fn unknown_series_types_count_as_standard() {
    assert_eq!(series_type(Some("anime")), "anime");
    assert_eq!(series_type(Some("daily")), "daily");
    assert_eq!(series_type(Some("weird")), "standard");
    assert_eq!(series_type(None), "standard");
}

#[test]
fn client_urls_are_checked() {
    assert!(ArrClient::new("http://192.168.1.10:7878", "k".into()).is_ok());
    for bad in ["192.168.1.10:7878", "ftp://x", "http://", "not a url"] {
        assert!(ArrClient::new(bad, "k".into()).is_err(), "{bad}");
    }
    assert!(ArrClient::new("http://x", " ".into()).is_err());
}

fn lidarr(artists: serde_json::Value, albums: serde_json::Value) -> FakeArr {
    let fake = FakeArr::default();
    fake.reply("/api/v1/system/status", json!({ "appName": "Lidarr" }))
        .reply(
            "/api/v1/qualityprofile",
            json!([{ "id": 1, "name": "Lossless" }]),
        )
        .reply("/api/v1/artist", artists)
        .reply("/api/v1/album", albums);
    fake
}

#[tokio::test]
async fn radarr_and_sonarr_imports_keep_what_an_admin_unmonitored_or_removed() {
    let pool = setup().await;
    let movies = json!([
        movie(10, "The Matrix", 603, "The Matrix (1999)", true),
        movie(11, "Dune", 438631, "Dune (2021)", true),
        movie(12, "Heat", 949, "Heat (1995)", false),
    ]);
    let url = radarr(movies.clone()).await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_radarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let id = |tmdb: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT id FROM wanted_movies WHERE tmdb_id = ?")
                .bind(tmdb)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    // The admin stops monitoring The Matrix, monitors Heat, and takes Dune off the list.
    wanted_edit::set_monitored(&pool, Kind::Movie, id(603).await, false)
        .await
        .unwrap();
    wanted_edit::set_monitored(&pool, Kind::Movie, id(949).await, true)
        .await
        .unwrap();
    wanted_edit::remove(&pool, Kind::Movie, id(438631).await)
        .await
        .unwrap()
        .unwrap();
    let url = radarr(movies).await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_radarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let rows: Vec<(i64, bool)> =
        sqlx::query_as("SELECT tmdb_id, monitored FROM wanted_movies ORDER BY tmdb_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![(603, false), (949, true)],
        "as the admin left them"
    );

    // Back on the list, Dune is updated by the import as before.
    sqlx::query(
        "INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at)
             VALUES (438631, 'Dune', 0, 'radarr', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let url = radarr(json!([movie(11, "Dune", 438631, "Dune (2021)", true)])).await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_radarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let dune: bool =
        sqlx::query_scalar("SELECT monitored FROM wanted_movies WHERE tmdb_id = 438631")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(dune);

    // Sonarr's path: a series taken off stays off; one unmonitored stays unmonitored.
    let pool = setup_shows().await;
    let shows = json!([
        show(
            1,
            "Severance",
            371980,
            None,
            "Severance (2022)",
            json!([{ "seasonNumber": 1, "monitored": true }])
        ),
        show(
            2,
            "Andor",
            393189,
            None,
            "Andor (2022)",
            json!([{ "seasonNumber": 1, "monitored": true }])
        ),
    ]);
    let episodes = [(1, json!([])), (2, json!([]))];
    let url = sonarr(shows.clone(), &episodes).await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_sonarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let series = |tvdb: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT id FROM wanted_series WHERE tvdb_id = ?")
                .bind(tvdb)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    wanted_edit::set_monitored(&pool, Kind::Series, series(371980).await, false)
        .await
        .unwrap();
    wanted_edit::remove(&pool, Kind::Series, series(393189).await)
        .await
        .unwrap()
        .unwrap();
    let url = sonarr(shows, &episodes).await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_sonarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let rows: Vec<(i64, bool)> =
        sqlx::query_as("SELECT tvdb_id, monitored FROM wanted_series ORDER BY tvdb_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![(371980, false)]);
}

#[tokio::test]
async fn lidarr_imports_keep_what_an_admin_unmonitored_or_removed() {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "music".into(),
        kind: LibraryKind::Music,
        path: "/media/music".into(),
    };
    sync_config(&pool, &[config]).await.unwrap();
    let artists = json!([
        { "id": 10, "artistName": "Alder", "foreignArtistId": "mb-a1", "monitored": true, "rootFolderPath": "/music" },
        { "id": 11, "artistName": "Birch", "foreignArtistId": "mb-a2", "monitored": true, "rootFolderPath": "/music" },
    ]);
    let albums = json!([
        { "id": 100, "title": "One", "foreignAlbumId": "rg-1", "artistId": 10, "monitored": true },
        { "id": 101, "title": "Two", "foreignAlbumId": "rg-2", "artistId": 10, "monitored": true },
        { "id": 102, "title": "Bark", "foreignAlbumId": "rg-3", "artistId": 11, "monitored": true },
    ]);
    let import = || {
        let (pool, artists, albums) = (pool.clone(), artists.clone(), albums.clone());
        async move {
            let url = lidarr(artists, albums).start().await;
            let client = ArrClient::new(&url, "key".into()).unwrap();
            import_lidarr(&pool, &client, &HashMap::new())
                .await
                .unwrap()
        }
    };
    import().await;
    let id = |table: &'static str, column: &'static str, key: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT id FROM {table} WHERE {column} = ?"
            )))
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // Alder unmonitored, Alder's "Two" taken off, and Birch taken off with "Bark".
    wanted_edit::set_monitored(
        &pool,
        Kind::Artist,
        id("wanted_artists", "mb_artist_id", "mb-a1").await,
        false,
    )
    .await
    .unwrap();
    wanted_edit::remove(
        &pool,
        Kind::Album,
        id("wanted_albums", "mb_release_group_id", "rg-2").await,
    )
    .await
    .unwrap()
    .unwrap();
    wanted_edit::remove(
        &pool,
        Kind::Artist,
        id("wanted_artists", "mb_artist_id", "mb-a2").await,
    )
    .await
    .unwrap()
    .unwrap();
    import().await;
    let artists: Vec<(String, bool)> =
        sqlx::query_as("SELECT mb_artist_id, monitored FROM wanted_artists ORDER BY 1")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(artists, vec![("mb-a1".into(), false)]);
    let albums: Vec<String> =
        sqlx::query_scalar("SELECT mb_release_group_id FROM wanted_albums ORDER BY 1")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(albums, vec!["rg-1"]);
}

#[tokio::test]
async fn imports_lidarrs_artists_and_albums() {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "music".into(),
        kind: LibraryKind::Music,
        path: "/media/music".into(),
    };
    sync_config(&pool, &[config]).await.unwrap();
    // One album already on disk, matched to its release group; one wanted by hand.
    sqlx::raw_sql(
            "INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation,
                 match_release_group_id) VALUES (1, 'k', 'One', 'Alder', 'a', 1, 'rg-1');
             INSERT INTO wanted_artists (mb_artist_id, name, monitored, monitor_new, source, added_at)
                 VALUES ('mb-hand', 'Chosen By Hand', 1, 'all', 'manual', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
    let artists = json!([
        { "id": 10, "artistName": "Alder", "foreignArtistId": "mb-a1", "monitored": true,
          "qualityProfileId": 1, "rootFolderPath": "/music", "monitorNewItems": "all" },
        { "id": 11, "artistName": "Birch", "foreignArtistId": "mb-a2", "monitored": false,
          "rootFolderPath": "/music", "monitorNewItems": "none" },
        { "id": 12, "artistName": "Renamed In Lidarr", "foreignArtistId": "mb-hand", "monitored": false,
          "rootFolderPath": "/music" },
    ]);
    let albums = json!([
        { "id": 100, "title": "One", "foreignAlbumId": "rg-1", "artistId": 10, "monitored": true,
          "releaseDate": "2016-04-29T00:00:00Z", "albumType": "Album", "secondaryTypes": [] },
        { "id": 101, "title": "Live", "foreignAlbumId": "rg-2", "artistId": 10, "monitored": false,
          "releaseDate": "0001-01-01T00:00:00Z", "albumType": "Album",
          "secondaryTypes": [{ "id": 6, "name": "Live" }] },
        { "id": 102, "title": "Nobody's", "foreignAlbumId": "rg-3", "artistId": 99, "monitored": true },
    ]);
    let url = lidarr(artists.clone(), albums).start().await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    let report = import_lidarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    assert_eq!(
        report,
        LidarrReport {
            artists: 3,
            monitored_artists: 1,
            albums: 2,
            monitored_albums: 1,
            on_disk: 1,
            without_library: 0,
            removed_artists: 0,
            removed_albums: 0,
        }
    );
    type Artist = (
        String,
        String,
        bool,
        String,
        Option<String>,
        Option<i64>,
        String,
    );
    let rows: Vec<Artist> = sqlx::query_as(
        "SELECT mb_artist_id, name, monitored, monitor_new, quality_profile, library_id, source
             FROM wanted_artists ORDER BY mb_artist_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows[0],
        (
            "mb-a1".into(),
            "Alder".into(),
            true,
            "all".into(),
            Some("Lossless".into()),
            Some(1),
            "lidarr".into()
        )
    );
    assert_eq!(rows[1].3, "none");
    assert_eq!(
        rows[2].1, "Chosen By Hand",
        "an entry added another way is left alone"
    );
    let albums: Vec<(String, String, Option<String>, String, bool)> = sqlx::query_as(
        "SELECT mb_release_group_id, title, release_date, secondary_types, monitored
             FROM wanted_albums ORDER BY mb_release_group_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        albums[0],
        (
            "rg-1".into(),
            "One".into(),
            Some("2016-04-29".into()),
            "[]".into(),
            true
        )
    );
    assert_eq!(albums[1].2, None, "a placeholder date is no date");
    assert_eq!(albums[1].3, r#"["Live"]"#);

    // Birch and the live album leave Lidarr; the hand-added artist stays either way.
    let fewer = json!([artists[0].clone()]);
    let one_album = json!([{ "id": 100, "title": "One (2016)", "foreignAlbumId": "rg-1", "artistId": 10, "monitored": true }]);
    let url = lidarr(fewer, one_album).start().await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    let report = import_lidarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    assert_eq!((report.removed_artists, report.removed_albums), (1, 1));
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM wanted_artists ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, ["Alder", "Chosen By Hand"]);
    let title: String = sqlx::query_scalar("SELECT title FROM wanted_albums")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "One (2016)");

    // A Lidarr that lists nothing doesn't empty the list.
    let url = lidarr(json!([]), json!([])).start().await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    let err = import_lidarr(&pool, &client, &HashMap::new())
        .await
        .unwrap_err();
    assert!(err.is::<RefusedToEmpty>(), "{err:#}");
}

#[tokio::test]
async fn a_lidarr_listing_nothing_removes_nothing() {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "music".into(),
        kind: LibraryKind::Music,
        path: "/media/music".into(),
    };
    sync_config(&pool, &[config]).await.unwrap();
    sqlx::query(
        "INSERT INTO wanted_artists (mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES ('mb-hand', 'Chosen By Hand', 1, 'all', 'manual', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let import = |artists: serde_json::Value, albums: serde_json::Value| {
        let pool = pool.clone();
        async move {
            let url = lidarr(artists, albums).start().await;
            let client = ArrClient::new(&url, "key".into()).unwrap();
            import_lidarr(&pool, &client, &HashMap::new()).await
        }
    };
    let wanted = || {
        let pool = pool.clone();
        async move {
            let artists: Vec<String> =
                sqlx::query_scalar("SELECT mb_artist_id FROM wanted_artists ORDER BY 1")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            let albums: Vec<String> =
                sqlx::query_scalar("SELECT mb_release_group_id FROM wanted_albums ORDER BY 1")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            (artists, albums)
        }
    };

    // Nothing imported from it yet: an empty Lidarr is an empty import, not a refusal.
    let report = import(json!([]), json!([])).await.unwrap();
    assert_eq!(report, LidarrReport::default());
    assert_eq!(wanted().await, (vec!["mb-hand".to_owned()], vec![]));

    let artists = json!([
        { "id": 10, "artistName": "Alder", "foreignArtistId": "mb-a1", "monitored": true, "rootFolderPath": "/music" },
        { "id": 11, "artistName": "Birch", "foreignArtistId": "mb-a2", "monitored": true, "rootFolderPath": "/music" },
    ]);
    let albums = json!([
        { "id": 100, "title": "One", "foreignAlbumId": "rg-1", "artistId": 10, "monitored": true },
        { "id": 101, "title": "Bark", "foreignAlbumId": "rg-2", "artistId": 11, "monitored": true },
    ]);
    import(artists, albums).await.unwrap();
    let before = wanted().await;
    assert_eq!(
        before,
        (
            vec!["mb-a1".to_owned(), "mb-a2".into(), "mb-hand".into()],
            vec!["rg-1".to_owned(), "rg-2".into()]
        )
    );

    // Then it lists nothing: refused, with the reason, and every entry stays.
    let err = import(json!([]), json!([])).await.unwrap_err();
    assert!(err.is::<RefusedToEmpty>(), "{err:#}");
    assert_eq!(
        err.to_string(),
        "lidarr listed nothing; refusing to remove all 2 artists imported from it (check the URL)"
    );
    assert_eq!(wanted().await, before);
}

/// Imports Severance from a Sonarr whose season 1 and episode S01E02 are monitored or not.
async fn import_severance(pool: &SqlitePool, season: bool, episode: bool) {
    let url = sonarr(
        json!([show(
            1,
            "Severance",
            371980,
            None,
            "Severance (2022)",
            json!([{ "seasonNumber": 1, "monitored": season }])
        )]),
        &[(
            1,
            json!([{ "seasonNumber": 1, "episodeNumber": 2, "monitored": episode }]),
        )],
    )
    .await;
    import_sonarr(
        pool,
        &ArrClient::new(&url, "key".into()).unwrap(),
        &HashMap::new(),
    )
    .await
    .unwrap();
}

/// Season 1's monitoring, and S01E02's override when there is one.
async fn severance_monitoring(pool: &SqlitePool) -> (bool, Option<bool>) {
    let season: bool = sqlx::query_scalar("SELECT monitored FROM wanted_seasons WHERE season = 1")
        .fetch_one(pool)
        .await
        .unwrap();
    let episode: Option<bool> = sqlx::query_scalar(
        "SELECT monitored FROM wanted_episode_overrides WHERE season = 1 AND episode = 2",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    (season, episode)
}

#[tokio::test]
async fn imports_keep_season_and_episode_choices_until_sonarr_changes_them() {
    let pool = setup_shows().await;
    import_severance(&pool, true, true).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM wanted_series")
        .fetch_one(&pool)
        .await
        .unwrap();
    // Leave S01E02 out, then stop monitoring season 1, and pick season 3, which Sonarr doesn't
    // list, in Melete.
    let left_out = wanted_edit::set_episode_monitored(&pool, id, 1, 2, false).await;
    assert_eq!(left_out.unwrap(), Ok(()));
    assert!(
        wanted_edit::set_season_monitored(&pool, id, 1, false)
            .await
            .unwrap()
    );
    assert!(
        wanted_edit::set_season_monitored(&pool, id, 3, true)
            .await
            .unwrap()
    );

    // Sonarr says what it said before: Melete's choices stand.
    import_severance(&pool, true, true).await;
    assert_eq!(severance_monitoring(&pool).await, (false, Some(false)));
    let third: bool = sqlx::query_scalar("SELECT monitored FROM wanted_seasons WHERE season = 3")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        third,
        "a season set in Melete stays though Sonarr doesn't list it"
    );

    // Sonarr changes both, to what Melete chose: its change is the later one, so from now on
    // Sonarr's values are followed again.
    import_severance(&pool, false, false).await;
    assert_eq!(severance_monitoring(&pool).await, (false, None));
    import_severance(&pool, true, true).await;
    assert_eq!(severance_monitoring(&pool).await, (true, None));
}

#[tokio::test]
async fn a_lidarr_import_keeps_the_rule_for_new_albums_set_in_melete() {
    let pool = db::open_in_memory().await;
    let artists = json!([{ "id": 1, "artistName": "Radiohead",
        "foreignArtistId": "a74b1b7f-71a5-4011-9441-d0b5e4122711", "monitored": true,
        "monitorNewItems": "all", "qualityProfileId": 1, "rootFolderPath": "/music",
        "path": "/music/Radiohead" }]);
    let url = lidarr(artists.clone(), json!([])).start().await;
    let client = ArrClient::new(&url, "key".into()).unwrap();
    import_lidarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM wanted_artists")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        wanted_edit::set_monitor_new(&pool, id, "none")
            .await
            .unwrap()
    );
    import_lidarr(&pool, &client, &HashMap::new())
        .await
        .unwrap();
    let rule: String = sqlx::query_scalar("SELECT monitor_new FROM wanted_artists")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rule, "none");
}
