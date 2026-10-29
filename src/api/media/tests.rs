use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::extract::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};

struct Fixture {
    app: Router,
    pool: SqlitePool,
    token: String,
    upstream: Arc<AtomicUsize>,
    _artwork: tempfile::TempDir,
}

async fn exec(pool: &SqlitePool, sql: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await
        .unwrap();
}

/// The Matrix (matched), Zebra Film (unmatched), Gone (missing), and Severance (matched)
/// with a multi-episode file, an episode TMDB doesn't list, and an unparsed file.
async fn fixture() -> Fixture {
    let pool = db::open_in_memory().await;
    let libraries = sync_config(
        &pool,
        &[
            LibraryConfig {
                name: "movies".into(),
                kind: LibraryKind::Movies,
                path: "/m".into(),
            },
            LibraryConfig {
                name: "shows".into(),
                kind: LibraryKind::Shows,
                path: "/s".into(),
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!((libraries[0].id, libraries[1].id), (1, 2));
    exec(&pool, r#"
            INSERT INTO tmdb_movies (tmdb_id, imdb_id, title, original_title, release_date, year,
                overview, runtime_minutes, poster_path, backdrop_path, genres, fetched_at)
            VALUES (603, 'tt0133093', 'The Matrix', 'The Matrix', '1999-03-31', 1999,
                'A hacker learns the truth.', 136, '/matrix.jpg', '/matrix-bg.jpg',
                '["Science Fiction"]', 0);
            INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
                match_status, match_tmdb_id, missing_since) VALUES
                (1, 1, 'The Matrix (1999)', 'The Matrix', 1999, 1, 'matched', 603, NULL),
                (2, 1, 'Zebra Film (2005)', 'Zebra Film', 2005, 1, 'unmatched', NULL, NULL),
                (3, 1, 'Gone (2000)', 'Gone', 2000, 1, NULL, NULL, 99);
            INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                first_seen_generation, seen_generation, missing_since) VALUES
                (1, 'The Matrix (1999)/The Matrix.mkv', 100, 1000, 1, 1, 1, NULL),
                (1, 'The Matrix (1999)/Old.mkv', 50, 500, 1, 1, 1, 77),
                (1, 'Zebra Film (2005)/Zebra.mkv', 200, 2000, 2, 1, 1, NULL);

            INSERT INTO tmdb_series (tmdb_id, tvdb_id, name, year, status, poster_path, genres, fetched_at)
            VALUES (95396, 371980, 'Severance', 2022, 'Returning Series', '/sev.jpg', '["Drama"]', 0);
            INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, still_path) VALUES
                (95396, 1, 1, 11, 'Good News About Hell', '/s1e1.jpg'),
                (95396, 1, 2, 12, 'Half Loop', NULL),
                (95396, 1, 3, 13, 'In Perpetuity', NULL),
                (95396, 2, 1, 21, 'Hello, Ms. Cobel', NULL);
            INSERT INTO series (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
                match_status, match_tmdb_id) VALUES
                (1, 2, 'Severance (2022)', 'Severance', 2022, 1, 'matched', 95396);
            INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
                episode, episode_end, first_seen_generation, seen_generation) VALUES
                (2, 'Severance (2022)/S01E01.mkv', 10, 10, 1, 1, 1, NULL, 1, 1),
                (2, 'Severance (2022)/S01E02E03.mkv', 20, 20, 1, 1, 2, 3, 1, 1),
                (2, 'Severance (2022)/S03E01.mkv', 30, 30, 1, 3, 1, NULL, 1, 1),
                (2, 'Severance (2022)/Behind the scenes.mkv', 40, 40, 1, NULL, NULL, NULL, 1, 1);
        "#).await;

    let user = auth::create_user(&pool, "viewer", "viewer-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "test")
        .await
        .unwrap();

    // A stand-in for TMDB's image server that knows one image.
    let upstream = Arc::new(AtomicUsize::new(0));
    let counter = upstream.clone();
    let images = Router::new().fallback(move |request: Request| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            if request.uri().path() == "/w342/matrix.jpg" {
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "image/jpeg")],
                    b"jpeg bytes".to_vec(),
                )
            } else {
                (
                    StatusCode::NOT_FOUND,
                    [(header::CONTENT_TYPE, "text/plain")],
                    Vec::new(),
                )
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, images).await.unwrap() });

    let artwork = tempfile::tempdir().unwrap();
    let images = Images::with_base(format!("http://{addr}")).unwrap();
    Fixture {
        app: routes(pool.clone(), artwork.path().to_path_buf(), images),
        pool,
        token,
        upstream,
        _artwork: artwork,
    }
}

