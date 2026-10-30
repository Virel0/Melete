use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;

struct Fixture {
    pool: SqlitePool,
    app: Router,
}

/// A shows library, an anime library and a movies library. Film A (matched, with its art, a
/// textless backdrop, a logo and a focal point) came in at 100, Film B (unmatched) at 300 and
/// a gone film last. Show (matched) has S01E01 from 50, S01E02-E03 in one file from 400 and a
/// special; Wanted Anime sits among the shows but is wanted as anime (200); Anime is in the
/// anime library (250).
async fn fixture() -> Fixture {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        r#"INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'shows', 'shows', '/s', NULL), (2, 'anime', 'shows', '/a', 'anime'),
             (3, 'movies', 'movies', '/m', NULL);
         INSERT INTO tmdb_movies (tmdb_id, title, year, overview, runtime_minutes, genres,
             poster_path, backdrop_path, textless_backdrop_path, logo_path, fetched_at)
             VALUES (603, 'Film A', 1999, 'A hacker learns the truth.', 136, '["Action"]',
                 '/a.jpg', '/a-back.jpg', '/a-clean.jpg', '/a-logo.png', 0);
         INSERT INTO artwork_focus (file, x, y) VALUES ('/a-back.jpg', 0.25, 0.5);
         INSERT INTO tmdb_series (tmdb_id, name, year, overview, genres, poster_path, fetched_at)
             VALUES (10, 'Show', 2020, 'Things happen.', '["Drama"]', '/show.jpg', 0);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, still_path)
             VALUES (10, 1, 2, 102, 'Two', '/s1e2.jpg');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
             match_status, match_tmdb_id, missing_since) VALUES
             (1, 3, 'Film A (1999)', 'Film A', 1999, 1, 'matched', 603, NULL),
             (2, 3, 'Film B (2001)', 'Film B', 2001, 1, NULL, NULL, NULL),
             (3, 3, 'Gone (2002)', 'Gone', 2002, 1, NULL, NULL, 5);
         INSERT INTO series (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
             match_status, match_tmdb_id) VALUES
             (1, 1, 'Show', 'Show', NULL, 1, 'matched', 10),
             (2, 1, 'Wanted Anime', 'Wanted Anime', 2010, 1, NULL, NULL),
             (3, 2, 'Anime', 'Anime', 2022, 1, NULL, NULL);
         INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, source, added_at,
             series_id) VALUES (1, 'Wanted Anime', 1, 'anime', 'manual', 0, 2);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
             movie_id, series_id, season, episode, episode_end, first_seen_generation,
             seen_generation, missing_since) VALUES
             (1, 1, 'Show/S01E01.mkv', 1, 1, 50, NULL, 1, 1, 1, NULL, 1, 1, NULL),
             (2, 1, 'Show/S01E02E03.mkv', 1, 1, 400, NULL, 1, 1, 2, 3, 1, 1, NULL),
             (3, 1, 'Show/S00E01.mkv', 1, 1, 60, NULL, 1, 0, 1, NULL, 1, 1, NULL),
             (4, 1, 'Wanted Anime/S01E01.mkv', 1, 1, 200, NULL, 2, 1, 1, NULL, 1, 1, NULL),
             (5, 2, 'Anime/S01E01.mkv', 1, 1, 250, NULL, 3, 1, 1, NULL, 1, 1, NULL),
             (6, 3, 'Film A (1999)/a.mkv', 1, 1, 100, 1, NULL, NULL, NULL, NULL, 1, 1, NULL),
             (7, 3, 'Film B (2001)/b.mkv', 1, 1, 300, 2, NULL, NULL, NULL, NULL, 1, 1, NULL),
             (8, 3, 'Gone (2002)/g.mkv', 1, 1, 900, 3, NULL, NULL, NULL, NULL, 1, 1, 5);"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = routes(pool.clone(), ArtworkState::stored_only(pool.clone()));
    Fixture { pool, app }
}

async fn user(f: &Fixture, name: &str) -> (i64, String) {
    let user = auth::create_user(&f.pool, name, "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = auth::issue_token(&f.pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap();
    (user.id, token)
}

/// Records `file` as watched by `user`, or `position` seconds in, at `at`.
async fn progress(f: &Fixture, user: i64, file: i64, position: f64, at: i64) {
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
             watched, play_count, updated_at) VALUES (?1, ?2, ?3, 6000, ?3 = 0, ?3 = 0, ?4)",
    )
    .bind(user)
    .bind(file)
    .bind(position)
    .bind(at)
    .execute(&f.pool)
    .await
    .unwrap();
}

