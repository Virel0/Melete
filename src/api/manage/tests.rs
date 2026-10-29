use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;

struct Fixture {
    app: Router,
    admin: String,
    viewer: String,
}

async fn fixture() -> Fixture {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'Films', 'movies', '/m', NULL), (2, 'Kids', 'movies', '/k', 'kids');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation)
             VALUES (1, 1, 'Alien (1979)', 'Alien', 1979, 1), (2, 2, 'Up (2009)', 'Up', 2009, 1);
         INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation, added_at) VALUES
             (1, 'Alien (1979)/Alien.1979.2160p.UHD.BluRay.REMUX.mkv', 50, 0, 1, 1, 1, 10),
             (2, 'Up (2009)/Up.2009.720p.WEB-DL.mkv', 5, 0, 2, 1, 1, 20);
         INSERT INTO wanted_movies (id, tmdb_id, title, year, monitored, library_id, movie_id,
                 source, added_at) VALUES
             (1, 348, 'Alien', 1979, 1, 1, 1, 'radarr', 30),
             (2, 999, 'Brazil', 1985, 1, 1, NULL, 'manual', 40),
             (3, 998, 'Clue', 1985, 0, 1, NULL, 'manual', 50);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut tokens = Vec::new();
    for (name, admin) in [("alice", true), ("bob", false)] {
        let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        tokens.push(token);
    }
    let viewer = tokens.pop().unwrap();
    Fixture {
        app: routes(pool),
        admin: tokens.pop().unwrap(),
        viewer,
    }
}

async fn get(f: &Fixture, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::get(uri)
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

fn titles(body: &Value) -> Vec<&str> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn admins_see_every_managed_list_and_others_are_refused() {
    let f = fixture().await;
    for list in ["movies", "series", "anime", "artists"] {
        let uri = format!("/api/v1/manage/{list}");
        let (status, body) = get(&f, &uri, &f.admin).await;
        assert_eq!(status, StatusCode::OK, "{list}");
        assert!(body["items"].is_array(), "{list}");
        let (status, _) = get(&f, &uri, &f.viewer).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{list}");
    }
}

#[tokio::test]
async fn the_movies_list_filters_sorts_and_pages() {
    let f = fixture().await;
    let (_, all) = get(&f, "/api/v1/manage/movies", &f.admin).await;
    assert_eq!(titles(&all), ["Alien", "Brazil", "Clue", "Up"]);
    assert_eq!(all["total"], 4);
    assert_eq!(
        all["counts"],
        serde_json::json!({ "in_library": 2, "missing": 1, "downloading": 0, "unreleased": 0,
                             "unmonitored": 1, "cutoff_unmet": 0 })
    );
    let alien = &all["items"][0];
    assert_eq!(alien["state"], "in_library");
    assert_eq!(alien["quality"], "Remux-2160p");
    assert_eq!(
        (alien["wanted_id"].as_i64(), alien["movie_id"].as_i64()),
        (Some(1), Some(1))
    );
    assert_eq!(all["items"][3]["wanted"], false);

    let cases = [
        ("state=missing", vec!["Brazil"]),
        ("monitored=false", vec!["Clue", "Up"]),
        ("wanted=false", vec!["Up"]),
        ("category=kids", vec!["Up"]),
        ("category=none", vec!["Alien", "Brazil", "Clue"]),
        ("library=Films&state=in_library", vec!["Alien"]),
        ("q=RAZ", vec!["Brazil"]),
        ("sort=size", vec!["Alien", "Up", "Brazil", "Clue"]),
        ("sort=added", vec!["Clue", "Brazil", "Alien", "Up"]),
        ("sort=year", vec!["Up", "Brazil", "Clue", "Alien"]),
        ("limit=2&offset=1", vec!["Brazil", "Clue"]),
    ];
    for (query, expected) in cases {
        let (status, body) = get(&f, &format!("/api/v1/manage/movies?{query}"), &f.admin).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(titles(&body), expected, "{query}");
    }
    let (_, paged) = get(&f, "/api/v1/manage/movies?limit=2&offset=1", &f.admin).await;
    assert_eq!(paged["total"], 4, "the total counts every page");
}

#[tokio::test]
async fn one_title_is_found_by_its_wanted_entry_or_its_folder() {
    let f = fixture().await;
    let cases = [
        ("wanted_id=2", vec!["Brazil"]),
        ("wanted_id=1", vec!["Alien"]),
        ("folder=1", vec!["Alien"]),
        ("folder=2", vec!["Up"]),
        ("folder=3", vec![]),
        ("wanted_id=9", vec![]),
    ];
    for (query, expected) in cases {
        let (status, body) = get(&f, &format!("/api/v1/manage/movies?{query}"), &f.admin).await;
        assert_eq!(status, StatusCode::OK, "{query}");
        assert_eq!(titles(&body), expected, "{query}");
        assert_eq!(body["counts"]["in_library"], 2, "counts stay the service's");
    }
    for list in ["series", "anime", "artists"] {
        let uri = format!("/api/v1/manage/{list}?wanted_id=1&folder=1");
        let (status, _) = get(&f, &uri, &f.admin).await;
        assert_eq!(status, StatusCode::OK, "{list}");
    }
}

#[tokio::test]
async fn bad_filters_are_refused() {
    let f = fixture().await;
    for query in [
        "limit=0",
        "limit=501",
        "offset=-1",
        "category=adults",
        "state=lost",
        "sort=rating",
        "monitored=maybe",
    ] {
        let (status, _) = get(&f, &format!("/api/v1/manage/movies?{query}"), &f.admin).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
    let uri = "/api/v1/manage/artists?cutoff_unmet=true";
    let (status, _) = get(&f, uri, &f.admin).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "music isn't upgraded");
    let (status, _) = get(&f, "/api/v1/manage/movies?cutoff_unmet=true", &f.admin).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn histories_and_past_searches_are_for_admins_and_checked() {
    let f = fixture().await;
    let (status, page) = get(&f, "/api/v1/manage/movies/history", &f.admin).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["total"], 0);

    let cases = [
        (
            "/api/v1/manage/movies/history?kind=grabbed&limit=10",
            StatusCode::OK,
        ),
        ("/api/v1/manage/movies/history?wanted_id=2", StatusCode::OK),
        ("/api/v1/manage/movies/searches?wanted_id=2", StatusCode::OK),
        ("/api/v1/manage/films/history", StatusCode::BAD_REQUEST),
        (
            "/api/v1/manage/movies/history?kind=moved",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/manage/movies/history?limit=0",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/api/v1/manage/movies/history?wanted_id=99",
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/v1/manage/series/history?wanted_id=2",
            StatusCode::NOT_FOUND,
        ),
        ("/api/v1/manage/movies/searches", StatusCode::BAD_REQUEST),
        (
            "/api/v1/manage/anime/searches?wanted_id=2",
            StatusCode::NOT_FOUND,
        ),
    ];
    for (uri, expected) in cases {
        let (status, body) = get(&f, uri, &f.admin).await;
        assert_eq!(status, expected, "{uri}: {body}");
        if expected == StatusCode::OK {
            assert_eq!(
                get(&f, uri, &f.viewer).await.0,
                StatusCode::FORBIDDEN,
                "{uri}"
            );
        }
    }
}