async fn get_raw(f: &Fixture, uri: &str, token: Option<&str>) -> Response {
    let mut request = Request::get(uri);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    f.app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn get(f: &Fixture, uri: &str) -> (StatusCode, Value) {
    let response = get_raw(f, uri, Some(&f.token)).await;
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn titles(body: &Value) -> Vec<&str> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn lists_present_movies_with_fallback_titles() {
    let f = fixture().await;
    let (status, body) = get(&f, "/api/v1/movies").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({ "total": 2, "items": [
            { "id": 1, "title": "The Matrix", "year": 1999,
              "poster": "/api/v1/artwork/w342/matrix.jpg", "matched": true,
              "added_at": 1000, "unwatched_episodes": null },
            { "id": 2, "title": "Zebra Film", "year": 2005, "poster": null, "matched": false,
              "added_at": 2000, "unwatched_episodes": null },
        ]})
    );
}

#[tokio::test]
async fn list_filters_sorts_and_pages() {
    let f = fixture().await;
    assert_eq!(
        titles(&get(&f, "/api/v1/movies?sort=recent").await.1),
        ["Zebra Film", "The Matrix"]
    );
    assert_eq!(
        titles(&get(&f, "/api/v1/movies?sort=year").await.1),
        ["Zebra Film", "The Matrix"]
    );
    assert_eq!(
        titles(&get(&f, "/api/v1/movies?q=MATR").await.1),
        ["The Matrix"]
    );
    assert_eq!(
        get(&f, "/api/v1/movies?q=%25").await.1["total"],
        0,
        "% is literal"
    );
    assert_eq!(get(&f, "/api/v1/movies?library=shows").await.1["total"], 0);
    let (_, page) = get(&f, "/api/v1/movies?limit=1&offset=1").await;
    assert_eq!(
        (titles(&page), page["total"].as_i64()),
        (vec!["Zebra Film"], Some(2))
    );
    for bad in ["sort=rating", "limit=0", "limit=501", "offset=-1"] {
        assert_eq!(
            get(&f, &format!("/api/v1/movies?{bad}")).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(titles(&get(&f, "/api/v1/series").await.1), ["Severance"]);
}

#[tokio::test]
async fn search_ignores_accents_and_punctuation_and_sorting_a_leading_article() {
    let f = fixture().await;
    exec(
        &f.pool,
        "INSERT INTO tmdb_movies (tmdb_id, title, original_title, fetched_at) VALUES
             (194, 'Amélie', 'Le Fabuleux Destin d''Amélie Poulain', 0),
             (10228, 'Pokémon: The First Movie', 'ミュウツーの逆襲', 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES
             (10, 1, 'Amelie (2001)', 'Amelie', 1, 'matched', 194),
             (11, 1, 'Pokemon (1998)', 'Pokemon', 1, 'matched', 10228),
             (12, 1, 'Spider-Man (2002)', 'Spider-Man', 1, NULL, NULL),
             (13, 1, 'Mad Max (1979)', 'Mad Max', 1, NULL, NULL),
             (14, 1, 'Memento (2000)', 'Memento', 1, NULL, NULL);",
    )
    .await;
    crate::search_keys::fill(&f.pool).await.unwrap();
    for (q, found) in [
        ("amelie", "Amélie"),
        ("AMELIE", "Amélie"),
        ("pokemon", "Pokémon: The First Movie"),
        ("spiderman", "Spider-Man"),
        ("spider%20man", "Spider-Man"),
        ("spider-man", "Spider-Man"),
        ("fabuleux%20destin", "Amélie"),
        (
            "%E3%83%9F%E3%83%A5%E3%82%A6%E3%83%84%E3%83%BC",
            "Pokémon: The First Movie",
        ),
    ] {
        assert_eq!(
            titles(&get(&f, &format!("/api/v1/movies?q={q}")).await.1),
            [found],
            "{q}"
        );
    }
    assert_eq!(
        get(&f, "/api/v1/movies?q=%25").await.1["total"],
        0,
        "punctuation alone still matches only itself"
    );
    assert_eq!(
        titles(&get(&f, "/api/v1/movies").await.1),
        [
            "Amélie",
            "Mad Max",
            "The Matrix",
            "Memento",
            "Pokémon: The First Movie",
            "Spider-Man",
            "Zebra Film"
        ]
    );
}

#[tokio::test]
async fn libraries_and_lists_carry_categories() {
    let f = fixture().await;
    exec(
        &f.pool,
        "UPDATE libraries SET category = 'anime' WHERE name = 'shows';
             INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/music');",
    )
    .await;
    let (status, body) = get(&f, "/api/v1/browse/libraries").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([
            { "id": 1, "name": "movies", "kind": "movies", "category": null, "titles": 2 },
            { "id": 2, "name": "shows", "kind": "shows", "category": "anime", "titles": 1 },
        ]),
        "no music, no paths"
    );
    assert_eq!(
        titles(&get(&f, "/api/v1/series?category=anime").await.1),
        ["Severance"]
    );
    assert_eq!(get(&f, "/api/v1/series?category=none").await.1["total"], 0);
    assert_eq!(get(&f, "/api/v1/movies?category=none").await.1["total"], 2);
    assert_eq!(get(&f, "/api/v1/movies?category=kids").await.1["total"], 0);
    assert_eq!(
        get(&f, "/api/v1/movies?category=cartoons").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get_raw(&f, "/api/v1/browse/libraries", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn genres_are_counted_in_one_request_and_filter_lists() {
    let f = fixture().await;
    exec(
        &f.pool,
        r#"UPDATE tmdb_series SET genres = '["Drama", "science fiction"]';
               UPDATE libraries SET category = 'anime' WHERE name = 'shows';
               INSERT INTO tmdb_movies (tmdb_id, title, year, genres, fetched_at)
                   VALUES (604, 'Gone', 2000, '["Drama"]', 0);
               UPDATE movies SET match_status = 'matched', match_tmdb_id = 604 WHERE id = 3;"#,
    )
    .await;
    let (status, body) = get(&f, "/api/v1/browse/genres").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!([
            { "name": "Science Fiction", "movies": 1, "series": 1 },
            { "name": "Drama", "movies": 0, "series": 1 },
        ]),
        "case aside; a missing movie isn't counted; most titles first"
    );
    assert_eq!(
        get(&f, "/api/v1/browse/genres?category=none").await.1,
        json!([{ "name": "Science Fiction", "movies": 1, "series": 0 }])
    );
    assert_eq!(
        get(&f, "/api/v1/browse/genres?library=shows").await.1,
        json!([
            { "name": "Drama", "movies": 0, "series": 1 },
            { "name": "science fiction", "movies": 0, "series": 1 },
        ])
    );
    assert_eq!(
        get(&f, "/api/v1/browse/genres?category=x").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        titles(&get(&f, "/api/v1/movies?genre=science%20fiction").await.1),
        ["The Matrix"]
    );
    assert_eq!(get(&f, "/api/v1/movies?genre=Drama").await.1["total"], 0);
    assert_eq!(
        titles(&get(&f, "/api/v1/series?genre=Science%20Fiction").await.1),
        ["Severance"]
    );
    assert_eq!(
        get_raw(&f, "/api/v1/browse/genres", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn series_items_count_the_callers_unwatched_episodes() {
    let f = fixture().await;
    let (_, body) = get(&f, "/api/v1/series").await;
    // S01E01, S01E02-E03 in one file and S03E01; the extra isn't an episode.
    assert_eq!(
        (
            &body["items"][0]["unwatched_episodes"],
            &body["items"][0]["added_at"]
        ),
        (&json!(4), &json!(10))
    );
    exec(
        &f.pool,
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, play_count,
             updated_at)
         SELECT u.id, f.id, 0, 1, 1, 5 FROM users u, media_files f
         WHERE u.username = 'viewer' AND f.rel_path = 'Severance (2022)/S01E02E03.mkv'",
    )
    .await;
    let (_, body) = get(&f, "/api/v1/series").await;
    assert_eq!(body["items"][0]["unwatched_episodes"], 2);
    // Someone else's watching counts for them alone.
    let other = auth::create_user(&f.pool, "other", "other-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = auth::issue_token(&f.pool, other.id, TokenKind::Session, "t")
        .await
        .unwrap();
    let response = get_raw(&f, "/api/v1/series", Some(&token)).await;
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["items"][0]["unwatched_episodes"], 4);
}

#[tokio::test]
async fn lists_leave_out_series_libraries_and_categories() {
    let f = fixture().await;
    exec(
        &f.pool,
        "UPDATE libraries SET category = 'kids' WHERE name = 'movies';
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (2, 2, 'Other', 'Other', 1);",
    )
    .await;
    assert_eq!(
        titles(&get(&f, "/api/v1/series?exclude_series=1").await.1),
        ["Other"]
    );
    assert_eq!(
        get(&f, "/api/v1/series?exclude_series=1,2").await.1["total"],
        0
    );
    assert_eq!(
        get(&f, "/api/v1/series?exclude_library=shows").await.1["total"],
        0
    );
    assert_eq!(
        get(&f, "/api/v1/movies?exclude_category=kids").await.1["total"],
        0
    );
    assert_eq!(
        get(&f, "/api/v1/movies?exclude_category=anime").await.1["total"],
        2
    );
    assert_eq!(
        get(&f, "/api/v1/movies?exclude_series=1").await.1["total"],
        2,
        "series ids don't touch movies"
    );
    for bad in ["exclude_series=one", "exclude_category=none"] {
        assert_eq!(
            get(&f, &format!("/api/v1/series?{bad}")).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
}

/// Series titles of `uri`'s answer for the user holding `token`.
async fn series_seen(f: &Fixture, uri: &str, token: &str) -> Vec<String> {
    let response = get_raw(f, uri, Some(token)).await;
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["total"],
        body["items"].as_array().unwrap().len(),
        "{uri}"
    );
    titles(&body).into_iter().map(str::to_owned).collect()
}

#[tokio::test]
async fn series_lists_leave_anime_out_or_list_only_anime() {
    let f = fixture().await;
    // Severance sits in the plain shows library; Wanted Anime sits there too but is wanted as
    // anime; Anime is in the anime library and Kids Show in the kids library.
    exec(
        &f.pool,
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
             (3, 'anime', 'shows', '/a', 'anime'), (4, 'kids', 'shows', '/k', 'kids');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES
             (2, 2, 'Wanted Anime', 'Wanted Anime', 1), (3, 3, 'Anime', 'Anime', 1),
             (4, 4, 'Kids Show', 'Kids Show', 1);
         INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, source, added_at,
             series_id) VALUES (1, 'Wanted Anime', 1, 'anime', 'manual', 0, 2),
             (2, 'Severance', 1, 'standard', 'manual', 0, 1);",
    )
    .await;
    let me = f.token.clone();
    assert_eq!(
        series_seen(&f, "/api/v1/series?anime=exclude", &me).await,
        ["Kids Show", "Severance"]
    );
    assert_eq!(
        series_seen(&f, "/api/v1/series?anime=only", &me).await,
        ["Anime", "Wanted Anime"]
    );
    // The other filters combine with it.
    let combined = [
        ("anime=exclude&category=none", vec!["Severance"]),
        ("anime=exclude&category=kids", vec!["Kids Show"]),
        ("anime=exclude&exclude_category=kids", vec!["Severance"]),
        ("anime=only&category=none", vec!["Wanted Anime"]),
        ("anime=only&q=wanted", vec!["Wanted Anime"]),
        ("anime=only&library=anime", vec!["Anime"]),
        ("anime=exclude&limit=1&offset=1", vec!["Severance"]),
    ];
    for (query, expected) in combined {
        let (status, body) = get(&f, &format!("/api/v1/series?{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(titles(&body), expected, "{query}");
    }
    // A user who may see only the plain and kids libraries sees only their part of each.
    let kid = auth::create_user(&f.pool, "kid", "kid-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let limit = crate::access::Restriction {
        libraries: Some(vec![2, 4]),
        max_age: None,
    };
    crate::access::set_restriction(&f.pool, kid.id, Some(&limit))
        .await
        .unwrap();
    let (_, kid) = auth::issue_token(&f.pool, kid.id, TokenKind::Session, "t")
        .await
        .unwrap();
    assert_eq!(
        series_seen(&f, "/api/v1/series?anime=exclude", &kid).await,
        ["Kids Show", "Severance"]
    );
    assert_eq!(
        series_seen(&f, "/api/v1/series?anime=only", &kid).await,
        ["Wanted Anime"]
    );
    for bad in ["/api/v1/series?anime=yes", "/api/v1/movies?anime=exclude"] {
        assert_eq!(get(&f, bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn details_carry_the_stored_textless_backdrop_logo_and_focus() {
    let f = fixture().await;
    exec(
        &f.pool,
        "UPDATE tmdb_movies SET textless_backdrop_path = '/plain.jpg', logo_path = '/logo.png';
             UPDATE tmdb_series SET logo_path = '/sev-logo.png';
             INSERT INTO artwork_focus (file, x, y) VALUES ('/matrix-bg.jpg', 0.7, 0.4);",
    )
    .await;
    let (_, movie) = get(&f, "/api/v1/movies/1").await;
    assert_eq!(
        movie["textless_backdrop"],
        "/api/v1/artwork/w1280/plain.jpg"
    );
    assert_eq!(movie["logo"], "/api/v1/artwork/w500/logo.png");
    assert_eq!(movie["backdrop_focus"], json!({ "x": 0.7, "y": 0.4 }));
    assert_eq!(
        movie["textless_backdrop_focus"],
        Value::Null,
        "not worked out yet, and nothing is fetched here"
    );
    let (_, series) = get(&f, "/api/v1/series/1").await;
    assert_eq!(series["logo"], "/api/v1/artwork/w500/sev-logo.png");
    assert_eq!(f.upstream.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn movie_details_with_present_files() {
    let f = fixture().await;
    let (status, body) = get(&f, "/api/v1/movies/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "id": 1, "library": "movies", "title": "The Matrix", "original_title": "The Matrix",
            "year": 1999, "overview": "A hacker learns the truth.", "runtime_minutes": 136,
            "genres": ["Science Fiction"], "tmdb_id": 603, "imdb_id": "tt0133093",
            "poster": "/api/v1/artwork/w500/matrix.jpg",
            "backdrop": "/api/v1/artwork/w1280/matrix-bg.jpg",
            "textless_backdrop": null, "logo": null, "backdrop_focus": null,
            "textless_backdrop_focus": null,
            "match_status": "matched",
            "files": [{ "id": 1, "path": "The Matrix (1999)/The Matrix.mkv", "size": 100, "modified_at": 1000 }],
        })
    );
    let (_, unmatched) = get(&f, "/api/v1/movies/2").await;
    assert_eq!(
        (unmatched["title"].as_str(), unmatched["genres"].clone()),
        (Some("Zebra Film"), json!([]))
    );
    assert_eq!(
        get(&f, "/api/v1/movies/3").await.0,
        StatusCode::NOT_FOUND,
        "missing movies are hidden"
    );
    assert_eq!(get(&f, "/api/v1/movies/99").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn details_list_a_titles_extras() {
    let f = fixture().await;
    exec(
        &f.pool,
        r#"INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
               title, first_seen_generation, seen_generation, missing_since, probed_size,
               probed_modified_at, probe_info) VALUES
               (1099511627776, 1, 'The Matrix (1999)/Featurettes/Bullet Time.mkv', 5, 1, 1,
                'featurette', 'Bullet Time', 1, 1, NULL, 5, 1, '{"duration_seconds": 300.5}'),
               (1099511627777, 1, 'The Matrix (1999)/The Matrix-trailer.mkv', 5, 1, 1,
                'trailer', 'The Matrix', 1, 1, NULL, NULL, NULL, NULL),
               (1099511627778, 1, 'The Matrix (1999)/Trailers/Old.mkv', 5, 1, 1,
                'trailer', 'Old', 1, 1, 9, NULL, NULL, NULL);"#,
    )
    .await;
    let (_, body) = get(&f, "/api/v1/movies/1/extras").await;
    assert_eq!(
        body,
        json!([
            { "id": 1099511627777_i64, "kind": "trailer", "title": "The Matrix",
              "duration_seconds": null },
            { "id": 1099511627776_i64, "kind": "featurette", "title": "Bullet Time",
              "duration_seconds": 300.5 },
        ]),
        "trailers first, missing ones left out"
    );
    let (_, movie) = get(&f, "/api/v1/movies/1").await;
    assert_eq!(
        movie["files"].as_array().unwrap().len(),
        1,
        "extras aren't files"
    );
    assert_eq!(get(&f, "/api/v1/movies/2/extras").await.1, json!([]));
    assert_eq!(get(&f, "/api/v1/series/1/extras").await.1, json!([]));
    assert_eq!(
        get(&f, "/api/v1/movies/3/extras").await.0,
        StatusCode::NOT_FOUND,
        "a missing movie has none to list"
    );
}

#[tokio::test]
async fn series_details_summarise_seasons() {
    let f = fixture().await;
    let (status, body) = get(&f, "/api/v1/series/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Severance");
    assert_eq!(body["tvdb_id"], 371980);
    assert_eq!(
        body["seasons"],
        json!([
            { "season": 1, "episodes": 3, "available": 3 },
            { "season": 2, "episodes": 1, "available": 0 },
            { "season": 3, "episodes": 0, "available": 1 },
        ])
    );
    assert_eq!(
        body["unparsed_files"][0]["path"],
        "Severance (2022)/Behind the scenes.mkv"
    );
}

#[tokio::test]
async fn season_joins_episodes_and_files() {
    let f = fixture().await;
    let (status, body) = get(&f, "/api/v1/series/1/seasons/1").await;
    assert_eq!(status, StatusCode::OK);
    let episodes = body["episodes"].as_array().unwrap();
    assert_eq!(episodes.len(), 3);
    assert_eq!(episodes[0]["still"], "/api/v1/artwork/w300/s1e1.jpg");
    assert_eq!(
        episodes[0]["files"][0]["path"],
        "Severance (2022)/S01E01.mkv"
    );
    for e in &episodes[1..] {
        assert_eq!(
            e["files"][0]["path"], "Severance (2022)/S01E02E03.mkv",
            "{e}"
        );
    }

    let (_, extra) = get(&f, "/api/v1/series/1/seasons/3").await;
    assert_eq!(extra["episodes"][0]["episode"], 1);
    assert_eq!(extra["episodes"][0]["name"], Value::Null);
    assert_eq!(extra["episodes"][0]["files"][0]["size"], 30);

    let (_, unwatched) = get(&f, "/api/v1/series/1/seasons/2").await;
    assert_eq!(unwatched["episodes"][0]["files"], json!([]));
    assert_eq!(
        get(&f, "/api/v1/series/1/seasons/9").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&f, "/api/v1/series/9/seasons/1").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn everything_needs_a_signed_in_user() {
    let f = fixture().await;
    for uri in [
        "/api/v1/movies",
        "/api/v1/series/1",
        "/api/v1/artwork/w342/matrix.jpg",
    ] {
        assert_eq!(
            get_raw(&f, uri, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    assert_eq!(f.upstream.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn artwork_is_downloaded_once_then_served_from_cache() {
    let f = fixture().await;
    for _ in 0..2 {
        let response = get_raw(&f, "/api/v1/artwork/w342/matrix.jpg", Some(&f.token)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert!(
            response.headers()[header::CACHE_CONTROL]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"jpeg bytes");
    }
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1);
    assert!(f._artwork.path().join("w342/matrix.jpg").exists());
}

#[tokio::test]
async fn peoples_pictures_only_at_their_one_size() {
    let f = fixture().await;
    exec(
        &f.pool,
        "INSERT INTO tmdb_people (id, name, profile_path) VALUES (6384, 'Keanu Reeves', '/keanu.jpg')",
    )
    .await;
    assert_eq!(
        get(&f, "/api/v1/artwork/w342/keanu.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.upstream.load(Ordering::SeqCst),
        0,
        "not at a poster's size"
    );
    // TMDB's stand-in hasn't it, but it was asked: the size and the picture were allowed.
    assert_eq!(
        get(&f, "/api/v1/artwork/w185/keanu.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1);
    assert_eq!(
        get(&f, "/api/v1/artwork/w185/matrix.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.upstream.load(Ordering::SeqCst),
        1,
        "a poster isn't a person's picture"
    );
}

#[tokio::test]
async fn artwork_only_at_the_sizes_the_api_hands_out() {
    let f = fixture().await;
    assert_eq!(
        get(&f, "/api/v1/artwork/original/matrix.jpg").await.0,
        StatusCode::BAD_REQUEST
    );
    for uri in [
        "/api/v1/artwork/w1280/matrix.jpg",
        "/api/v1/artwork/w300/matrix.jpg",
    ] {
        assert_eq!(
            get(&f, uri).await.0,
            StatusCode::NOT_FOUND,
            "a poster at a non-poster size: {uri}"
        );
    }
    assert_eq!(f.upstream.load(Ordering::SeqCst), 0, "nothing was fetched");
}

#[tokio::test]
async fn textless_backdrops_and_logos_are_fetched_at_their_own_sizes() {
    let f = fixture().await;
    exec(
        &f.pool,
        "UPDATE tmdb_movies SET textless_backdrop_path = '/plain.jpg', logo_path = '/logo.png'",
    )
    .await;
    for uri in [
        "/api/v1/artwork/w500/plain.jpg",
        "/api/v1/artwork/w1280/logo.png",
        "/api/v1/artwork/w342/logo.png",
    ] {
        assert_eq!(get(&f, uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    assert_eq!(f.upstream.load(Ordering::SeqCst), 0, "nothing was fetched");
    // The stand-in image server knows neither, but both are asked for.
    for uri in [
        "/api/v1/artwork/w1280/plain.jpg",
        "/api/v1/artwork/w500/logo.png",
    ] {
        assert_eq!(get(&f, uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    assert_eq!(f.upstream.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn listed_posters_are_fetched_only_when_allowed_and_only_at_w342() {
    let f = fixture().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    crate::discover_posters::listed(&f.pool, "/elsewhere.jpg", now)
        .await
        .unwrap();
    assert_eq!(
        get(&f, "/api/v1/artwork/w342/elsewhere.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.upstream.load(Ordering::SeqCst), 0, "off: not fetched");
    exec(
            &f.pool,
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY_POSTERS', 'on', 0)",
        )
        .await;
    assert_eq!(
        get(&f, "/api/v1/artwork/w500/elsewhere.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.upstream.load(Ordering::SeqCst), 0, "not at w500");
    // The stand-in image server doesn't know it, but it's asked.
    get(&f, "/api/v1/artwork/w342/elsewhere.jpg").await;
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn artwork_refuses_unknown_and_malformed_requests() {
    let f = fixture().await;
    assert_eq!(
        get(&f, "/api/v1/artwork/w342/not-ours.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.upstream.load(Ordering::SeqCst),
        0,
        "unknown images are never fetched"
    );
    assert_eq!(
        get(&f, "/api/v1/artwork/w9999/matrix.jpg").await.0,
        StatusCode::BAD_REQUEST
    );
    for file in ["..%2F..%2Fetc%2Fpasswd", "matrix.gif", ".jpg", "a.b.jpg"] {
        let uri = format!("/api/v1/artwork/w342/{file}");
        assert_eq!(get(&f, &uri).await.0, StatusCode::BAD_REQUEST, "{file}");
    }
    // Known to the library but gone from TMDB.
    assert_eq!(
        get(&f, "/api/v1/artwork/w300/s1e1.jpg").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_requested_titles_poster_may_be_fetched() {
    let f = fixture().await;
    sqlx::raw_sql(
            "INSERT INTO users (id, username, password_hash, is_admin, created_at) VALUES (90, 'alice', 'x', 0, 0);
             INSERT INTO requests (user_id, kind, tmdb_id, title, poster, state, created_at)
                 VALUES (90, 'movie', 7, 'Wanted Movie', '/requested.jpg', 'requested', 0);
             INSERT INTO requests (user_id, kind, mb_release_group_id, title, poster, state, created_at)
                 VALUES (90, 'album', 'rg', 'An Album', '/album.jpg', 'requested', 0);",
        )
        .execute(&f.pool)
        .await
        .unwrap();
    // TMDB is asked for it (the fake has only the Matrix, so it's a 404 from there).
    get(&f, "/api/v1/artwork/w342/requested.jpg").await;
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1);
    // A music request's picture isn't TMDB's: never fetched from there.
    get(&f, "/api/v1/artwork/w342/album.jpg").await;
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_image_tmdb_lacks_isnt_asked_for_again_soon() {
    let f = fixture().await;
    for _ in 0..3 {
        assert_eq!(
            get(&f, "/api/v1/artwork/w300/s1e1.jpg").await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(f.upstream.load(Ordering::SeqCst), 1, "TMDB was asked once");
    // Other images are still fetched.
    assert_eq!(
        get(&f, "/api/v1/artwork/w342/matrix.jpg").await.0,
        StatusCode::OK
    );
    assert_eq!(f.upstream.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn recent_goes_by_when_files_came_in_not_when_they_changed() {
    let f = fixture().await;
    // The Matrix's file came in first but was changed last (an upgrade, a tag edit): it stays
    // behind Zebra Film, which came in later.
    exec(
        &f.pool,
        "UPDATE media_files SET added_at = 100, modified_at = 9000
             WHERE rel_path = 'The Matrix (1999)/The Matrix.mkv';
         UPDATE media_files SET added_at = 200 WHERE rel_path = 'Zebra Film (2005)/Zebra.mkv';",
    )
    .await;
    assert_eq!(
        titles(&get(&f, "/api/v1/movies?sort=recent").await.1),
        ["Zebra Film", "The Matrix"]
    );
    exec(
        &f.pool,
        "UPDATE media_files SET added_at = 300 WHERE rel_path = 'The Matrix (1999)/The Matrix.mkv'",
    )
    .await;
    assert_eq!(
        titles(&get(&f, "/api/v1/movies?sort=recent").await.1),
        ["The Matrix", "Zebra Film"]
    );
}
