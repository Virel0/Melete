use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, RawQuery, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use super::*;
use crate::db;
use crate::profiles::import::tests::converted;

#[test]
fn titles_compare_without_case_punctuation_or_articles() {
    assert_eq!(comparable("Dune: Part Two"), comparable("Dune Part Two"));
    assert_eq!(comparable("Charlotte's Web"), comparable("Charlottes Web"));
    assert_eq!(
        comparable("The Lord of the Rings"),
        comparable("Lord of the Rings")
    );
    assert_eq!(comparable("Fast & Furious"), comparable("Fast and Furious"));
    assert_ne!(comparable("Dune"), comparable("Dune Part Two"));
    // Only a leading article goes: "Plan A" isn't "Plan".
    assert_ne!(comparable("Plan A"), comparable("Plan"));
    assert_eq!(comparable("The"), "the");
    assert_eq!(comparable("Amélie"), comparable("Amelie"));
    assert_eq!(
        comparable("Léon: The Professional"),
        comparable("Leon The Professional")
    );
    assert_eq!(
        comparable("Pokémon Detective Pikachu"),
        comparable("Pokemon.Detective.Pikachu")
    );
    assert_eq!(without_year("Doctor Who (2005)"), "Doctor Who");
    assert_eq!(without_year("Shameless (US)"), "Shameless (US)");
}

fn movie_wanted(title: &str, year: Option<i64>) -> Wanted {
    Wanted {
        titles: vec![title.into()],
        year,
        ..Wanted::default()
    }
}

#[test]
fn remakes_and_unknown_years_are_refused() {
    let check =
        |wanted: &Wanted, name: &str| mismatch(&release::parse(name), wanted, Target::Movie(1));
    let little_women = movie_wanted("Little Women", Some(2019));
    assert_eq!(
        check(
            &little_women,
            "Little.Women.2018.1080p.WEB-DL.DD5.1.H264-GRP"
        )
        .as_deref(),
        Some("it's from 2018, not 2019")
    );
    assert_eq!(
        check(
            &little_women,
            "Little.Women.2019.1080p.WEB-DL.DD5.1.H264-GRP"
        ),
        None
    );
    let tba = movie_wanted("Dune", None);
    assert!(check(&tba, "Dune.1984.1080p.BluRay.x264-GRP").is_some());
    assert!(
        check(
            &movie_wanted("Plan A", Some(2021)),
            "Plan.2021.1080p.WEB-DL.H264-GRP"
        )
        .is_some()
    );
    assert_eq!(
        check(
            &movie_wanted("Amélie", Some(2001)),
            "Amelie.2001.1080p.BluRay.x264-GRP"
        ),
        None
    );
    let doctor_who = Wanted {
        titles: vec!["Doctor Who (2005)".into()],
        year: Some(2005),
        ..Wanted::default()
    };
    let episode = Target::Series {
        id: 1,
        season: 1,
        episode: Some(5),
    };
    assert_eq!(
        mismatch(
            &release::parse("Doctor.Who.2005.S01E05.1080p.BluRay.x264-GRP"),
            &doctor_who,
            episode
        ),
        None
    );
    assert!(
        mismatch(
            &release::parse("Doctor.Who.1963.S01E05.DVDRip.x264-GRP"),
            &doctor_who,
            episode
        )
        .is_some()
    );
}

fn indexer(movie: &[&str], tv: &[&str]) -> IndexerRow {
    IndexerRow {
        id: 1,
        name: "i".into(),
        url: String::new(),
        api_key: String::new(),
        categories: vec![2000, 2040, 5000, 5040, 8000],
        kind: torznab::Kind::Torznab,
        movie_params: movie.iter().map(|p| p.to_string()).collect(),
        tv_params: tv.iter().map(|p| p.to_string()).collect(),
    }
}