async fn home_of(f: &Fixture, token: &str) -> Value {
    let request = Request::get("/api/v1/home")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// `kind:id` of each item, to compare orders.
fn titles(items: &Value) -> Vec<String> {
    items
        .as_array()
        .unwrap()
        .iter()
        .map(|i| format!("{}:{}", i["kind"].as_str().unwrap(), i["id"]))
        .collect()
}

fn row<'a>(home: &'a Value, key: &str) -> &'a Value {
    home["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["key"] == key)
        .map(|r| &r["items"])
        .unwrap_or_else(|| panic!("no {key} row in {home}"))
}

fn keys(home: &Value) -> Vec<&str> {
    home["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["key"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn rows_hold_their_titles_in_order_with_anime_apart() {
    let f = fixture().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 1, 0.0, 10).await;
    progress(&f, me, 7, 600.0, 20).await;
    let home = home_of(&f, &token).await;
    assert_eq!(
        keys(&home),
        [
            "continue_watching",
            "next_up",
            "latest_movies",
            "latest_shows",
            "latest_anime"
        ]
    );
    assert_eq!(titles(row(&home, "continue_watching")), ["movie:2"]);
    assert_eq!(titles(row(&home, "next_up")), ["series:1"]);
    // Wanted as anime, or in the anime library: never among the shows.
    assert_eq!(titles(row(&home, "latest_shows")), ["series:1"]);
    assert_eq!(
        titles(row(&home, "latest_anime")),
        ["series:3", "series:2"],
        "newest first"
    );
    assert_eq!(
        titles(row(&home, "latest_movies")),
        ["movie:2", "movie:1"],
        "a gone film is left out"
    );
    // The hero: what's being watched, then the newest of everything, each title once.
    assert_eq!(
        titles(&home["hero"]),
        ["movie:2", "series:1", "series:3", "series:2", "movie:1"]
    );
}

#[tokio::test]
async fn each_item_carries_its_art_facts_and_progress() {
    let f = fixture().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 1, 0.0, 10).await;
    progress(&f, me, 7, 600.0, 20).await;
    let home = home_of(&f, &token).await;
    let film_a = &row(&home, "latest_movies")[1];
    assert_eq!(
        film_a,
        &json!({
            "kind": "movie", "id": 1, "title": "Film A", "year": 1999, "genres": ["Action"],
            "runtime_minutes": 136, "overview": "A hacker learns the truth.",
            "poster": "/api/v1/artwork/w342/a.jpg",
            "backdrop": "/api/v1/artwork/w1280/a-back.jpg",
            "backdrop_focus": { "x": 0.25, "y": 0.5 },
            "textless_backdrop": "/api/v1/artwork/w1280/a-clean.jpg",
            "textless_backdrop_focus": null,
            "logo": "/api/v1/artwork/w500/a-logo.png",
            "added_at": 100, "unwatched_episodes": null, "episode": null, "progress": null,
        })
    );
    // Partway through: the same progress wherever the film is shown.
    let partway = json!({ "file_id": 7, "position_seconds": 600.0, "duration_seconds": 6000.0,
                          "watched": false, "updated_at": 20 });
    assert_eq!(row(&home, "continue_watching")[0]["progress"], partway);
    assert_eq!(row(&home, "latest_movies")[0]["progress"], partway);
    assert_eq!(home["hero"][0]["progress"], partway);
    // The next episode, and the two on disk not watched (one file holds both; the special
    // doesn't count).
    let show = &row(&home, "next_up")[0];
    assert_eq!(
        show["episode"],
        json!({ "file_id": 2, "season": 1, "episode": 2, "name": "Two",
                "still": "/api/v1/artwork/w300/s1e2.jpg" })
    );
    assert_eq!(show["unwatched_episodes"], 2);
    assert_eq!(show["added_at"], 50, "when its first file came in");
    assert_eq!(show["progress"], Value::Null);
    assert_eq!(show["runtime_minutes"], Value::Null);
    let other = &row(&home, "latest_anime")[1];
    assert_eq!(
        (
            &other["title"],
            &other["unwatched_episodes"],
            &other["poster"]
        ),
        (&json!("Wanted Anime"), &json!(1), &Value::Null)
    );
}

