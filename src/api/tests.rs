use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

#[tokio::test]
async fn health_reports_ok_and_version() {
    let response = router()
        .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        body,
        json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") })
    );
}

#[tokio::test]
async fn admins_can_ask_for_a_scan() {
    let pool = crate::db::open_in_memory().await;
    let mut tokens = Vec::new();
    for (name, admin) in [("admin", true), ("viewer", false)] {
        let user = crate::auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        tokens.push(token);
    }
    let app = app(pool);
    for (token, expected) in [
        (&tokens[0], StatusCode::ACCEPTED),
        (&tokens[1], StatusCode::FORBIDDEN),
    ] {
        let request = Request::post("/api/v1/libraries/scan")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            expected
        );
    }
}

#[tokio::test]
async fn admins_scan_one_library_or_one_titles_folder() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Dune (2021)', 'Dune', 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut tokens = Vec::new();
    for (name, admin) in [("admin", true), ("viewer", false)] {
        let user = crate::auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        tokens.push(token);
    }
    let app = app(pool);
    for (uri, token, expected) in [
        (
            "/api/v1/libraries/movies/scan",
            &tokens[0],
            StatusCode::ACCEPTED,
        ),
        ("/api/v1/movies/1/rescan", &tokens[0], StatusCode::ACCEPTED),
        (
            "/api/v1/libraries/nope/scan",
            &tokens[0],
            StatusCode::NOT_FOUND,
        ),
        ("/api/v1/series/1/rescan", &tokens[0], StatusCode::NOT_FOUND),
        (
            "/api/v1/libraries/movies/scan",
            &tokens[1],
            StatusCode::FORBIDDEN,
        ),
        ("/api/v1/movies/1/rescan", &tokens[1], StatusCode::FORBIDDEN),
    ] {
        let request = Request::post(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let status = app.clone().oneshot(request).await.unwrap().status();
        assert_eq!(status, expected, "{uri}");
    }
}