#[test]
fn queries_by_id_where_the_indexer_takes_one() {
    let wanted = Wanted {
        titles: vec!["Dune: Part Two".into()],
        year: Some(2024),
        imdb_id: Some("tt15239678".into()),
        tmdb_id: Some(693134),
        tvdb_id: Some(424536),
        ..Wanted::default()
    };
    let movie = Target::Movie(1);
    let query = |i: &IndexerRow, t| params(i, &wanted, t).unwrap();
    assert_eq!(
        query(&indexer(&["q", "imdbid"], &[]), movie),
        [
            ("cat", "2000,2040".into()),
            ("t", "movie".into()),
            ("imdbid", "15239678".into())
        ]
    );
    assert_eq!(
        query(&indexer(&["q", "tmdbid"], &[]), movie)[2],
        ("tmdbid", "693134".into())
    );
    assert_eq!(
        query(&indexer(&[], &[]), movie)[1..],
        [("t", "search".into()), ("q", "Dune: Part Two 2024".into())]
    );
    let episode = Target::Series {
        id: 1,
        season: 1,
        episode: Some(5),
    };
    assert_eq!(
        query(&indexer(&[], &["q", "season", "ep", "tvdbid"]), episode),
        [
            ("cat", "5000,5040".into()),
            ("t", "tvsearch".into()),
            ("tvdbid", "424536".into()),
            ("season", "1".into()),
            ("ep", "5".into())
        ]
    );
    assert_eq!(
        query(&indexer(&[], &["q", "season"]), episode)[1..],
        [
            ("t", "search".into()),
            ("q", "Dune: Part Two S01E05".into())
        ],
        "without ep, the numbers go in the text"
    );
    let season = Target::Series {
        id: 1,
        season: 2,
        episode: None,
    };
    assert_eq!(
        query(&indexer(&[], &["q", "season"]), season)[1..3],
        [("t", "tvsearch".into()), ("q", "Dune: Part Two".into())]
    );
    let mut no_movies = indexer(&["q"], &["q"]);
    no_movies.categories = vec![5000];
    assert_eq!(
        params(&no_movies, &wanted, movie).unwrap_err(),
        "it has no movie categories"
    );
}

/// Fake indexers at /{n}/api: n=1 answers `feed`, n=2 refuses the key. Requests are logged.
async fn indexers(pool: &SqlitePool, feed: String) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let state = (log.clone(), feed);
    let router = axum::Router::new()
        .route(
            "/{n}/api",
            axum::routing::get(
                |UrlPath(n): UrlPath<i64>,
                 RawQuery(q): RawQuery,
                 State((log, feed)): State<(Arc<Mutex<Vec<String>>>, String)>| async move {
                    log.lock()
                        .unwrap()
                        .push(format!("{n}?{}", q.unwrap_or_default()));
                    match n {
                        1 => feed.into_response(),
                        _ => StatusCode::UNAUTHORIZED.into_response(),
                    }
                },
            ),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let all = r#"["q","imdbid","tmdbid"]"#;
    let tv = r#"["q","season","ep","tvdbid"]"#;
    for (n, name, enabled, categories) in [
        (1, "Good", 1, "[2000,2040,2045,5000,5040]"),
        (2, "Refusing", 1, "[2000,5000]"),
        (3, "Only TV", 1, "[5000]"),
        (4, "Off", 0, "[2000,5000]"),
    ] {
        sqlx::query(
            "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                     tv_search_params, source, source_id, added_at)
                 VALUES (?, ?, 'secret-key', ?, ?, ?, ?, 'prowlarr', ?, 0)",
        )
        .bind(name)
        .bind(format!("{base}/{n}/api"))
        .bind(enabled)
        .bind(categories)
        .bind(all)
        .bind(tv)
        .bind(n)
        .execute(pool)
        .await
        .unwrap();
    }
    log
}

fn feed(titles: &[&str]) -> String {
    let items: String = titles
            .iter()
            .enumerate()
            .map(|(i, title)| {
                format!(
                    r#"<item><title>{title}</title><guid>https://tracker.example/t/{i}?passkey=pk</guid>
                    <link>http://prowlarr.example/1/download?apikey=secret-key&amp;id={i}</link><size>{}</size>
                    <torznab:attr name="seeders" value="{}"/></item>"#,
                    (i + 1) * 1_000_000_000,
                    100 - i
                )
            })
            .collect();
    format!(
        r#"<rss xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel>{items}</channel></rss>"#
    )
}

async fn setup() -> SqlitePool {
    let pool = db::open_in_memory().await;
    profiles::save(&pool, App::Radarr, &converted(App::Radarr))
        .await
        .unwrap();
    profiles::save(&pool, App::Sonarr, &converted(App::Sonarr))
        .await
        .unwrap();
    pool
}