#[tokio::test]
async fn a_watched_film_says_so_and_an_episode_partway_is_its_series() {
    let f = fixture().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 6, 0.0, 10).await;
    progress(&f, me, 4, 30.0, 30).await;
    let home = home_of(&f, &token).await;
    let film_a = &row(&home, "latest_movies")[1];
    assert_eq!(film_a["progress"]["watched"], true);
    let watching = &row(&home, "continue_watching")[0];
    assert_eq!(
        (
            &watching["kind"],
            &watching["id"],
            &watching["episode"]["file_id"]
        ),
        (&json!("series"), &json!(2), &json!(4))
    );
    assert_eq!(watching["progress"]["position_seconds"], 30.0);
}

#[tokio::test]
async fn favourites_come_after_next_up_newest_first() {
    let f = fixture().await;
    let (me, token) = user(&f, "viewer").await;
    progress(&f, me, 1, 0.0, 10).await;
    progress(&f, me, 6, 600.0, 20).await;
    // Film A, then Anime, then the gone film last.
    sqlx::query(
        "INSERT INTO favourites (user_id, movie_id, series_id, added_at) VALUES
             (?1, 1, NULL, 5), (?1, NULL, 3, 9), (?1, 3, NULL, 10)",
    )
    .bind(me)
    .execute(&f.pool)
    .await
    .unwrap();
    let home = home_of(&f, &token).await;
    assert_eq!(
        keys(&home),
        [
            "continue_watching",
            "next_up",
            "favourites",
            "latest_movies",
            "latest_shows",
            "latest_anime"
        ]
    );
    let favourites = row(&home, "favourites");
    assert_eq!(
        titles(favourites),
        ["series:3", "movie:1"],
        "a gone film is left out"
    );
    // Each as the other rows have it.
    let film_a = &favourites[1];
    assert_eq!(film_a, &row(&home, "latest_movies")[1]);
    assert_eq!(film_a["progress"]["position_seconds"], 600.0);
    // Someone else's favourites aren't theirs.
    let (_, other) = user(&f, "other").await;
    assert!(!keys(&home_of(&f, &other).await).contains(&"favourites"));
}

#[tokio::test]
async fn empty_rows_are_left_out_and_each_user_sees_their_own() {
    let f = fixture().await;
    let (me, _) = user(&f, "viewer").await;
    progress(&f, me, 7, 600.0, 20).await;
    let (_, other) = user(&f, "other").await;
    let home = home_of(&f, &other).await;
    assert_eq!(
        keys(&home),
        ["latest_movies", "latest_shows", "latest_anime"],
        "nothing to continue or watch next"
    );
    assert_eq!(row(&home, "latest_movies")[0]["progress"], Value::Null);
    assert_eq!(
        titles(&home["hero"]),
        ["series:1", "movie:2", "series:3", "series:2", "movie:1"]
    );

    let pool = db::open_in_memory().await;
    let empty = Fixture {
        app: routes(pool.clone(), ArtworkState::stored_only(pool.clone())),
        pool,
    };
    let (_, token) = user(&empty, "viewer").await;
    assert_eq!(
        home_of(&empty, &token).await,
        json!({ "hero": [], "rows": [] })
    );
}

#[tokio::test]
async fn the_hero_has_at_most_eight_picks() {
    let f = fixture().await;
    for id in 10..20 {
        sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (?1, 3, 'M' || ?1, 'M' || ?1, 1);
             INSERT INTO media_files (library_id, rel_path, size, modified_at, added_at, movie_id,
                 first_seen_generation, seen_generation) VALUES (3, 'm' || ?1, 1, 1, 1000 + ?1, ?1, 1, 1);",
        )
        .bind(id)
        .execute(&f.pool)
        .await
        .unwrap();
    }
    let (_, token) = user(&f, "viewer").await;
    let home = home_of(&f, &token).await;
    assert_eq!(home["hero"].as_array().unwrap().len(), HERO);
    assert_eq!(home["hero"][0]["id"], 19, "newest first");
    assert_eq!(row(&home, "latest_movies").as_array().unwrap().len(), 12);
}

#[tokio::test]
async fn needs_a_signed_in_user() {
    let f = fixture().await;
    let response = f
        .app
        .clone()
        .oneshot(Request::get("/api/v1/home").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
