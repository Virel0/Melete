use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

struct Fixture {
    pool: SqlitePool,
    app: Router,
}

/// Movies 1 (matched to TMDB) and 2, series 1, and movie 3, gone from the library.
async fn fixture() -> Fixture {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'shows', 'shows', '/s');
         INSERT INTO tmdb_movies (tmdb_id, title, year, poster_path, fetched_at)
             VALUES (603, 'The Matrix', 1999, '/p.jpg', 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation,
             match_status, match_tmdb_id, missing_since) VALUES
             (1, 1, 'Matrix', 'Matrix', NULL, 1, 'matched', 603, NULL),
             (2, 1, 'Heat (1995)', 'Heat', 1995, 1, NULL, NULL, NULL),
             (3, 1, 'Gone', 'Gone', NULL, 1, NULL, NULL, 5);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 2, 'Show', 'Show', 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    Fixture {
        app: routes(pool.clone()),
        pool,
    }
}

async fn token(f: &Fixture, name: &str) -> String {
    let user = auth::create_user(&f.pool, name, "user-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    auth::issue_token(&f.pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

async fn send(f: &Fixture, token: &str, method: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The (kind, id) of each favourite listed at `uri`.
async fn listed(f: &Fixture, token: &str, uri: &str) -> Vec<(String, i64)> {
    let (status, body) = send(f, token, "GET", uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap().to_owned(),
                i["id"].as_i64().unwrap(),
            )
        })
        .collect()
}

fn pair(kind: &str, id: i64) -> (String, i64) {
    (kind.into(), id)
}

#[tokio::test]
async fn favourites_are_listed_newest_first_by_kind_and_each_users_own() {
    let f = fixture().await;
    let alice = token(&f, "alice").await;
    for uri in [
        "/api/v1/me/favourites/movies/2",
        "/api/v1/me/favourites/series/1",
        "/api/v1/me/favourites/movies/1",
    ] {
        let (status, body) = send(&f, &alice, "PUT", uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["favourite"], true);
    }
    let all = listed(&f, &alice, "/api/v1/me/favourites").await;
    assert_eq!(all, [pair("movie", 1), pair("series", 1), pair("movie", 2)]);
    let movies = listed(&f, &alice, "/api/v1/me/favourites?kind=movie").await;
    assert_eq!(movies, [pair("movie", 1), pair("movie", 2)]);
    let page = listed(&f, &alice, "/api/v1/me/favourites?limit=1&offset=1").await;
    assert_eq!(page, [pair("series", 1)]);

    let (_, body) = send(&f, &alice, "GET", "/api/v1/me/favourites?kind=movie").await;
    assert_eq!(body["total"], 2);
    assert_eq!(body["version"], 3);
    let matrix = &body["items"][0];
    assert_eq!(
        (&matrix["title"], &matrix["year"], &matrix["matched"]),
        (&json!("The Matrix"), &json!(1999), &json!(true))
    );
    assert_eq!(matrix["poster"], "/api/v1/artwork/w342/p.jpg");
    assert_eq!(body["items"][1]["title"], "Heat");

    let bob = token(&f, "bob").await;
    assert!(listed(&f, &bob, "/api/v1/me/favourites").await.is_empty());
    let (_, state) = send(&f, &bob, "GET", "/api/v1/me/favourites/movies/1").await;
    assert_eq!(state, json!({ "favourite": false, "added_at": null }));
    for wrong in ["?kind=album", "?limit=0", "?offset=-1"] {
        let uri = format!("/api/v1/me/favourites{wrong}");
        assert_eq!(
            send(&f, &alice, "GET", &uri).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}

#[tokio::test]
async fn adding_twice_keeps_the_first_and_removing_is_safe_to_repeat() {
    let f = fixture().await;
    let alice = token(&f, "alice").await;
    let uri = "/api/v1/me/favourites/movies/1";
    send(&f, &alice, "PUT", uri).await;
    sqlx::query("UPDATE favourites SET added_at = 7")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, again) = send(&f, &alice, "PUT", uri).await;
    assert_eq!(again["added_at"], 7);
    assert_eq!(version(&f.pool, 1).await.unwrap(), 1, "nothing changed");
    for _ in 0..2 {
        let (status, body) = send(&f, &alice, "DELETE", uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["favourite"], false);
    }
    assert_eq!(version(&f.pool, 1).await.unwrap(), 2);
    assert!(listed(&f, &alice, "/api/v1/me/favourites").await.is_empty());
}

#[tokio::test]
async fn only_titles_in_the_library_can_be_added_and_a_favourite_outlives_its_title_going() {
    let f = fixture().await;
    let alice = token(&f, "alice").await;
    for uri in [
        "/api/v1/me/favourites/movies/3",
        "/api/v1/me/favourites/movies/99",
        "/api/v1/me/favourites/series/2",
    ] {
        assert_eq!(send(&f, &alice, "PUT", uri).await.0, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        send(&f, &alice, "GET", "/api/v1/me/favourites/movies/99")
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    send(&f, &alice, "PUT", "/api/v1/me/favourites/series/1").await;
    // The series' folder goes missing, as a NAS dropping out looks: the favourite stays.
    sqlx::query("UPDATE series SET missing_since = 9 WHERE id = 1")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, body) = send(&f, &alice, "GET", "/api/v1/me/favourites").await;
    assert_eq!(body["items"][0]["present"], false);
    assert_eq!(body["items"][0]["title"], "Show");
    let (_, state) = send(&f, &alice, "GET", "/api/v1/me/favourites/series/1").await;
    assert_eq!(state["favourite"], true);
}

#[tokio::test]
async fn the_event_is_sent_once_there_have_been_any_and_changes_with_them() {
    let f = fixture().await;
    let alice = token(&f, "alice").await;
    assert_eq!(event(&f.pool, 1).await.unwrap(), None);
    send(&f, &alice, "PUT", "/api/v1/me/favourites/movies/1").await;
    let first = event(&f.pool, 1).await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&first).unwrap(),
        json!({ "version": 1, "total": 1 })
    );
    send(&f, &alice, "DELETE", "/api/v1/me/favourites/movies/1").await;
    let after = event(&f.pool, 1).await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&after).unwrap(),
        json!({ "version": 2, "total": 0 })
    );
}