#[tokio::test]
async fn unknown_route_is_404() {
    let response = router()
        .oneshot(Request::get("/api/v1/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lists_libraries_with_counts() {
    use crate::library::{LibraryConfig, LibraryKind, scan, sync_config};

    let dir = tempfile::tempdir().unwrap();
    for rel in [
        "Show (2020)/S01/Show S01E01.mkv",
        "Show (2020)/S01/Show 05.mkv",
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"video").unwrap();
    }
    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "shows".into(),
        kind: LibraryKind::Shows,
        path: dir.path().into(),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    scan(&pool, &library).await.unwrap();

    let response = library_routes(pool)
        .oneshot(
            Request::get("/api/v1/libraries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let mut body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body[0]["last_scan_at"].is_i64());
    body[0]["last_scan_at"] = Value::Null;
    assert_eq!(
        body,
        json!([{
            "id": library.id,
            "name": "shows",
            "kind": "shows",
            "category": null,
            "path": dir.path().to_str().unwrap(),
            "last_scan_at": null,
            "titles": 1,
            "files": 2,
            "missing_files": 0,
            "unparsed_files": 1,
        }])
    );
}

#[tokio::test]
async fn libraries_say_their_category() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
                 (1, 'anime', 'shows', '/a', 'anime'), (2, 'shows', 'shows', '/s', NULL);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = library_routes(pool)
        .oneshot(
            Request::get("/api/v1/libraries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (&body[0]["category"], &body[1]["category"]),
        (&json!("anime"), &Value::Null)
    );
}

#[tokio::test]
async fn counts_albums_and_tracks_for_music_libraries() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/music');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'dir:a', 'A', 'X', 'a', 1), (2, 1, 'dir:b', 'B', 'X', 'b', 1);
             UPDATE music_albums SET missing_since = 5 WHERE id = 2;
             INSERT INTO music_tracks (library_id, rel_path, size, modified_at, first_seen_generation,
                 seen_generation, album_id, tagged, title, missing_since) VALUES
                 (1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'One', NULL),
                 (1, 'a/2.flac', 1, 1, 1, 1, 1, 0, 'Two', NULL),
                 (1, 'b/1.flac', 1, 1, 1, 1, 2, 1, 'Gone', 5);",
        )
        .execute(&pool)
        .await
        .unwrap();
    let response = library_routes(pool)
        .oneshot(
            Request::get("/api/v1/libraries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let counts = (
        &body[0]["titles"],
        &body[0]["files"],
        &body[0]["missing_files"],
        &body[0]["unparsed_files"],
    );
    assert_eq!(counts, (&json!(1), &json!(2), &json!(1), &json!(1)));
}

#[tokio::test]
async fn lists_unmatched_and_ambiguous_titles() {
    use crate::library::{LibraryConfig, LibraryKind, sync_config};

    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: "/media/movies".into(),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    for (rel, title, year, status, candidates) in [
        (
            "Dune",
            "Dune",
            None,
            Some("ambiguous"),
            Some(r#"[{"tmdb_id":841,"title":"Dune","year":1984}]"#),
        ),
        ("Lost (2001)", "Lost", Some(2001), Some("unmatched"), None),
        ("Fine (2002)", "Fine", Some(2002), None, None),
    ] {
        sqlx::query(
            "INSERT INTO movies (library_id, rel_path, parsed_title, parsed_year, seen_generation,
                     match_status, match_candidates) VALUES (?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(library.id)
        .bind(rel)
        .bind(title)
        .bind(year)
        .bind(status)
        .bind(candidates)
        .execute(&pool)
        .await
        .unwrap();
    }

    let response = library_routes(pool)
        .oneshot(
            Request::get("/api/v1/metadata/unmatched")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let mut body: Value = serde_json::from_slice(&body).unwrap();
    for item in body.as_array_mut().unwrap() {
        item.as_object_mut().unwrap().remove("id");
    }
    assert_eq!(
        body,
        json!([
            { "kind": "movie", "library": "movies", "rel_path": "Dune", "parsed_title": "Dune",
              "parsed_year": null, "status": "ambiguous",
              "candidates": [{ "tmdb_id": 841, "title": "Dune", "year": 1984 }] },
            { "kind": "movie", "library": "movies", "rel_path": "Lost (2001)", "parsed_title": "Lost",
              "parsed_year": 2001, "status": "unmatched", "candidates": [] },
        ])
    );
}

mod openapi {
    use std::collections::BTreeSet;

    use axum::http::Method;

    use super::*;

    pub(super) fn spec() -> Value {
        serde_json::from_str(OPENAPI).expect("openapi.json is valid JSON")
    }

    pub(super) async fn full_server() -> (SqlitePool, Router, tempfile::TempDir) {
        full_server_with(None, None).await
    }

    /// The whole server, with `tmdb` as its TMDB and `musicbrainz` as its MusicBrainz.
    pub(super) async fn full_server_with(
        tmdb: Option<Arc<tmdb::Client>>,
        musicbrainz: Option<Arc<crate::musicbrainz::Client>>,
    ) -> (SqlitePool, Router, tempfile::TempDir) {
        let pool = crate::db::open_in_memory().await;
        let artwork = tempfile::tempdir().unwrap();
        // Never contacted: nothing in these tests asks for uncached artwork.
        let images = tmdb::Images::with_base("http://127.0.0.1:9".into()).unwrap();
        let transcoder = crate::transcode::Transcoder::new(
            "ffmpeg".into(),
            crate::transcode::Hardware::Software,
            artwork.path().join("transcode"),
            4,
        );
        let outside = Outside {
            images,
            tmdb,
            coverart: None,
            musicbrainz,
            slskd: None,
            stems: None,
            lyrics: None,
            translator: None,
            oidc: None,
            ai: None,
        };
        let app = server(
            pool.clone(),
            artwork.path(),
            outside,
            "ffprobe".into(),
            transcoder,
            None,
            tokio::sync::mpsc::channel(1).0,
        );
        (pool, app, artwork)
    }

    #[tokio::test]
    async fn every_documented_route_exists() {
        let (_, app, _artwork) = full_server().await;
        let spec = spec();
        let mut checked = 0;
        for (path, operations) in spec["paths"].as_object().unwrap() {
            let uri = path
                .replace("{id}", "1")
                .replace("{season}", "1")
                .replace("{size}", "w342")
                .replace("{file}", "x.jpg");
            for method in operations.as_object().unwrap().keys() {
                let method = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
                let request = Request::builder()
                    .method(method.clone())
                    .uri(&uri)
                    .body(Body::empty())
                    .unwrap();
                let status = app.clone().oneshot(request).await.unwrap().status();
                assert!(
                    status != StatusCode::NOT_FOUND && status != StatusCode::METHOD_NOT_ALLOWED,
                    "{method} {path} is documented but answered {status}"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 319);
    }

    /// The response's fields are exactly the schema's.
    fn assert_shape(spec: &Value, schema: &str, value: &Value) {
        let documented: BTreeSet<&str> = spec["components"]["schemas"][schema]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("no schema {schema}"))
            .keys()
            .map(String::as_str)
            .collect();
        let actual: BTreeSet<&str> = value
            .as_object()
            .unwrap_or_else(|| panic!("{schema}: not an object: {value}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            actual, documented,
            "{schema} fields differ from openapi.json"
        );
    }

    /// A signed-in user's token for `full_server`'s app, and a GET that must succeed.
    async fn user_get(
        pool: &SqlitePool,
        app: &Router,
    ) -> impl Fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send>> {
        let user = crate::auth::create_user(pool, "viewer", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = app.clone();
        move |uri: String| {
            let request = Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            Box::pin(async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            })
        }
    }

    #[tokio::test]
    async fn browse_libraries_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path, category)
                     VALUES (1, 'anime', 'shows', '/a', 'anime');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = user_get(&pool, &app).await;
        let libraries = get("/api/v1/browse/libraries".into()).await;
        assert_shape(&spec(), "BrowseLibrary", &libraries[0]);
    }

    #[tokio::test]
    async fn genres_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
                   INSERT INTO tmdb_movies (tmdb_id, title, year, genres, fetched_at)
                       VALUES (603, 'The Matrix', 1999, '["Action"]', 0);
                   INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                       match_status, match_tmdb_id)
                       VALUES (1, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = user_get(&pool, &app).await;
        let genres = get("/api/v1/browse/genres".into()).await;
        assert_shape(&spec(), "Genre", &genres[0]);
    }

    #[tokio::test]
    async fn favourites_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let get = user_get(&pool, &app).await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 1, 'Heat', 'Heat', 1);
             INSERT INTO favourites (user_id, movie_id, added_at)
                 VALUES ((SELECT id FROM users), 1, 1);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let spec = spec();
        let list = get("/api/v1/me/favourites".into()).await;
        assert_shape(&spec, "FavouriteList", &list);
        assert_shape(&spec, "Favourite", &list["items"][0]);
        let state = get("/api/v1/me/favourites/movies/1".into()).await;
        assert_shape(&spec, "FavouriteState", &state);
    }

    #[tokio::test]
    async fn title_artwork_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
                 INSERT INTO tmdb_movies (tmdb_id, title, backdrop_path, fetched_at)
                     VALUES (603, 'The Matrix', '/b.jpg', 0);
                 INSERT INTO artwork_focus (file, x, y) VALUES ('/b.jpg', 0.25, 0.5);
                 INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                     match_status, match_tmdb_id)
                     VALUES (1, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = user_get(&pool, &app).await;
        let artwork = get("/api/v1/movies/1/artwork".into()).await;
        assert_shape(&spec(), "TitleArtwork", &artwork);
        assert_eq!(artwork["backdrop_focus"], json!({ "x": 0.25, "y": 0.5 }));
    }

    #[tokio::test]
    async fn diagnostics_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let send = |method: &str, uri: String, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let report = json!({
            "app": "Video app", "app_version": "1.0", "platform": "tvOS", "level": "error",
            "summary": "Stalled", "details": { "file_id": 1 },
        });
        let kept = send("POST", "/api/v1/me/diagnostics".into(), Some(report)).await;
        assert_shape(&spec, "DiagnosticsKept", &kept);
        let page = send("GET", "/api/v1/diagnostics".into(), None).await;
        assert_shape(&spec, "DiagnosticsPage", &page);
        assert_shape(&spec, "DiagnosticsSummary", &page["items"][0]);
        let one = send("GET", format!("/api/v1/diagnostics/{}", kept["id"]), None).await;
        assert_shape(&spec, "DiagnosticsDetail", &one);
    }

    #[tokio::test]
    async fn file_segments_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
                 INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'Show', 'Show', 1);
                 INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                     season, episode, first_seen_generation, seen_generation)
                     VALUES (1, 1, 'Show/S01E01.mkv', 1, 1, 1, 1, 1, 1, 1);
                 INSERT INTO media_segment_scans (file_id, size, modified_at, scanned_at)
                     VALUES (1, 1, 1, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = user_get(&pool, &app).await;
        let segments = get("/api/v1/files/1/segments".into()).await;
        assert_shape(&spec(), "FileSegments", &segments);
    }

    #[tokio::test]
    async fn trailers_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
                 INSERT INTO tmdb_movies (tmdb_id, title, fetched_at, videos) VALUES (603, 'M', 1,
                     '[{"key": "vKQi3bBA1y8", "name": "Trailer", "language": "en",
                        "kind": "Trailer", "official": true, "size": 1080, "published_at": null}]');
                 INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                     match_status, match_tmdb_id) VALUES (1, 1, 'M', 'M', 1, 'matched', 603);
                 INSERT INTO settings (name, value, updated_at)
                     VALUES ('MELETE_TRAILERS_ONLINE', 'on', 0);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = user_get(&pool, &app).await;
        let trailer = get("/api/v1/movies/1/trailer".into()).await;
        assert_shape(&spec(), "Trailer", &trailer);
    }

    #[tokio::test]
    async fn discover_items_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
                 INSERT INTO tmdb_movies (tmdb_id, title, year, poster_path, fetched_at)
                     VALUES (603, 'The Matrix', 1999, '/m.jpg', 0);
                 INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                     match_status, match_tmdb_id)
                     VALUES (1, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = user_get(&pool, &app).await;
        let item = get("/api/v1/items/tmdb:movie:603".into()).await;
        assert_shape(&spec(), "DiscoverItem", &item);
    }

    #[tokio::test]
    async fn next_up_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let get = user_get(&pool, &app).await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
                 INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'Show', 'Show', 1);
                 INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                     season, episode, first_seen_generation, seen_generation) VALUES
                     (1, 1, 'Show/S01E01.mkv', 1, 1, 1, 1, 1, 1, 1),
                     (2, 1, 'Show/S01E02.mkv', 1, 1, 1, 1, 2, 1, 1);
                 INSERT INTO playback_progress (user_id, file_id, position_seconds, watched,
                     play_count, updated_at)
                     SELECT id, 1, 0, 1, 1, 5 FROM users WHERE username = 'viewer';",
        )
        .execute(&pool)
        .await
        .unwrap();
        let next = get("/api/v1/me/next-up".into()).await;
        assert_shape(&spec(), "NextUpItem", &next[0]);
    }

    #[tokio::test]
    async fn watch_home_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let get = user_get(&pool, &app).await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/s');
                 INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'Show', 'Show', 1);
                 INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                     season, episode, first_seen_generation, seen_generation) VALUES
                     (1, 1, 'Show/S01E01.mkv', 1, 1, 1, 1, 1, 1, 1),
                     (2, 1, 'Show/S01E02.mkv', 1, 1, 1, 1, 2, 1, 1);
                 INSERT INTO playback_progress (user_id, file_id, position_seconds, watched,
                     play_count, updated_at)
                     SELECT id, 2, 30, 0, 0, 5 FROM users WHERE username = 'viewer';",
        )
        .execute(&pool)
        .await
        .unwrap();
        let spec = spec();
        let home = get("/api/v1/home".into()).await;
        assert_shape(&spec, "WatchHome", &home);
        assert_shape(&spec, "WatchHomeRow", &home["rows"][0]);
        assert_shape(&spec, "WatchHomeItem", &home["hero"][0]);
        assert_shape(&spec, "WatchHomeItem", &home["rows"][0]["items"][0]);
    }

    #[tokio::test]
    async fn music_likes_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let get = user_get(&pool, &app).await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                 INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                     seen_generation) VALUES (1, 1, 'k', 'Post', 'Björk', 'a', 1);
                 INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, album_id, tagged, title, genres)
                     VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Army of Me', '[]');
                 INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                     payload, created_at, received_at)
                     SELECT 'o', id, 'phone', 'likes', 'L', 'upsert',
                         '{"contentKey":"k","deleted":false,"id":"L","jellyfinId":"1","liked":true,"originDevice":"phone","updatedAt":1000}',
                         1000, 1000 FROM users WHERE username = 'viewer';"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let spec = spec();
        let likes = get("/api/v1/music/likes".into()).await;
        assert_shape(&spec, "MusicLikes", &likes);
        assert_shape(&spec, "MusicTrack", &likes["tracks"][0]["track"]);
        assert_eq!(likes["tracks"][0]["track"]["liked"], true);
    }

    #[tokio::test]
    async fn quick_connect_responses_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let post = |uri: &'static str, bearer: bool, body: Value| {
            let mut request = Request::post(uri).header("content-type", "application/json");
            if bearer {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let request = request.body(Body::from(body.to_string())).unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let started = post(
            "/api/v1/auth/quick-connect",
            false,
            json!({ "device_name": "tv" }),
        )
        .await;
        assert_shape(&spec, "QuickConnectCode", &started);
        let secret = json!({ "secret": started["secret"] });
        let waiting = post("/api/v1/auth/quick-connect/poll", false, secret.clone()).await;
        assert_shape(&spec, "QuickConnectWaiting", &waiting);
        let seen = post(
            "/api/v1/auth/quick-connect/lookup",
            true,
            json!({ "code": started["code"] }),
        )
        .await;
        assert_shape(&spec, "QuickConnectLookup", &seen);
        let approved = post(
            "/api/v1/auth/quick-connect/approve",
            true,
            json!({ "code": started["code"] }),
        )
        .await;
        assert_shape(&spec, "QuickConnectApproved", &approved);
        let session = post("/api/v1/auth/quick-connect/poll", false, secret).await;
        assert_shape(&spec, "LoginResponse", &session);
    }

    #[tokio::test]
    async fn oidc_responses_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let fake = crate::oidc::fake::start().await;
        let client = crate::oidc::Client::new(crate::oidc::fake::settings(&fake.issuer));
        let app = app(pool.clone()).merge(super::oidc::routes(
            pool.clone(),
            Some(Arc::new(client.unwrap())),
            Arc::default(),
        ));
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let call = |method: &'static str, uri: &'static str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json");
            let request = request
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        assert_shape(
            &spec,
            "OidcAbout",
            &call("GET", "/api/v1/auth/oidc", None).await,
        );
        assert_shape(
            &spec,
            "OidcLink",
            &call("GET", "/api/v1/auth/oidc/link", None).await,
        );
        assert_shape(
            &spec,
            "OidcStart",
            &call(
                "POST",
                "/api/v1/auth/oidc/link",
                Some(json!({ "password": "user-password" })),
            )
            .await,
        );
        let start = json!({ "device_name": "Firefox" });
        assert_shape(
            &spec,
            "OidcStart",
            &call("POST", "/api/v1/auth/oidc/start", Some(start)).await,
        );
    }

    #[tokio::test]
    async fn an_apps_oidc_sign_in_carries_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let fake = crate::oidc::fake::start().await;
        let client = crate::oidc::Client::new(crate::oidc::fake::settings(&fake.issuer));
        let app = super::oidc::routes(pool, Some(Arc::new(client.unwrap())), Arc::default());
        let body = json!({ "device_name": "tv", "return_to": "aoide://signed-in" });
        let request = Request::post("/api/v1/auth/oidc/start")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let started: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "OidcStart", &started);
        assert!(started["secret"].as_str().unwrap().starts_with("mos_"));
    }

    #[tokio::test]
    async fn ai_responses_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
                   INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year,
                       seen_generation) VALUES (1, 1, 'Heat (1995)', 'Heat', 1995, 1);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let (fake, api) = crate::ai::fake::start().await;
        *fake.content.lock().unwrap() =
            r#"{"titles": [{"title": "Heat", "year": 1995}, {"title": "Thief"}]}"#.into();
        let app = super::ai::routes(pool.clone(), Some(Arc::new(crate::ai::fake::client(&api))));
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let call = |method: &'static str, uri: &'static str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        assert_shape(&spec, "AiAbout", &call("GET", "/api/v1/ai", None).await);
        let found = call(
            "POST",
            "/api/v1/ai/search",
            Some(json!({ "kind": "movies", "query": "crime" })),
        )
        .await;
        assert_shape(&spec, "AiSearch", &found);
        assert_shape(&spec, "ListItem", &found["results"][0]);
        assert_shape(&spec, "AiSuggestion", &found["suggested"][1]);
    }

    #[tokio::test]
    async fn listening_together_carries_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                 INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                     seen_generation) VALUES (1, 1, 'k', 'A', 'B', 'a', 1);
                 INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                     first_seen_generation, seen_generation, album_id, tagged, title, genres)
                     VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'One', '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = super::listen::routes(pool);
        let call = |method: &'static str, uri: &'static str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let state = json!({ "queue": [1], "index": 0, "position_ms": 0, "playing": true });
        let started = call("POST", "/api/v1/music/sessions", Some(state)).await;
        assert_shape(&spec, "ListenAnswer", &started);
        assert_shape(&spec, "ListenSession", &started["session"]);
        assert_shape(&spec, "ListenMember", &started["session"]["members"][0]);
        let mine = call("GET", "/api/v1/music/sessions", None).await;
        assert_shape(&spec, "ListenSession", &mine[0]);
    }

    #[tokio::test]
    async fn classical_responses_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Symphony No. 5', 'Beethoven', 'a', 1);
                   INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres,
                       composer, work, movement, movement_number) VALUES
                       (1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'I', '[]', 'Ludwig van Beethoven',
                        'Symphony No. 5', 'Allegro con brio', 1);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::classical::refresh(&pool).await.unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let get = |uri: String| {
            let request = Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let composers = get("/api/v1/music/composers".into()).await;
        assert_shape(&spec, "MusicComposerPage", &composers);
        assert_shape(&spec, "MusicComposer", &composers["items"][0]);
        let composer = get(format!(
            "/api/v1/music/composers/{}",
            composers["items"][0]["id"]
        ))
        .await;
        assert_shape(&spec, "MusicComposerDetail", &composer);
        let works = get("/api/v1/music/works".into()).await;
        assert_shape(&spec, "MusicWorkPage", &works);
        assert_shape(&spec, "MusicWork", &works["items"][0]);
        let work = get(format!("/api/v1/music/works/{}", works["items"][0]["id"])).await;
        assert_shape(&spec, "MusicWorkDetail", &work);
        assert_shape(&spec, "MusicWorkMovement", &work["movements"][0]);
        let recording = &work["recording_list"][0];
        assert_shape(&spec, "MusicWorkRecording", recording);
        assert_shape(&spec, "MusicWorkPart", &recording["tracks"][0]);
        assert_shape(&spec, "MusicAlbum", &recording["album"]);
        assert_shape(&spec, "MusicTrack", &recording["tracks"][0]["track"]);
    }

    #[tokio::test]
    async fn pronounced_lyrics_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let scratch = tempfile::tempdir().unwrap();
        let (base, _) = crate::libretranslate::testing::start(false).await;
        let app = server(
            pool.clone(),
            scratch.path(),
            Outside {
                images: tmdb::Images::with_base("http://127.0.0.1:9".into()).unwrap(),
                tmdb: None,
                coverart: None,
                musicbrainz: None,
                slskd: None,
                stems: None,
                lyrics: None,
                translator: Some(Arc::new(
                    crate::libretranslate::Client::new(&base, None).unwrap(),
                )),
                oidc: None,
                ai: None,
            },
            "ffprobe".into(),
            crate::transcode::Transcoder::new(
                "ffmpeg".into(),
                crate::transcode::Hardware::Software,
                scratch.path().join("transcode"),
                4,
            ),
            None,
            tokio::sync::mpsc::channel(1).0,
        );
        let library = scratch.path().join("music");
        std::fs::create_dir(&library).unwrap();
        std::fs::write(library.join("1.flac"), b"x").unwrap();
        std::fs::write(library.join("1.lrc"), "[00:01.00]夜は眠らない\n").unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', '', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 1, '1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]');
                   INSERT INTO settings (name, value, updated_at)
                       VALUES ('MELETE_LYRICS_ROMANISATION', 'on', 0);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let spec = spec();
        for (query, lyrics, line) in [
            (
                "pronounce=true",
                "MusicPronouncedLyrics",
                "MusicPronouncedLyricLine",
            ),
            (
                "pronounce=true&translate=de",
                "MusicTranslatedPronouncedLyrics",
                "MusicTranslatedPronouncedLyricLine",
            ),
        ] {
            let request = Request::get(format!("/api/v1/music/tracks/1/lyrics?{query}"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_shape(&spec, lyrics, &body);
            assert_shape(&spec, line, &body["lines"][0]);
        }
    }

    #[tokio::test]
    async fn translated_lyrics_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let scratch = tempfile::tempdir().unwrap();
        let (base, _) = crate::libretranslate::testing::start(false).await;
        let app = server(
            pool.clone(),
            scratch.path(),
            Outside {
                images: tmdb::Images::with_base("http://127.0.0.1:9".into()).unwrap(),
                tmdb: None,
                coverart: None,
                musicbrainz: None,
                slskd: None,
                stems: None,
                lyrics: None,
                translator: Some(Arc::new(
                    crate::libretranslate::Client::new(&base, None).unwrap(),
                )),
                oidc: None,
                ai: None,
            },
            "ffprobe".into(),
            crate::transcode::Transcoder::new(
                "ffmpeg".into(),
                crate::transcode::Hardware::Software,
                scratch.path().join("transcode"),
                4,
            ),
            None,
            tokio::sync::mpsc::channel(1).0,
        );
        let library = scratch.path().join("music");
        std::fs::create_dir(&library).unwrap();
        std::fs::write(library.join("1.flac"), b"x").unwrap();
        std::fs::write(library.join("1.lrc"), "[00:01.00]One\n").unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', '', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 1, '1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::get("/api/v1/music/tracks/1/lyrics?translate=de")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "MusicTranslatedLyrics", &body);
        assert_shape(&spec, "MusicTranslatedLyricLine", &body["lines"][0]);
    }

    #[tokio::test]
    async fn lyrics_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let library = tempfile::tempdir().unwrap();
        std::fs::write(library.path().join("1.flac"), b"x").unwrap();
        std::fs::write(library.path().join("1.lrc"), "[00:01.00]One\n").unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', '', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 1, '1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::get("/api/v1/music/tracks/1/lyrics")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "MusicLyrics", &body);
        assert_shape(&spec, "MusicLyricLine", &body["lines"][0]);
    }

    #[tokio::test]
    async fn lyric_search_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let library = tempfile::tempdir().unwrap();
        std::fs::write(library.path().join("1.flac"), b"x").unwrap();
        std::fs::write(library.path().join("1.lrc"), "[00:01.00]One more time\n").unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(library.path().to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', '', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 1, '1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::lyrics::read_library(&pool).await.unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::get("/api/v1/music/lyrics/search?q=more%20time")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "MusicLyricSearchPage", &body);
        assert_shape(&spec, "MusicLyricHit", &body["items"][0]);
        assert_shape(&spec, "MusicTrack", &body["items"][0]["track"]);
        assert_shape(&spec, "MusicLyricLine", &body["items"][0]["line"]);
    }

    #[tokio::test]
    async fn the_household_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', 'a', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres,
                       duration_seconds) VALUES
                       (1, 1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]', 200),
                       (2, 1, 'a/2.flac', 1, 1, 1, 1, 1, 1, 'Two', '[]', 200);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let mut tokens = Vec::new();
        for name in ["alice", "bob"] {
            let user = crate::auth::create_user(&pool, name, "user-password".into(), false)
                .await
                .unwrap()
                .unwrap();
            tokens.push((
                user.id,
                crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                    .await
                    .unwrap()
                    .1,
            ));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        for (id, track, started, ended) in [
            ("a", "1", now - 600_000, true),
            ("b", "2", now - 10_000, false),
        ] {
            crate::plays::record(
                &pool,
                tokens[0].0,
                crate::plays::Source::Api,
                &crate::plays::Listen {
                    event_id: id.into(),
                    track_key: track.into(),
                    device: "Phone".into(),
                    started_at: started,
                    ended_at: ended.then_some(started + 1000),
                    position_ms: None,
                    completed: ended,
                    skipped: false,
                    activity: None,
                },
            )
            .await
            .unwrap();
        }
        let call = |token: String, method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let set = call(
            tokens[0].1.clone(),
            "PUT",
            "/api/v1/music/household/sharing",
            Some(json!({ "sharing": true })),
        )
        .await;
        assert_shape(&spec, "ListeningSharing", &set);
        let household = call(tokens[1].1.clone(), "GET", "/api/v1/music/household", None).await;
        assert_shape(&spec, "MusicHousehold", &household);
        let alice = &household["people"][0];
        assert_shape(&spec, "MusicListener", alice);
        assert_shape(&spec, "MusicNowPlaying", &alice["now_playing"]);
        assert_shape(&spec, "MusicTrack", &alice["now_playing"]["track"]);
        assert_shape(&spec, "MusicHeard", &alice["recently_played"][0]);
    }

    #[tokio::test]
    async fn deleting_listens_answers_with_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::builder()
            .method("DELETE")
            .uri("/api/v1/music/plays?all=true")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let deleted: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "MusicPlaysDeleted", &deleted);
    }

    #[tokio::test]
    async fn transitions_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, tagged, title, genres)
                       VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 'One', '[]'),
                              (2, 1, 'a/2', 1, 1, 1, 1, 1, 'Two', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let call = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        call(
            "POST",
            "/api/v1/music/plays",
            Some(json!({ "id": "p", "track_id": 2, "started_at": 1000, "completed": true })),
        )
        .await;
        let kept = call(
            "POST",
            "/api/v1/music/transitions",
            Some(
                json!({ "from_track_id": 1, "to_track_id": 2, "started_at": 990,
                             "kind": "throw", "seconds": 6.5, "play_id": "p" }),
            ),
        )
        .await;
        let spec = spec();
        assert_shape(&spec, "MusicTransition", &kept);
        assert_shape(&spec, "MusicTransitionListen", &kept["listen"]);
        let page = call("GET", "/api/v1/music/transitions", None).await;
        assert_shape(&spec, "MusicTransitionPage", &page);
        assert_shape(&spec, "MusicTransition", &page["items"][0]);
    }

    #[tokio::test]
    async fn the_transition_summary_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, tagged, title, genres)
                       VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 'One', '[]'),
                              (2, 1, 'a/2', 1, 1, 1, 1, 1, 'Two', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let admin = crate::auth::create_user(&pool, "admin", "a-long-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let call = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        call(
            "POST",
            "/api/v1/music/transitions",
            Some(
                json!({ "from_track_id": 1, "to_track_id": 2, "started_at": now - 60,
                             "kind": "beatmatch", "seconds": 30, "rules": 1 }),
            ),
        )
        .await;
        let summary = call("GET", "/api/v1/music/transitions/summary?days=7", None).await;
        let spec = spec();
        assert_shape(&spec, "MusicTransitionSummary", &summary);
        for group in ["by_kind", "by_style", "by_rules"] {
            assert_shape(&spec, "MusicTransitionCount", &summary[group][0]);
        }
    }

    #[tokio::test]
    async fn ambient_mixes_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
                r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', 'a', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres,
                       duration_seconds) VALUES (1, 1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]', 200);
                   INSERT INTO music_audio_analysis (track_id, modified_at, analyzer, source,
                       loudness_lufs, true_peak_dbfs, bpm, bpm_confidence, measured_at)
                       VALUES (1, 1, 1, 'server', -20, -3, 72, 0.9, 1);"#,
            )
            .execute(&pool)
            .await
            .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let call = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let mixes = call("GET", "/api/v1/music/mixes", None).await;
        assert_shape(&spec, "MusicAmbientMix", &mixes[0]);
        let batch = call(
            "POST",
            "/api/v1/music/mixes/chill/next",
            Some(json!({ "queue": [], "limit": 5 })),
        )
        .await;
        assert_shape(&spec, "MusicAmbientMixTracks", &batch);
        assert_shape(&spec, "MusicTrack", &batch["tracks"][0]);
    }

    #[tokio::test]
    async fn pins_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_artists (id, key, name) VALUES (1, 'name:alice', 'Alice');
                   INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name,
                       folder, seen_generation) VALUES (1, 1, 'k', 'Album', 1, 'Alice', 'a', 1);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let op = crate::aoide::sync::validate(
                &serde_json::value::RawValue::from_string(
                    r#"{"opId":"p","entity":"playlists","entityId":"pl","operation":"upsert","payload":{"id":"pl","name":"Mix","updatedAt":1},"createdAt":1}"#
                        .into(),
                )
                .unwrap(),
            )
            .unwrap();
        crate::aoide::sync::push(&pool, user.id, "phone", vec![op], 1)
            .await
            .unwrap();
        let body = json!({ "items": [
            { "kind": "album", "id": "1" },
            { "kind": "artist", "id": "1" },
            { "kind": "playlist", "id": "pl" },
        ]});
        let request = Request::put("/api/v1/music/pins")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let pins: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "MusicPins", &pins);
        for pin in pins["items"].as_array().unwrap() {
            assert_shape(&spec, "MusicPin", pin);
        }
        assert_shape(&spec, "MusicAlbum", &pins["items"][0]["album"]);
        assert_shape(&spec, "MusicArtist", &pins["items"][1]["artist"]);
        assert_shape(&spec, "Playlist", &pins["items"][2]["playlist"]);
    }

    #[tokio::test]
    async fn home_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', 'a', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]'),
                              (2, 1, 'a/2.flac', 1, 1, 1, 1, 1, 1, 'Two', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        crate::plays::record(
            &pool,
            user.id,
            crate::plays::Source::Api,
            &crate::plays::Listen {
                event_id: "e".into(),
                track_key: "1".into(),
                device: "Phone".into(),
                started_at: now - 86_400_000,
                ended_at: None,
                position_ms: None,
                completed: true,
                skipped: false,
                activity: None,
            },
        )
        .await
        .unwrap();
        let request = Request::get("/api/v1/music/home")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "MusicHome", &body);
        assert_shape(&spec, "MusicAlbum", &body["recently_played"][0]);
        assert_shape(&spec, "MusicMostPlayed", &body["most_played"][0]);
        assert_shape(&spec, "MusicTrack", &body["most_played"][0]["track"]);
        assert_shape(&spec, "MusicAlbum", &body["recently_added"][0]);
        assert_shape(&spec, "MusicMix", &body["mixes"][0]);
        assert_shape(&spec, "MusicTrack", &body["mixes"][0]["tracks"][0]);
    }

    #[tokio::test]
    async fn play_history_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                       seen_generation) VALUES (1, 1, 'k', 'Album', 'Alice', 'a', 1);
                   INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'One', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let call = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let play = call(
            "POST",
            "/api/v1/music/plays",
            Some(json!({ "track_id": 1, "started_at": 1000, "skipped": true,
                             "position_seconds": 12, "activity": "focus" })),
        )
        .await;
        assert_shape(&spec, "MusicPlay", &play);
        assert_shape(&spec, "MusicTrack", &play["track"]);
        let page = call("GET", "/api/v1/music/plays", None).await;
        assert_shape(&spec, "MusicPlayPage", &page);
        assert_shape(&spec, "MusicPlay", &page["items"][0]);
    }

    #[tokio::test]
    async fn responses_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        crate::library::sync_config(
            &pool,
            &[
                crate::library::LibraryConfig {
                    name: "movies".into(),
                    kind: crate::library::LibraryKind::Movies,
                    path: "/m".into(),
                },
                crate::library::LibraryConfig {
                    name: "shows".into(),
                    kind: crate::library::LibraryKind::Shows,
                    path: "/s".into(),
                },
            ],
        )
        .await
        .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO tmdb_movies (tmdb_id, title, year, genres, fetched_at)
                       VALUES (603, 'The Matrix', 1999, '["Action"]', 0);
                   INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                       match_status, match_tmdb_id, match_candidates) VALUES
                       (1, 1, 'The Matrix (1999)', 'The Matrix', 1, 'matched', 603, NULL),
                       (2, 1, 'Dune', 'Dune', 1, 'ambiguous', NULL,
                        '[{"tmdb_id":841,"title":"Dune","year":1984}]');
                   INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                       first_seen_generation, seen_generation)
                       VALUES (1, 'The Matrix (1999)/m.mkv', 1, 1, 1, 1, 1);
                   INSERT INTO tmdb_series (tmdb_id, name, genres, fetched_at)
                       VALUES (1, 'Show', '[]', 0);
                   INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id)
                       VALUES (1, 1, 1, 11);
                   INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
                       match_status, match_tmdb_id) VALUES (1, 2, 'Show', 'Show', 1, 'matched', 1);
                   INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id,
                       season, episode, first_seen_generation, seen_generation)
                       VALUES (2, 'Show/S01E01.mkv', 1, 1, 1, 1, 1, 1, 1);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap();

        let send = |method: &str, uri: &str, token: Option<&str>, body: Option<Value>| {
            let mut request = Request::builder().method(method).uri(uri);
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            let request = match body {
                Some(body) => request
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string())),
                None => request.body(Body::empty()),
            }
            .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();

        let login =
            json!({ "username": "admin", "password": "admin-password", "device_name": "test" });
        let signed_in = send("POST", "/api/v1/auth/login", None, Some(login)).await;
        assert_shape(&spec, "LoginResponse", &signed_in);
        let login =
            json!({ "username": "admin", "password": "admin-password", "device_name": "web" });
        let web = send("POST", "/api/v1/auth/web-login", None, Some(login)).await;
        assert_shape(&spec, "WebLoginResponse", &web);
        let token = signed_in["token"].as_str().unwrap().to_owned();
        let token = Some(token.as_str());

        assert_shape(
            &spec,
            "Health",
            &send("GET", "/api/v1/health", None, None).await,
        );
        assert_shape(&spec, "Me", &send("GET", "/api/v1/me", token, None).await);
        assert_shape(
            &spec,
            "TokenInfo",
            &send("GET", "/api/v1/auth/tokens", token, None).await[0],
        );
        let key = send(
            "POST",
            "/api/v1/auth/keys",
            token,
            Some(json!({ "name": "k" })),
        )
        .await;
        assert_shape(&spec, "NewKey", &key);

        let page = send("GET", "/api/v1/movies", token, None).await;
        assert_shape(&spec, "Page", &page);
        assert_shape(&spec, "ListItem", &page["items"][0]);
        let movie = send("GET", "/api/v1/movies/1", token, None).await;
        assert_shape(&spec, "MovieDetail", &movie);
        assert_shape(&spec, "File", &movie["files"][0]);
        let series = send("GET", "/api/v1/series/1", token, None).await;
        assert_shape(&spec, "SeriesDetail", &series);
        assert_shape(&spec, "SeasonSummary", &series["seasons"][0]);
        let season = send("GET", "/api/v1/series/1/seasons/1", token, None).await;
        assert_shape(&spec, "Season", &season);
        assert_shape(&spec, "Episode", &season["episodes"][0]);

        let libraries = send("GET", "/api/v1/libraries", token, None).await;
        assert_shape(&spec, "LibrarySummary", &libraries[0]);
        let unmatched = send("GET", "/api/v1/metadata/unmatched", token, None).await;
        assert_shape(&spec, "Unmatched", &unmatched[0]);
        assert_shape(&spec, "Candidate", &unmatched[0]["candidates"][0]);
        let users = send("GET", "/api/v1/users", token, None).await;
        assert_shape(&spec, "User", &users[0]);
    }

    #[tokio::test]
    async fn malformed_requests_get_json_errors() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let cases: [(&str, &str, Option<&str>, StatusCode); 6] = [
            (
                "PUT",
                "/api/v1/movies/1/match",
                None,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ),
            (
                "PUT",
                "/api/v1/movies/1/match",
                Some("{}"),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "PUT",
                "/api/v1/movies/1/match",
                Some("{nope"),
                StatusCode::BAD_REQUEST,
            ),
            (
                "PUT",
                "/api/v1/movies/abc/match",
                Some(r#"{"tmdb_id":1}"#),
                StatusCode::BAD_REQUEST,
            ),
            ("GET", "/api/v1/nowhere", None, StatusCode::NOT_FOUND),
            (
                "POST",
                "/api/v1/health",
                None,
                StatusCode::METHOD_NOT_ALLOWED,
            ),
        ];
        for (method, uri, body, expected) in cases {
            let mut request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"));
            if body.is_some() {
                request = request.header("content-type", "application/json");
            }
            let request = request
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned())))
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected, "{method} {uri}");
            if expected == StatusCode::METHOD_NOT_ALLOWED {
                assert!(response.headers().contains_key("allow"), "headers are kept");
            }
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body: Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| panic!("{method} {uri}: not JSON: {bytes:?}"));
            assert!(
                body["error"].as_str().is_some_and(|e| !e.is_empty()),
                "{method} {uri}: {body}"
            );
        }
    }

    #[tokio::test]
    async fn decided_titles_leave_the_unmatched_list() {
        let pool = crate::db::open_in_memory().await;
        crate::library::sync_config(
            &pool,
            &[crate::library::LibraryConfig {
                name: "movies".into(),
                kind: crate::library::LibraryKind::Movies,
                path: "/m".into(),
            }],
        )
        .await
        .unwrap();
        sqlx::raw_sql(
            "INSERT INTO movies (library_id, rel_path, parsed_title, seen_generation,
                     match_status, match_method) VALUES
                     (1, 'Home Video', 'Home Video', 1, 'unmatched', 'manual'),
                     (1, 'Lost', 'Lost', 1, 'unmatched', NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let response = library_routes(pool)
            .oneshot(
                Request::get("/api/v1/metadata/unmatched")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let listed: Vec<&str> = body
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["rel_path"].as_str().unwrap())
            .collect();
        assert_eq!(listed, ["Lost"]);
    }

    #[tokio::test]
    async fn music_responses_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
                   INSERT INTO music_artists (id, key, name) VALUES (1, 'name:a', 'A');
                   INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at)
                       VALUES ('r', 'g', 'Album', '[]', 0);
                   INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name,
                       folder, seen_generation, match_status, match_release_id)
                       VALUES (1, 1, 'dir:a', 'Album', 1, 'A', 'a', 1, 'matched', 'r');
                   INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                       first_seen_generation, seen_generation, album_id, tagged, title, genres)
                       VALUES (1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'One', '["Rock"]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let get = |uri: &'static str| {
            let request = Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(
                    response.status().is_success(),
                    "{uri}: {}",
                    response.status()
                );
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let artists = get("/api/v1/music/artists").await;
        assert_shape(&spec, "MusicArtistPage", &artists);
        assert_shape(&spec, "MusicArtist", &artists["items"][0]);
        assert_shape(
            &spec,
            "MusicArtistDetail",
            &get("/api/v1/music/artists/1").await,
        );
        let albums = get("/api/v1/music/albums").await;
        assert_shape(&spec, "MusicAlbumPage", &albums);
        assert_shape(&spec, "MusicAlbum", &albums["items"][0]);
        let album = get("/api/v1/music/albums/1").await;
        assert_shape(&spec, "MusicAlbumDetail", &album);
        assert_shape(&spec, "MusicRelease", &album["release"]);
        assert_shape(&spec, "MusicTrack", &album["track_list"][0]);
        let tracks = get("/api/v1/music/tracks").await;
        assert_shape(&spec, "MusicTrackPage", &tracks);
        assert_shape(&spec, "MusicTrack", &tracks["items"][0]);
        assert_shape(&spec, "MusicGenre", &get("/api/v1/music/genres").await[0]);
        assert_shape(
            &spec,
            "MotionCoverChoice",
            &get("/api/v1/music/motion-covers").await,
        );
        sqlx::raw_sql(
            "INSERT INTO music_cover_colours (album_id, source, colours, measured_at)
                     VALUES (1, 'file:cover.jpg#00', '{}', 0);
                 INSERT INTO music_motion_covers (album_id, state, source, version, reason,
                     queued_at, finished_at)
                     VALUES (1, 'skipped', 'file:cover.jpg#00', 1, 'mostly text', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let album = get("/api/v1/music/albums/1").await;
        assert_shape(
            &spec,
            "MotionCoverGeneration",
            &album["motion_cover_generation"],
        );
    }

    #[tokio::test]
    async fn playback_responses_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let library = tempfile::tempdir().unwrap();
        std::fs::write(library.path().join("m.mkv"), b"video").unwrap();
        crate::library::sync_config(
            &pool,
            &[crate::library::LibraryConfig {
                name: "movies".into(),
                kind: crate::library::LibraryKind::Movies,
                path: library.path().into(),
            }],
        )
        .await
        .unwrap();
        sqlx::raw_sql(
                "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'm.mkv', 'M', 1);
                 INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                     first_seen_generation, seen_generation) VALUES (1, 1, 'm.mkv', 5, 1, 1, 1, 1);",
            )
            .execute(&pool)
            .await
            .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let mut request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"));
            if body.is_some() {
                request = request.header("content-type", "application/json");
            }
            let request = request
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        assert_shape(
            &spec,
            "PlayResponse",
            &send("POST", "/api/v1/files/1/play", None).await,
        );
        let report = json!({ "position_seconds": 1.0, "duration_seconds": 100.0 });
        assert_shape(
            &spec,
            "Progress",
            &send("PUT", "/api/v1/files/1/progress", Some(report)).await,
        );
        assert_shape(
            &spec,
            "ContinueItem",
            &send("GET", "/api/v1/me/continue", None).await[0],
        );
        let entries = send("GET", "/api/v1/me/progress?movie_id=1", None).await;
        assert_shape(&spec, "ProgressEntry", &entries[0]);
        let watched = send(
            "PUT",
            "/api/v1/files/1/watched",
            Some(json!({ "watched": true })),
        )
        .await;
        assert_shape(&spec, "Progress", &watched);
        assert_shape(
            &spec,
            "Languages",
            &send("GET", "/api/v1/me/languages", None).await,
        );
        sqlx::query(
            "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
             VALUES (1, 5, 1, 1, ?)",
        )
        .bind(
            json!({ "container": null, "duration_seconds": null, "bit_rate": null,
                    "video": [], "audio": [], "subtitles": [] })
            .to_string(),
        )
        .execute(&pool)
        .await
        .unwrap();
        let play = send("POST", "/api/v1/files/1/play", None).await;
        assert_shape(&spec, "TrackPick", &play["tracks"]);
    }

    #[tokio::test]
    async fn file_info_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let library = tempfile::tempdir().unwrap();
        let clip = crate::probe::testing::make_clip(library.path());
        let size = std::fs::metadata(&clip).unwrap().len() as i64;
        crate::library::sync_config(
            &pool,
            &[crate::library::LibraryConfig {
                name: "movies".into(),
                kind: crate::library::LibraryKind::Movies,
                path: library.path().into(),
            }],
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'clip.mkv', 'Clip', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
                "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                     first_seen_generation, seen_generation) VALUES (1, 1, 'clip.mkv', ?, 1, 1, 1, 1)",
            )
            .bind(size)
            .execute(&pool)
            .await
            .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::get("/api/v1/files/1")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let file: Value = serde_json::from_slice(&bytes).unwrap();

        let spec = spec();
        assert_shape(&spec, "FileInfo", &file);
        assert_shape(&spec, "MediaInfo", &file["media"]);
        assert_shape(&spec, "VideoStream", &file["media"]["video"][0]);
        assert_shape(&spec, "AudioStream", &file["media"]["audio"][0]);
        assert_shape(&spec, "SubtitleStream", &file["media"]["subtitles"][0]);
    }

    #[tokio::test]
    async fn transcode_response_carries_exactly_the_documented_fields() {
        let (pool, app, artwork) = full_server().await;
        let clip = crate::probe::testing::make_long_clip(artwork.path(), 7);
        let size = std::fs::metadata(&clip).unwrap().len() as i64;
        crate::library::sync_config(
            &pool,
            &[crate::library::LibraryConfig {
                name: "movies".into(),
                kind: crate::library::LibraryKind::Movies,
                path: artwork.path().into(),
            }],
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'long.mkv', 'Long', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
                "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                     first_seen_generation, seen_generation) VALUES (1, 1, 'long.mkv', ?, 1, 1, 1, 1)",
            )
            .bind(size)
            .execute(&pool)
            .await
            .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::post("/api/v1/files/1/transcode")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "TranscodeResponse", &body);
    }

    #[tokio::test]
    async fn subtitle_tracks_carry_exactly_the_documented_fields() {
        let (pool, app, artwork) = full_server().await;
        let clip = crate::probe::testing::make_clip(artwork.path());
        std::fs::write(
            artwork.path().join("clip.en.srt"),
            "1\n00:00:00,000 --> 00:00:01,000\nHi\n",
        )
        .unwrap();
        let size = std::fs::metadata(&clip).unwrap().len() as i64;
        crate::library::sync_config(
            &pool,
            &[crate::library::LibraryConfig {
                name: "movies".into(),
                kind: crate::library::LibraryKind::Movies,
                path: artwork.path().into(),
            }],
        )
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                     VALUES (1, 1, 'clip.mkv', 'Clip', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
                "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                     first_seen_generation, seen_generation) VALUES (1, 1, 'clip.mkv', ?, 1, 1, 1, 1)",
            )
            .bind(size)
            .execute(&pool)
            .await
            .unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::get("/api/v1/files/1/subtitles")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let tracks: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "SubtitleTrack", &tracks[0]);
        assert_shape(&spec, "SubtitleTrack", &tracks[1]);
    }

    #[tokio::test]
    async fn import_responses_carry_exactly_the_documented_fields() {
        use crate::import::testing::FakeArr;

        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let fake = FakeArr::default();
        fake.reply("/api/v3/qualityprofile", json!([]))
            .reply(
                "/api/v3/movie",
                json!([{ "id": 1, "title": "Dune", "tmdbId": 438631, "monitored": true }]),
            )
            .reply(
                "/api/v3/series",
                json!([{ "id": 2, "title": "Frieren", "tvdbId": 424536, "monitored": true,
                             "seasons": [{ "seasonNumber": 1, "monitored": true }] }]),
            )
            .reply(
                "/api/v3/episode?seriesId=2",
                json!([{ "seasonNumber": 1, "episodeNumber": 2, "monitored": false }]),
            );
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();

        fake.reply("/api/v3/system/status", json!({ "appName": "Radarr" }));
        let url = fake.start().await;
        let body = json!({ "url": url, "api_key": "key" });
        assert_shape(
            &spec,
            "RadarrReport",
            &send("POST", "/api/v1/import/radarr", Some(body)).await,
        );
        assert_shape(
            &spec,
            "WantedMovie",
            &send("GET", "/api/v1/wanted/movies", None).await[0],
        );

        let sonarr = FakeArr::default();
        for (path, reply) in fake.replies.lock().unwrap().iter() {
            sonarr.reply(path, reply.clone());
        }
        sonarr
            .replies
            .lock()
            .unwrap()
            .retain(|(path, _)| path != "/api/v3/system/status");
        sonarr.reply("/api/v3/system/status", json!({ "appName": "Sonarr" }));
        let url = sonarr.start().await;
        let body = json!({ "url": url, "api_key": "key" });
        assert_shape(
            &spec,
            "SonarrReport",
            &send("POST", "/api/v1/import/sonarr", Some(body)).await,
        );
        let series = send("GET", "/api/v1/wanted/series", None).await;
        assert_shape(&spec, "WantedSeries", &series[0]);
        assert_shape(&spec, "WantedSeason", &series[0]["seasons"][0]);
        assert_shape(
            &spec,
            "EpisodeNumber",
            &series[0]["unmonitored_episodes"][0],
        );
    }

    #[tokio::test]
    async fn profile_responses_carry_exactly_the_documented_fields() {
        use crate::import::testing::FakeArr;

        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        for (app_name, captured) in [
            ("Radarr", include_str!("../../tests/fixtures/radarr.json")),
            ("Sonarr", include_str!("../../tests/fixtures/sonarr.json")),
        ] {
            let captured: Value = serde_json::from_str(captured).unwrap();
            let fake = FakeArr::default();
            fake.reply("/api/v3/system/status", json!({ "appName": app_name }))
                .reply("/api/v3/language", captured["languages"].clone())
                .reply("/api/v3/customformat", captured["customformats"].clone())
                .reply(
                    "/api/v3/qualityprofile",
                    captured["qualityprofiles"].clone(),
                );
            let body = json!({ "url": fake.start().await, "api_key": "key" });
            let uri = format!("/api/v1/import/{}/profiles", app_name.to_lowercase());
            assert_shape(
                &spec,
                "ProfileImportReport",
                &send("POST", &uri, Some(body)).await,
            );
        }
        let profiles = send("GET", "/api/v1/profiles", None).await;
        for profile in profiles.as_array().unwrap() {
            assert_shape(&spec, "QualityProfile", profile);
            assert_shape(&spec, "ProfileItem", &profile["items"][0]);
            assert_shape(&spec, "ProfileFormatScore", &profile["formats"][0]);
        }
        let variants = spec["components"]["schemas"]["FormatCondition"]["oneOf"]
            .as_array()
            .unwrap();
        let mut kinds_seen = BTreeSet::new();
        for format in send("GET", "/api/v1/custom-formats", None)
            .await
            .as_array()
            .unwrap()
        {
            assert_shape(&spec, "CustomFormat", format);
            for condition in format["conditions"].as_array().unwrap() {
                let kind = condition["kind"].as_str().unwrap();
                let variant = variants
                    .iter()
                    .find(|v| v["properties"]["kind"]["const"] == kind)
                    .unwrap_or_else(|| panic!("no FormatCondition for {kind}"));
                let documented: BTreeSet<&str> = variant["properties"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                let actual: BTreeSet<&str> = condition
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                assert_eq!(actual, documented, "{kind} condition");
                kinds_seen.insert(kind.to_owned());
            }
        }
        assert!(kinds_seen.len() >= 8, "{kinds_seen:?}");
        let id = profiles[0]["id"].as_i64().unwrap();
        let evaluation = send(
            "POST",
            &format!("/api/v1/profiles/{id}/evaluate"),
            Some(json!({ "name": "Movie.2023.2160p.WEB-DL.DV.H.265-GRP",
                             "existing": "Movie.2023.1080p.WEB-DL.H.264-GRP" })),
        )
        .await;
        assert_shape(&spec, "Evaluation", &evaluation);
        assert_shape(&spec, "Decision", &evaluation["decision"]);
        assert_shape(
            &spec,
            "MatchedFormat",
            &evaluation["decision"]["formats"][0],
        );
        assert_shape(&spec, "ExistingFile", &evaluation["existing"]);
        assert_shape(
            &spec,
            "QualityProfile",
            &send("GET", &format!("/api/v1/profiles/{id}"), None).await,
        );
    }

    #[tokio::test]
    async fn indexer_responses_carry_exactly_the_documented_fields() {
        use crate::import::testing::FakeArr;

        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let listed: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/prowlarr/indexers.json"))
                .unwrap();
        let fake = FakeArr::default();
        fake.reply("/api/v1/system/status", json!({ "appName": "Prowlarr" }))
            .reply("/api/v1/indexer", listed);
        let body = json!({ "url": fake.start().await, "api_key": "key" });
        assert_shape(
            &spec,
            "ProwlarrReport",
            &send("POST", "/api/v1/import/prowlarr", Some(body)).await,
        );
        assert_shape(
            &spec,
            "Indexer",
            &send("GET", "/api/v1/indexers", None).await[0],
        );
        // Prowlarr's listing has one indexer turned off: a note under Indexers.
        let health = send("GET", "/api/v1/admin/health", None).await;
        assert_shape(&spec, "AdminHealth", &health);
        assert_shape(&spec, "AdminHealthArea", &health["areas"][3]);
        assert_shape(
            &spec,
            "AdminHealthFinding",
            &health["areas"][3]["findings"][0],
        );
    }

    #[tokio::test]
    async fn search_responses_carry_exactly_the_documented_fields() {
        use crate::profiles::{App, import::tests::converted, save};

        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        save(&pool, App::Radarr, &converted(App::Radarr))
            .await
            .unwrap();
        let feed = axum::Router::new()
            .fallback(|| async { include_str!("../../tests/fixtures/prowlarr/search.xml") });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/1/api", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, feed).await.unwrap() });
        sqlx::query(
            "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                     tv_search_params, source, source_id, added_at)
                 VALUES ('Fake', ?, 'k', 1, '[2000]', '[\"q\"]', '[]', 'prowlarr', 1, 0)",
        )
        .bind(url)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wanted_movies (tmdb_id, title, year, monitored, quality_profile, source,
                     added_at, original_language)
                 VALUES (693134, 'Dune: Part Two', 2024, 1, '4K Preferred', 'radarr', 0, 'en')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let send = |method: &str, uri: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let search = send("POST", "/api/v1/wanted/movies/1/search").await;
        assert_shape(&spec, "Search", &search);
        assert_shape(&spec, "IndexerOutcome", &search["indexers"][0]);
        assert_shape(&spec, "ReleaseResult", &search["releases"][0]);
        assert_shape(&spec, "Decision", &search["releases"][0]["decision"]);
        let again = send("GET", &format!("/api/v1/searches/{}", search["id"])).await;
        assert_eq!(again, search);
    }

    #[tokio::test]
    async fn rss_syncs_carry_exactly_the_documented_fields() {
        use crate::profiles::{App, import::tests::converted, save};

        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        save(&pool, App::Radarr, &converted(App::Radarr))
            .await
            .unwrap();
        let feed = axum::Router::new()
            .fallback(|| async { include_str!("../../tests/fixtures/prowlarr/search.xml") });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/1/api", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, feed).await.unwrap() });
        sqlx::query(
                "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                     tv_search_params, source, source_id, added_at)
                 VALUES ('Fake', ?, 'k', 1, '[2000]', '[\"q\"]', '[]', 'prowlarr', 1, 0);
                 INSERT INTO wanted_movies (tmdb_id, title, year, monitored, quality_profile, source,
                     added_at, original_language)
                 VALUES (693134, 'Dune: Part Two', 2024, 1, '4K Preferred', 'radarr', 0, 'en')",
            )
            .bind(url)
            .execute(&pool)
            .await
            .unwrap();
        let send = |method: &str, uri: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                assert!(response.status().is_success(), "{}", response.status());
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let sync = send("POST", "/api/v1/rss/sync").await;
        assert_shape(&spec, "RssSync", &sync);
        assert_shape(&spec, "RssSyncSearch", &sync["searches"][0]);
        let grab = crate::rss::Grabbed {
            release_id: 1,
            title: "Some.Movie".into(),
            upgrade: false,
            download_id: None,
            refused: Some("it's already download 3".into()),
        };
        assert_shape(&spec, "RssGrab", &serde_json::to_value(grab).unwrap());
        let listed = send("GET", "/api/v1/rss/syncs?limit=5").await;
        assert_eq!(listed[0], sync);
        assert_eq!(
            send("GET", &format!("/api/v1/rss/syncs/{}", sync["id"])).await,
            sync
        );
        let search = send(
            "GET",
            &format!("/api/v1/searches/{}", sync["searches"][0]["search_id"]),
        )
        .await;
        assert_eq!(search["rss_sync_id"], sync["id"]);

        let status = |method: &str, uri: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap().status() }
        };
        assert_eq!(
            status("GET", "/api/v1/rss/syncs?limit=0").await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status("GET", "/api/v1/rss/syncs/999").await,
            StatusCode::NOT_FOUND
        );
        sqlx::query("UPDATE indexers SET enabled = 0")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            status("POST", "/api/v1/rss/sync").await,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn the_calendar_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let user = crate::auth::create_user(&pool, "viewer", "viewer-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO tmdb_movies (tmdb_id, title, release_date, digital_release, fetched_at)
                 VALUES (1, 'Some Film', '2026-09-01', '2026-11-03', 0);
                 INSERT INTO wanted_movies (tmdb_id, title, year, monitored, source, added_at)
                 VALUES (1, 'Some Film', 2026, 1, 'radarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let get = |uri: &str| {
            let request = Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap())
            }
        };
        let (status, body) = get("/api/v1/calendar?start=2026-11-01&end=2026-11-30").await;
        assert_eq!(status, StatusCode::OK);
        let spec = spec();
        assert_shape(&spec, "Calendar", &body);
        assert_shape(&spec, "CalendarEntry", &body["entries"][0]);
        assert_eq!(body["entries"][0]["kind"], "digital");
        let (status, _) = get("/api/v1/calendar?start=2026-11-01&end=2027-06-01").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) = get("/api/v1/calendar").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["start"].is_string() && body["end"].is_string());
    }

    #[tokio::test]
    async fn emptying_the_trash_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        // A library whose share is away: its file is left, and counted.
        sqlx::query(
                "INSERT INTO libraries (id, name, kind, path) VALUES (9, 'm', 'movies', '/nowhere');
                 INSERT INTO trashed_files (library_id, rel_path, trash_path, size, replaced_by, trashed_at)
                 VALUES (9, 'A/a.mkv', '.melete-trash/1/A/a.mkv', 5, 'A/a.2160p.mkv', 100)",
            )
            .execute(&pool)
            .await
            .unwrap();
        let send = |method: &str, uri: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (status, bytes)
            }
        };
        assert_eq!(
            send("DELETE", "/api/v1/trash/99").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send("DELETE", "/api/v1/trash/1").await.0,
            StatusCode::CONFLICT
        );
        let (status, bytes) = send("DELETE", "/api/v1/trash").await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "TrashEmptied", &body);
        assert_eq!(body, serde_json::json!({ "deleted": 0, "left": 1 }));
    }

    #[tokio::test]
    async fn the_trash_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
                "INSERT INTO libraries (id, name, kind, path) VALUES (9, 'm', 'movies', '/nowhere');
                 INSERT INTO trashed_files (library_id, rel_path, trash_path, size, replaced_by, trashed_at)
                 VALUES (9, 'A/a.mkv', '.melete-trash/1/A/a.mkv', 5, 'A/a.2160p.mkv', 100)",
            )
            .execute(&pool)
            .await
            .unwrap();
        let send = |method: &str, uri: &str| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (status, bytes)
            }
        };
        let (status, bytes) = send("GET", "/api/v1/trash").await;
        assert_eq!(status, StatusCode::OK);
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "TrashedFile", &body[0]);
        assert_eq!(
            send("POST", "/api/v1/trash/99/restore").await.0,
            StatusCode::NOT_FOUND
        );
        // The file isn't there to move back.
        assert_eq!(
            send("POST", &format!("/api/v1/trash/{}/restore", body[0]["id"]))
                .await
                .0,
            StatusCode::CONFLICT
        );
    }

    #[tokio::test]
    async fn a_wanted_title_keeps_a_profile_chosen_in_melete() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
                "INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, quality_profile, source, added_at)
                 VALUES (1, 424536, 'Frieren', 1, 'anime', '4K Preferred', 'sonarr', 0),
                        (2, 81189, 'Breaking Bad', 1, 'standard', '4K Preferred', 'sonarr', 0);
                 INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
                 VALUES (1, 129, 'Spirited Away', 1, 'radarr', 0)",
            )
            .execute(&pool)
            .await
            .unwrap();
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };
        let (status, series_profile) = send(
            "POST",
            "/api/v1/profiles/presets/anime",
            Some(json!({ "source": "sonarr", "apply_to_anime": true })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let spec = spec();
        assert_shape(&spec, "QualityProfile", &series_profile);
        let (_, wanted) = send("GET", "/api/v1/wanted/series", None).await;
        assert_shape(&spec, "WantedSeries", &wanted[0]);
        let chosen: Vec<(String, Value)> = wanted
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["title"].as_str().unwrap().to_owned(),
                    w["profile_id"].clone(),
                )
            })
            .collect();
        assert_eq!(
            chosen,
            [
                ("Breaking Bad".to_owned(), Value::Null),
                ("Frieren".to_owned(), series_profile["id"].clone())
            ],
            "only the anime series"
        );
        assert_eq!(
            send(
                "POST",
                "/api/v1/profiles/presets/anime",
                Some(json!({ "source": "sonarr" }))
            )
            .await
            .0,
            StatusCode::CONFLICT
        );

        let movie = "/api/v1/wanted/movies/1/profile";
        let (status, _) = send(
            "PUT",
            movie,
            Some(json!({ "profile_id": series_profile["id"] })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "a series profile for a movie"
        );
        let (_, movie_profile) = send(
            "POST",
            "/api/v1/profiles/presets/anime",
            Some(json!({ "source": "radarr" })),
        )
        .await;
        let (status, _) = send(
            "PUT",
            movie,
            Some(json!({ "profile_id": movie_profile["id"] })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, movies) = send("GET", "/api/v1/wanted/movies", None).await;
        assert_shape(&spec, "WantedMovie", &movies[0]);
        assert_eq!(movies[0]["profile_id"], movie_profile["id"]);
        assert_eq!(
            send(
                "PUT",
                "/api/v1/wanted/movies/9/profile",
                Some(json!({ "profile_id": null }))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let (status, _) = send("PUT", movie, Some(json!({ "profile_id": null }))).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn import_lists_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        crate::profiles::save(
            &pool,
            crate::profiles::App::Radarr,
            &crate::profiles::import::tests::converted(crate::profiles::App::Radarr),
        )
        .await
        .unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (9, 'm', 'movies', '/m')")
            .execute(&pool)
            .await
            .unwrap();
        let send = |method: &str, uri: &str, body: Option<Value>| {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let status = response.status();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (
                    status,
                    serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
                )
            }
        };
        let input = json!({ "name": "Popular", "provider": "tmdb", "list": "popular", "kind": "movies",
                                "library_id": 9, "monitored": true, "quality_profile": "4K Preferred", "max_items": 20,
                                "monitor_seasons": "latest", "enabled": true });
        let (status, list) = send("POST", "/api/v1/lists", Some(input.clone())).await;
        assert_eq!(status, StatusCode::CREATED);
        let spec = spec();
        assert_shape(&spec, "ImportList", &list);
        assert_eq!(list["monitor_seasons"], "latest");
        let (_, all) = send("GET", "/api/v1/lists", None).await;
        assert_eq!(all[0], list);
        assert_eq!(
            send("POST", "/api/v1/lists", Some(input.clone())).await.0,
            StatusCode::CONFLICT
        );
        let mut bad = input.clone();
        bad["list"] = json!("on_the_air");
        assert_eq!(
            send("POST", "/api/v1/lists", Some(bad)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        // Without a TMDB token lists are kept but can't sync.
        let uri = format!("/api/v1/lists/{}", list["id"]);
        assert_eq!(
            send("POST", &format!("{uri}/sync"), None).await.0,
            StatusCode::CONFLICT
        );
        let mut changed = input;
        changed["max_items"] = json!(5);
        let (status, updated) = send("PUT", &uri, Some(changed)).await;
        assert_eq!(
            (status, updated["max_items"].clone()),
            (StatusCode::OK, json!(5))
        );
        assert_eq!(send("DELETE", &uri, None).await.0, StatusCode::NO_CONTENT);
        assert_eq!(send("DELETE", &uri, None).await.0, StatusCode::NOT_FOUND);
        let result = serde_json::to_value(crate::lists::SyncResult {
            found: 2,
            added: 1,
            already_wanted: 0,
            skipped: vec![crate::lists::Skipped {
                title: "x".into(),
                why: "y".into(),
            }],
        })
        .unwrap();
        assert_shape(&spec, "ListSyncResult", &result);
    }

    #[tokio::test]
    async fn downloads_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
                "INSERT INTO downloads (infohash, title, decision, state, added_at, updated_at)
                 VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Some.Movie', '{}', 'downloading', 0, 0)",
            )
            .execute(&pool)
            .await
            .unwrap();
        let request = Request::get("/api/v1/downloads")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "Download", &body[0]);
    }

    #[tokio::test]
    async fn progress_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
                "INSERT INTO downloads (infohash, title, decision, state, progress, added_at, updated_at)
                 VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Some.Movie', '{}', 'downloading', 10, 0, strftime('%s', 'now'))",
            )
            .execute(&pool)
            .await
            .unwrap();
        let request = Request::get("/api/v1/progress")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "WantedProgress", &body[0]);
    }

    #[tokio::test]
    async fn requests_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
                "INSERT INTO requests (id, user_id, kind, tmdb_id, title, year, poster, state, created_at)
                 VALUES (5, ?, 'movie', 603, 'The Matrix', 1999, '/matrix.jpg', 'requested', 0)",
            )
            .bind(admin.id)
            .execute(&pool)
            .await
            .unwrap();
        let get = |uri: &'static str| {
            let (app, token) = (app.clone(), token.clone());
            async move {
                let request = Request::get(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap();
                let response = app.oneshot(request).await.unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        assert_shape(&spec, "Request", &get("/api/v1/requests/5").await);
        let page = get("/api/v1/requests").await;
        assert_shape(&spec, "RequestPage", &page);
        let settings = get("/api/v1/requests/settings").await;
        assert_shape(&spec, "RequestSettings", &settings);
        assert_shape(&spec, "RequestDefaults", &settings["defaults"][0]);
    }

    #[tokio::test]
    async fn a_series_requests_seasons_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
                "INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, source, added_at)
                     VALUES (3, 30, 1399, 'Game of Thrones', 1, 'standard', 'manual', 0);
                 INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (3, 1, 1), (3, 2, 0);
                 INSERT INTO requests (id, user_id, kind, tmdb_id, season, title, state, wanted_series_id, created_at, decided_at)
                     VALUES (6, ?, 'series', 1399, 1, 'Game of Thrones', 'approved', 3, 0, 0)",
            )
            .bind(user.id)
            .execute(&pool)
            .await
            .unwrap();
        let request = Request::get("/api/v1/requests/6")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "Request", &body);
        assert_eq!(body["seasons"].as_array().map(Vec::len), Some(2));
        assert_shape(&spec, "RequestSeason", &body["seasons"][0]);
    }

    #[tokio::test]
    async fn playlists_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let ops = [
                r#"{"opId":"p","entity":"playlists","entityId":"p","operation":"upsert","payload":{"id":"p","name":"Mix","updatedAt":1},"createdAt":1}"#,
                r#"{"opId":"i","entity":"playlist_items","entityId":"i","operation":"upsert","payload":{"id":"i","playlistId":"p","jellyfinId":"1","contentKey":"a|b|c|1","position":"a0","updatedAt":1},"createdAt":1}"#,
            ]
            .iter()
            .map(|op| {
                crate::aoide::sync::validate(
                    &serde_json::value::RawValue::from_string((*op).to_owned()).unwrap(),
                )
                .unwrap()
            })
            .collect();
        crate::aoide::sync::push(&pool, user.id, "dev", ops, 1)
            .await
            .unwrap();
        let app = playlists::routes(pool);
        let get = |uri: &'static str| {
            let (app, token) = (app.clone(), token.clone());
            async move {
                let request = Request::get(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap();
                let response = app.oneshot(request).await.unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        assert_shape(&spec, "Playlist", &get("/api/v1/playlists").await[0]);
        let one = get("/api/v1/playlists/p").await;
        assert_shape(&spec, "PlaylistDetail", &one);
        assert_shape(&spec, "PlaylistItem", &one["items"][0]);
    }

    #[tokio::test]
    async fn outside_services_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let admin = crate::auth::create_user(&pool, "admin", "a-long-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = services::routes(pool, tokio::sync::mpsc::channel(1).0);
        let request = Request::get("/api/v1/settings/services")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let spec = spec();
        assert_shape(&spec, "OutsideService", &body[0]);
        assert_shape(&spec, "OutsideServiceField", &body[0]["fields"][0]);
        // A toggle as documented: {"toggle": {"on": …}}.
        let lyrics = body
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["key"] == "lyrics")
            .unwrap();
        assert_eq!(
            lyrics["fields"][0]["input"],
            serde_json::json!({"toggle": {"on": "lrclib"}})
        );
    }

    #[tokio::test]
    async fn playlist_imports_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let tracks =
            crate::playlist_import::parse("Track Name,Artist Name(s)\nCreep,Radiohead\n").unwrap();
        let made = crate::playlist_import::create(&pool, &user, "Mine", &tracks)
            .await
            .unwrap();
        let app = requests::routes(pool, None, crate::fulfil::Sources::default());
        let get = |uri: String| {
            let (app, token) = (app.clone(), token.clone());
            async move {
                let request = Request::get(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap();
                let response = app.oneshot(request).await.unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let list = get("/api/v1/requests/imports".into()).await;
        assert_shape(&spec, "PlaylistImportSummary", &list[0]);
        let one = get(format!("/api/v1/requests/imports/{}", made.id)).await;
        assert_shape(&spec, "PlaylistImport", &one);
        assert_shape(&spec, "PlaylistImportTrack", &one["items"][0]);
    }

    #[tokio::test]
    async fn request_search_results_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let (mb, _) = crate::musicbrainz::testing::start(0).await;
        let app = requests::routes(pool, Some(Arc::new(mb)), crate::fulfil::Sources::default());
        let request = Request::get("/api/v1/requests/search?kind=song&q=Paranoid%20Android")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "RequestCandidate", &body[0]);
    }

    #[tokio::test]
    async fn parsed_releases_carry_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let request = Request::get("/api/v1/releases/parse?name=Show.S01E01.1080p.WEB-DL-GRP")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "ParsedRelease", &body);
    }

    #[tokio::test]
    async fn missing_searches_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let sources = crate::search_missing::Sources {
            tmdb: None,
            slskd: None,
            pipeline: None,
            pace: std::time::Duration::ZERO,
        };
        let app = missing_searches::routes(pool, sources);
        let request = Request::post("/api/v1/missing-searches")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"area":"series"}"#))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "MissingSearch", &body);
    }

    #[tokio::test]
    async fn removed_titles_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO wanted_removed (kind, key, title, removed_at, removed_by)
                 VALUES ('movie', '438631', 'Dune', 1, ?)",
        )
        .bind(admin.id)
        .execute(&pool)
        .await
        .unwrap();
        let app = wanted_removed::routes(pool, None, None);
        let request = Request::get("/api/v1/wanted/removed")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "WantedRemoval", &body[0]);
    }

    #[tokio::test]
    async fn managed_lists_carry_exactly_the_documented_fields() {
        let pool = crate::manage::scale::big_library().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = manage::routes(pool);
        let spec = spec();
        for (list, page, item) in [
            ("movies", "ManagedMoviePage", "ManagedMovie"),
            ("series", "ManagedSeriesPage", "ManagedSeries"),
            ("anime", "ManagedSeriesPage", "ManagedSeries"),
            ("artists", "ManagedArtistPage", "ManagedArtist"),
        ] {
            let request = Request::get(format!("/api/v1/manage/{list}?sort=added"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_shape(&spec, page, &body);
            assert_shape(&spec, "ManagedCounts", &body["counts"]);
            assert_shape(&spec, item, &body["items"][0]);
        }
    }

    #[tokio::test]
    async fn system_status_carries_exactly_the_documented_fields() {
        let (pool, app, _artwork) = full_server().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let spec = spec();
        for (uri, schema) in [
            ("/api/v1/admin/system/disk", "DiskSpace"),
            ("/api/v1/admin/system/tasks", "BackgroundTask"),
        ] {
            let request = Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_shape(&spec, schema, &body[0]);
        }
    }

    #[tokio::test]
    async fn a_selections_outcome_carries_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
                 VALUES (1, 603, 'The Matrix', 1, 'manual', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let body = serde_json::json!({
            "action": "unmonitor", "wanted_ids": [1, 2], "folders": [], "profile_id": null
        });
        let request = Request::post("/api/v1/manage/movies/selection")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = manage::selection_routes(pool, None, None)
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let outcome: Value = serde_json::from_slice(&bytes).unwrap();
        assert_shape(&spec(), "ManagedSelectionOutcome", &outcome);
        assert_eq!(outcome["unchanged"][0]["wanted_id"], 2);
    }

    #[tokio::test]
    async fn histories_carry_exactly_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let admin = crate::auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, admin.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::raw_sql(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
                 VALUES (1, 603, 'The Matrix', 1, 'manual', 0);
             INSERT INTO searches (wanted_movie_id, searched_at, indexers)
                 VALUES (1, strftime('%s', 'now'), '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let app = manage::routes(pool);
        let get = |uri: &'static str| {
            let request = Request::get(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let app = app.clone();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap()
            }
        };
        let spec = spec();
        let page = get("/api/v1/manage/movies/history?wanted_id=1").await;
        assert_shape(&spec, "TitleEventPage", &page);
        assert_shape(&spec, "TitleEvent", &page["items"][0]);
        let searches = get("/api/v1/manage/movies/searches?wanted_id=1").await;
        assert_shape(&spec, "PastSearch", &searches[0]);
    }

    #[test]
    fn every_schema_requires_all_its_fields_except_optional_inputs() {
        let spec = spec();
        for (name, schema) in spec["components"]["schemas"].as_object().unwrap() {
            let Some(properties) = schema["properties"].as_object() else {
                continue;
            };
            let optional_fields = [
                "NewUser",
                "ProgressReport",
                "TranscodeRequest",
                "ImportRequest",
                "EvaluateRequest",
                "SeriesSearchRequest",
                "NewWantedSong",
                "NewRequest",
                "RequestChoice",
                "NewKeyRequest",
            ];
            if optional_fields.contains(&name.as_str()) {
                continue; // Inputs with optional fields.
            }
            let required: BTreeSet<&str> = schema["required"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} lists no required fields"))
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            let all: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
            assert_eq!(required, all, "{name}: every field is always present");
        }
    }
}

mod access;
mod access_walk;
mod scopes;