async fn want_dune(pool: &SqlitePool, profile: &str) -> i64 {
    sqlx::query_scalar(
            "INSERT INTO wanted_movies (tmdb_id, imdb_id, title, year, monitored, quality_profile, source,
                 added_at, original_language)
             VALUES (693134, 'tt15239678', 'Dune: Part Two', 2024, 1, ?, 'radarr', 0, 'en') RETURNING id",
        )
        .bind(profile)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn summary(view: &SearchView) -> Vec<(String, bool, Vec<String>)> {
    view.releases
        .iter()
        .map(|r| {
            let rejections = r.decision["rejections"].as_array().unwrap();
            let first = rejections
                .iter()
                .take(1)
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            (r.title.clone(), r.accepted, first)
        })
        .collect()
}

#[tokio::test]
async fn checks_titles_decides_and_keeps_every_result() {
    let pool = setup().await;
    let id = want_dune(&pool, "4K Preferred").await;
    let log = indexers(
        &pool,
        feed(&[
            "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX",
            "Dune.2021.2160p.WEB-DL.DDP5.1.DV.H.265-GRP",
            "Dune.Part.Two.2019.1080p.BluRay.x264-GRP",
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
            "Dune.Part.Two.2024.S01E01.1080p.WEB.H264-GRP",
            "Dune.Part.Two.2024.720p.WEB-DL.H.264-GRP",
            "Dune.Part.Two.2024.FRENCH.1080p.WEB.H264-GRP",
        ]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let search_id = search(&pool, &client, None, Target::Movie(id))
        .await
        .unwrap()
        .unwrap();
    let view = get(&pool, search_id).await.unwrap().unwrap();
    let s = |t: &str| t.to_owned();
    // Accepted first; the rest by quality rank too.
    assert_eq!(
        summary(&view),
        [
            (
                s(
                    "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR"
                ),
                true,
                vec![]
            ),
            (
                s("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX"),
                true,
                vec![]
            ),
            (
                s("Dune.2021.2160p.WEB-DL.DDP5.1.DV.H.265-GRP"),
                false,
                vec![s("it's \"Dune\", not \"Dune: Part Two\"")]
            ),
            (
                s("Dune.Part.Two.2019.1080p.BluRay.x264-GRP"),
                false,
                vec![s("it's from 2019, not 2024")]
            ),
            (
                s("Dune.Part.Two.2024.S01E01.1080p.WEB.H264-GRP"),
                false,
                vec![s("it's a TV release")]
            ),
            (
                s("Dune.Part.Two.2024.FRENCH.1080p.WEB.H264-GRP"),
                false,
                vec![s("the audio is fr; 4K Preferred wants en")]
            ),
            (
                s("Dune.Part.Two.2024.720p.WEB-DL.H.264-GRP"),
                false,
                vec![s("WEBDL-720p isn't allowed by 4K Preferred")]
            ),
        ]
    );
    assert_eq!(view.releases[0].decision["score"], 150 + 100 + 75);
    assert_eq!(
        view.indexers,
        [
            IndexerOutcome {
                indexer_id: 1,
                name: "Good".into(),
                results: 7,
                error: None
            },
            IndexerOutcome {
                indexer_id: 3,
                name: "Only TV".into(),
                results: 0,
                error: Some("it has no movie categories".into())
            },
            IndexerOutcome {
                indexer_id: 2,
                name: "Refusing".into(),
                results: 0,
                error: Some("the indexer rejected the API key".into())
            },
        ]
    );
    // Asked by IMDb id in movie categories; the turned-off and TV-only ones weren't asked.
    let mut asked = log.lock().unwrap().clone();
    asked.sort();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(
        asked[0].starts_with("1?cat=2000%2C2040%2C2045&t=movie&imdbid=15239678&apikey=secret-key"),
        "{asked:?}"
    );
    // The view never carries links, guids or the key.
    let shown = serde_json::to_string(&view).unwrap();
    assert!(
        !shown.contains("secret-key") && !shown.contains("passkey") && !shown.contains("download"),
        "{shown}"
    );
    // But they're kept for grabbing.
    let link: String =
        sqlx::query_scalar("SELECT download_url FROM release_decisions WHERE accepted = 1 LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(link.contains("apikey=secret-key"));
}

#[tokio::test]
async fn compares_against_the_file_on_disk() {
    let pool = setup().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation)
             VALUES (1, 1, 'Dune Part Two (2024)', 'Dune Part Two', 2024, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (path, missing) in [
        (
            "Dune Part Two (2024)/Dune Part Two (2024) WEBDL-2160p.mkv",
            None,
        ),
        (
            "Dune Part Two (2024)/Dune Part Two (2024) Remux-2160p.mkv",
            Some(5),
        ),
    ] {
        sqlx::query(
                "INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id, first_seen_generation,
                     seen_generation, missing_since)
                 VALUES (1, ?, 1, 0, 1, 1, 1, ?)",
            )
            .bind(path)
            .bind(missing)
            .execute(&pool)
            .await
            .unwrap();
    }
    let id = want_dune(&pool, "4K Preferred").await;
    sqlx::query("UPDATE wanted_movies SET movie_id = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    indexers(
        &pool,
        feed(&[
            "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX",
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
        ]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let view = get(
        &pool,
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    // The missing remux doesn't count: the WEBDL-2160p is what's there.
    assert_eq!(
        summary(&view),
        [
            (
                "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR"
                    .into(),
                true,
                vec![]
            ),
            (
                "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX".into(),
                false,
                vec!["the existing file's quality (WEBDL-2160p) is better".into()]
            ),
        ]
    );
}

#[tokio::test]
async fn a_file_that_cant_be_judged_blocks_upgrades() {
    let pool = setup().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation)
             VALUES (1, 1, 'Dune Part Two (2024)', 'Dune Part Two', 2024, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id, first_seen_generation, seen_generation)
             VALUES (1, 'Dune Part Two (2024)/Dune Part Two (2024).mkv', 1, 0, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let id = want_dune(&pool, "4K Preferred").await;
    sqlx::query("UPDATE wanted_movies SET movie_id = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    indexers(
        &pool,
        feed(&["Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX"]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let view = get(
        &pool,
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let rejections = view.releases[0].decision["rejections"].as_array().unwrap();
    assert!(!view.releases[0].accepted);
    assert!(
        rejections[0]
            .as_str()
            .unwrap()
            .contains("doesn't name its quality"),
        "{rejections:?}"
    );
}

#[tokio::test]
async fn multi_episode_releases_are_compared_with_every_episode_they_cover() {
    let pool = setup().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
            "INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'Show', 'Show', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    // E05 is a 720p file; E06 a 2160p remux.
    for (path, episode) in [
        ("Show/Show.S01E05.720p.HDTV.x264-GRP.mkv", 5),
        ("Show/Show.S01E06.2160p.BluRay.REMUX.HEVC-GRP.mkv", 6),
    ] {
        sqlx::query(
                "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season, episode,
                     first_seen_generation, seen_generation)
                 VALUES (1, ?, 1, 0, 1, 1, ?, 1, 1)",
            )
            .bind(path)
            .bind(episode)
            .execute(&pool)
            .await
            .unwrap();
    }
    let id: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, quality_profile, source, added_at, series_id)
             VALUES (1, 'Show', 1, 'standard', '4K Preferred', 'sonarr', 0, 1) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    indexers(
        &pool,
        feed(&[
            "Show.S01E05.1080p.WEB-DL.H.264-GRP",
            "Show.S01E05E06.1080p.WEB-DL.H.264-GRP",
        ]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let target = Target::Series {
        id,
        season: 1,
        episode: Some(5),
    };
    let view = get(
        &pool,
        search(&pool, &client, None, target).await.unwrap().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let verdict = |title: &str| view.releases.iter().find(|r| r.title == title).unwrap();
    // Better than E05's 720p, so the single episode is an upgrade...
    assert!(verdict("Show.S01E05.1080p.WEB-DL.H.264-GRP").accepted);
    // ...but the double episode would replace E06's remux too.
    let double = verdict("Show.S01E05E06.1080p.WEB-DL.H.264-GRP");
    assert!(!double.accepted);
    assert_eq!(
        double.decision["rejections"][0],
        "the existing file's quality (Remux-2160p) is better"
    );
}

#[tokio::test]
async fn checks_episodes_and_seasons() {
    let pool = setup().await;
    let id: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, title, year, monitored, series_type, quality_profile, source, added_at)
             VALUES (424536, 'Frieren: Beyond Journey''s End', 2023, 1, 'anime', '4K Preferred', 'sonarr', 0)
             RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    let log = indexers(
        &pool,
        feed(&[
            "Frieren.Beyond.Journeys.End.S01E05.1080p.WEB.H264-GRP",
            "Frieren.Beyond.Journeys.End.S01E06.1080p.WEB.H264-GRP",
            "Frieren.Beyond.Journeys.End.S01.1080p.BluRay.x264-GRP",
            "[SubsPlease] Frieren - Beyond Journey's End - 05 (1080p) [ABCD1234].mkv",
            "Other.Show.S01E05.1080p.WEB.H264-GRP",
        ]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let episode = Target::Series {
        id,
        season: 1,
        episode: Some(5),
    };
    let view = get(
        &pool,
        search(&pool, &client, None, episode)
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let verdicts: Vec<(String, bool, Vec<String>)> = summary(&view);
    let find = |title: &str| {
        verdicts
            .iter()
            .find(|(t, ..)| t.starts_with(title))
            .unwrap()
            .clone()
    };
    assert!(find("Frieren.Beyond.Journeys.End.S01E05").1);
    assert_eq!(
        find("Frieren.Beyond.Journeys.End.S01E06").2,
        ["it isn't S01E05"]
    );
    assert_eq!(
        find("Frieren.Beyond.Journeys.End.S01.").2,
        ["it's a season pack; this searched for one episode"]
    );
    assert!(find("[SubsPlease]").2[0].contains("numbered from the first episode"));
    assert!(find("Other.Show").2[0].starts_with("it's \"Other Show\""));
    assert!(
        log.lock()
            .unwrap()
            .iter()
            .any(|q| q.starts_with("1?cat=5000%2C5040&t=tvsearch&tvdbid=424536&season=1&ep=5&"))
    );

    let season = Target::Series {
        id,
        season: 1,
        episode: None,
    };
    let view = get(
        &pool,
        search(&pool, &client, None, season).await.unwrap().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let accepted: Vec<&str> = view
        .releases
        .iter()
        .filter(|r| r.accepted)
        .map(|r| r.title.as_str())
        .collect();
    assert_eq!(
        accepted,
        ["Frieren.Beyond.Journeys.End.S01.1080p.BluRay.x264-GRP"]
    );
}

#[tokio::test]
async fn refuses_to_start_without_what_it_needs() {
    let pool = setup().await;
    let client = torznab::Client::new().unwrap();
    assert_eq!(
        search(&pool, &client, None, Target::Movie(9))
            .await
            .unwrap(),
        Err(Refused::NotWanted)
    );
    let id = want_dune(&pool, "Gone Profile").await;
    let refused = search(&pool, &client, None, Target::Movie(id))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(refused, Refused::NoProfile(ref why) if why.contains("Gone Profile")),
        "{refused}"
    );
    let id = want_dune_again(&pool).await;
    assert_eq!(
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap(),
        Err(Refused::NoIndexers)
    );
}

async fn want_dune_again(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
            "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, quality_profile, source, added_at)
             VALUES (1, 'Other', 2020, 1, '4K Preferred', 'radarr', 0) RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn old_searches_are_removed() {
    let pool = setup().await;
    let id = want_dune(&pool, "4K Preferred").await;
    indexers(&pool, feed(&[])).await;
    sqlx::query(
        "INSERT INTO searches (wanted_movie_id, searched_at, indexers) VALUES (?, 0, '[]')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let client = torznab::Client::new().unwrap();
    let new = search(&pool, &client, None, Target::Movie(id))
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM searches")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(ids, [new]);
}

#[test]
fn describes_files_from_their_probe() {
    let probe = |w: i64, h: i64, hdr: Option<&str>| {
        serde_json::json!({ "container": "matroska,webm", "duration_seconds": 1.0, "bit_rate": 1,
                "video": [{ "index": 0, "codec": "hevc", "profile": null, "width": w, "height": h,
                            "pixel_format": null, "frame_rate": null, "hdr": hdr }],
                "audio": [], "subtitles": [] })
        .to_string()
    };
    assert_eq!(
        describe(&probe(3840, 1600, Some("Dolby Vision"))).as_deref(),
        Some("2160p BluRay REMUX DV HDR10")
    );
    assert_eq!(
        describe(&probe(1920, 800, None)).as_deref(),
        Some("1080p BluRay REMUX")
    );
    assert_eq!(
        describe(&probe(1280, 720, Some("HDR10"))).as_deref(),
        Some("720p BluRay HDR10")
    );
    assert_eq!(
        describe(&probe(720, 576, None)).as_deref(),
        Some("480p BluRay")
    );
    assert_eq!(describe("not json"), None);
    assert!(looks_numbered("Show - 05.mkv"));
    assert!(looks_numbered("[Group] Show 1085 [1080p].mkv"));
    assert!(!looks_numbered("Behind the Scenes.mkv"));
    assert!(!looks_numbered("Making of (2019).mkv"));
    assert!(!looks_numbered("Featurette.1080p.mkv"));
}

#[test]
fn a_bare_episode_number_is_never_a_movie() {
    let wanted = movie_wanted("Dune Part Two", Some(2024));
    let release = release::parse("Dune.Part.Two.2024.E05.1080p.WEB.H264-GRP");
    assert_eq!(release.episodes, Episodes::Absolute(vec![5]));
    assert_eq!(
        mismatch(&release, &wanted, Target::Movie(1)).as_deref(),
        Some("it's a TV release")
    );
}

#[tokio::test]
async fn unnamed_files_are_judged_by_their_probe() {
    let pool = setup().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation) VALUES (1, 1, 'Dune Part Two (2024)', 'Dune Part Two', 2024, 1)")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query(
            "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id, first_seen_generation, seen_generation)
             VALUES (1, 1, 'Dune Part Two (2024)/Dune Part Two (2024).mkv', 10, 20, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let info = serde_json::json!({ "container": "matroska,webm", "duration_seconds": 1.0, "bit_rate": 1,
            "video": [{ "index": 0, "codec": "hevc", "profile": null, "width": 3840, "height": 1600,
                        "pixel_format": null, "frame_rate": null, "hdr": "Dolby Vision" }],
            "audio": [], "subtitles": [] });
    sqlx::query("INSERT INTO media_probes (file_id, size, modified_at, probed_at, info) VALUES (1, 10, 20, 0, ?)")
            .bind(info.to_string())
            .execute(&pool)
            .await
            .unwrap();
    let id = want_dune(&pool, "4K Preferred").await;
    sqlx::query("UPDATE wanted_movies SET movie_id = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    indexers(
        &pool,
        feed(&["Dune.Part.Two.2024.2160p.WEB-DL.DDP5.1.DV.H.265-GRP"]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let view = get(
        &pool,
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    // A 2160p DV file with no name to go by counts as a 2160p remux: not replaced by a WEB-DL.
    let release = &view.releases[0];
    assert!(!release.accepted);
    assert_eq!(
        release.decision["rejections"][0],
        "the existing file's quality (Remux-2160p) is better"
    );
}

#[tokio::test]
async fn waits_when_a_show_has_files_it_couldnt_number() {
    let pool = setup().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (2, 'shows', 'shows', '/s')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 2, 'Show', 'Show', 1)")
            .execute(&pool)
            .await
            .unwrap();
    sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, first_seen_generation, seen_generation)
             VALUES (2, 'Show/Behind the Scenes.mkv', 1, 0, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, quality_profile, source, added_at, series_id)
             VALUES (1, 'Show', 1, 'standard', '4K Preferred', 'sonarr', 0, 1) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    indexers(&pool, feed(&["Show.S01E05.1080p.WEB-DL.H.264-GRP"])).await;
    let client = torznab::Client::new().unwrap();
    let target = Target::Series {
        id,
        season: 1,
        episode: Some(5),
    };
    // Extras without numbers don't stand in the way...
    let view = get(
        &pool,
        search(&pool, &client, None, target).await.unwrap().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(view.releases[0].accepted, "{:?}", view.releases[0].decision);
    // ...files that look numbered do.
    sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, first_seen_generation, seen_generation)
             VALUES (2, 'Show/Show - 05.mkv', 1, 0, 1, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let view = get(
        &pool,
        search(&pool, &client, None, target).await.unwrap().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!view.releases[0].accepted);
    assert!(
        view.releases[0].decision["rejections"][0]
            .as_str()
            .unwrap()
            .contains("couldn't number, like Show - 05.mkv")
    );
}

#[tokio::test]
async fn a_year_off_is_fine_unless_tmdb_has_another_film_then() {
    use crate::tmdb::testing::{FakeTmdb, Reply};
    let pool = setup().await;
    let id = want_dune(&pool, "4K Preferred").await;
    indexers(
        &pool,
        feed(&[
            "Dune.Part.Two.2023.1080p.WEB-DL.DDP5.1.H.264-GRP",
            "Dune.Part.Two.2025.1080p.WEB-DL.DDP5.1.H.264-GRP",
        ]),
    )
    .await;
    let fake = FakeTmdb::default();
    // 2023: only the wanted film itself (its festival showing); 2025: another film.
    fake.reply(
            "/search/movie?include_adult=false&query=Dune: Part Two&year=2023",
            vec![Reply::ok(serde_json::json!({ "page": 1, "total_pages": 1, "results": [
                { "id": 693134, "title": "Dune: Part Two", "original_title": "Dune: Part Two", "release_date": "2024-02-27" } ] }))],
        )
        .reply(
            "/search/movie?include_adult=false&query=Dune: Part Two&year=2025",
            vec![Reply::ok(serde_json::json!({ "page": 1, "total_pages": 1, "results": [
                { "id": 1, "title": "Dune: Part Two", "original_title": null, "release_date": "2025-05-01" } ] }))],
        );
    let tmdb = fake.start().await;
    let client = torznab::Client::new().unwrap();
    let view = get(
        &pool,
        search(&pool, &client, Some(&tmdb), Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let verdict = |title: &str| view.releases.iter().find(|r| r.title == title).unwrap();
    assert!(verdict("Dune.Part.Two.2023.1080p.WEB-DL.DDP5.1.H.264-GRP").accepted);
    let later = verdict("Dune.Part.Two.2025.1080p.WEB-DL.DDP5.1.H.264-GRP");
    assert!(!later.accepted);
    assert_eq!(
        later.decision["rejections"][0],
        "it's from 2025, not 2024 (TMDB has another \"Dune: Part Two\" from 2025)"
    );
    // Without TMDB, a year off stays refused.
    let view = get(
        &pool,
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(view.releases.iter().all(|r| !r.accepted));
}

#[tokio::test]
async fn an_episode_range_holding_the_whole_season_counts_as_the_season() {
    let pool = setup().await;
    sqlx::query(
        "INSERT INTO tmdb_series (tmdb_id, name, genres, fetched_at) VALUES (55, 'Show', '[]', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for e in 1..=3 {
        sqlx::query("INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id) VALUES (55, 1, ?, ?)")
                .bind(e)
                .bind(100 + e)
                .execute(&pool)
                .await
                .unwrap();
    }
    let id: i64 = sqlx::query_scalar(
            "INSERT INTO wanted_series (tvdb_id, tmdb_id, title, monitored, series_type, quality_profile, source, added_at)
             VALUES (1, 55, 'Show', 1, 'anime', '4K Preferred', 'sonarr', 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    indexers(
        &pool,
        feed(&[
            "Show.S01E01-E03.1080p.WEB-DL.H.264-GRP",
            "Show.S01E01-E02.1080p.WEB-DL.H.264-GRP",
        ]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let season = Target::Series {
        id,
        season: 1,
        episode: None,
    };
    let view = get(
        &pool,
        search(&pool, &client, None, season).await.unwrap().unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let accepted: Vec<&str> = view
        .releases
        .iter()
        .filter(|r| r.accepted)
        .map(|r| r.title.as_str())
        .collect();
    assert_eq!(accepted, ["Show.S01E01-E03.1080p.WEB-DL.H.264-GRP"]);
}

#[tokio::test]
async fn a_profile_chosen_in_melete_is_used_over_the_arrs() {
    let pool = setup().await;
    let id = want_dune(&pool, "4K Preferred").await;
    let anime: i64 = crate::profiles::edit::anime_preset(&pool, "radarr")
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE wanted_movies SET profile_id = ? WHERE id = ?")
        .bind(anime)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    indexers(
        &pool,
        feed(&["Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX"]),
    )
    .await;
    let client = torznab::Client::new().unwrap();
    let view = get(
        &pool,
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(view.profile_id, Some(anime));
    // A profile of the other kind (it can't be chosen, but say it were) isn't used.
    let series_profile: i64 =
        sqlx::query_scalar("SELECT id FROM quality_profiles WHERE source = 'sonarr' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("UPDATE wanted_movies SET profile_id = ? WHERE id = ?")
        .bind(series_profile)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let view = get(
        &pool,
        search(&pool, &client, None, Target::Movie(id))
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(view.profile_id, Some(series_profile));
}

/// Frieren as TMDB lists it (28 episodes, then 10), wanted by TVDB id only.
async fn want_frieren_listed(pool: &SqlitePool, series_id: Option<i64>) -> i64 {
    sqlx::query(
            "INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, fetched_at) VALUES (209867, 424536, 'Frieren', 0);
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id)
             WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 38)
             SELECT 209867, iif(value <= 28, 1, 2), iif(value <= 28, value, value - 28), value FROM n;
             INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id) VALUES (209867, 0, 1, 1000)",
        )
        .execute(pool)
        .await
        .unwrap();
    sqlx::query_scalar(
        "INSERT INTO wanted_series (tvdb_id, title, year, monitored, series_type, quality_profile,
                 source, added_at, series_id, original_language)
             VALUES (424536, 'Frieren: Beyond Journey''s End', 2023, 1, 'anime', '4K Preferred',
                 'sonarr', 0, ?, 'en')
             RETURNING id",
    )
    .bind(series_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[test]
fn absolute_numbers_count_through_tmdbs_list() {
    let order: Vec<(i64, i64)> = (1..=28)
        .map(|e| (1, e))
        .chain((1..=10).map(|e| (2, e)))
        .collect();
    assert_eq!(in_season(&order, &[5]), Ok((1, vec![5])));
    assert_eq!(in_season(&order, &[30]), Ok((2, vec![2])));
    assert_eq!(
        in_season(&order, &[27, 28, 29]),
        Err("its episodes span seasons".into())
    );
    assert_eq!(
        in_season(&order, &[39]),
        Err("TMDB lists 38 episodes, not 39".into())
    );
    assert!(in_season(&[], &[5]).unwrap_err().contains("isn't known"));
}

#[tokio::test]
async fn releases_numbered_from_the_first_episode_are_mapped() {
    let pool = setup().await;
    sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'anime', 'shows', '/a');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Frieren', 'Frieren', 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season, episode,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Frieren/Frieren.S01E06.2160p.BluRay.REMUX.HEVC-GRP.mkv', 1, 0, 1, 1, 6, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
    let id = want_frieren_listed(&pool, Some(1)).await;
    indexers(
            &pool,
            feed(&[
                "[SubsPlease] Frieren - Beyond Journey's End - 05 (1080p) [ABCD1234].mkv",
                "[SubsPlease] Frieren - Beyond Journey's End - 06 (1080p) [ABCD1234].mkv",
                "[SubsPlease] Frieren - Beyond Journey's End - 30 (1080p) [ABCD1234].mkv",
                "[Judas] Frieren - Beyond Journey's End (01-28) [BD 1080p][HEVC x265 10bit][Dual-Audio][Eng-Subs]",
            ]),
        )
        .await;
    let client = torznab::Client::new().unwrap();
    let episode = |e| Target::Series {
        id,
        season: 1,
        episode: Some(e),
    };
    let verdicts = |view: SearchView| summary(&view);
    let five = verdicts(
        get(
            &pool,
            search(&pool, &client, None, episode(5))
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    let find = |v: &[(String, bool, Vec<String>)], t: &str| {
        v.iter()
            .find(|(title, ..)| title.contains(t))
            .unwrap()
            .clone()
    };
    assert!(find(&five, "End - 05").1, "{five:?}");
    assert!(find(&five, "End - 30").2[0].contains("as numbered it's S02E02"));
    // E06 is compared with the remux on disk for E06.
    let six = verdicts(
        get(
            &pool,
            search(&pool, &client, None, episode(6))
                .await
                .unwrap()
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    let (_, accepted, why) = find(&six, "End - 06");
    assert!(!accepted && why[0].contains("better"), "{why:?}");
    // The batch holds all of season 1.
    let season = Target::Series {
        id,
        season: 1,
        episode: None,
    };
    let whole = verdicts(
        get(
            &pool,
            search(&pool, &client, None, season).await.unwrap().unwrap(),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    assert!(find(&whole, "(01-28)").1, "{whole:?}");
}
