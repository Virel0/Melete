use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

/// A movie in library 1 and a series in library 2, both with trailers kept; Alice sees
/// everything, Bob only library 1.
async fn fixture() -> (SqlitePool, String, String) {
    let pool = crate::db::open_in_memory().await;
    let videos = serde_json::json!([
        { "key": "vKQi3bBA1y8", "name": "Official Trailer", "language": "en", "kind": "Trailer",
          "official": true, "size": 1080, "published_at": null },
        { "key": "AAAAAAAAAAA", "name": "Bande-annonce", "language": "fr", "kind": "Trailer",
          "official": true, "size": 1080, "published_at": null }
    ])
    .to_string();
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'shows', 'shows', '/s');
         INSERT INTO tmdb_movies (tmdb_id, title, fetched_at, videos, language)
             VALUES (603, 'The Matrix', 1, ?1, 'fr-FR');
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at, videos) VALUES (1396, 'Show', 1, ?1);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES (1, 1, 'M', 'M', 1, 'matched', 603),
             (2, 1, 'N', 'N', 1, NULL, NULL);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES (1, 2, 'S', 'S', 1, 'matched', 1396);",
    )
    .bind(videos)
    .execute(&pool)
    .await
    .unwrap();
    let mut tokens = Vec::new();
    for name in ["alice", "bob"] {
        let user = auth::create_user(&pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        tokens.push(
            auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap()
                .1,
        );
    }
    sqlx::raw_sql(
        "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             SELECT id, 0, NULL, 0 FROM users WHERE username = 'bob';
         INSERT INTO user_libraries (user_id, library_id)
             SELECT id, 1 FROM users WHERE username = 'bob';",
    )
    .execute(&pool)
    .await
    .unwrap();
    let bob = tokens.pop().unwrap();
    (pool, tokens.pop().unwrap(), bob)
}

async fn turn_on(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TRAILERS_ONLINE', 'on', 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn get(pool: &SqlitePool, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = routes(pool.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn off_serves_no_trailer() {
    let (pool, alice, _) = fixture().await;
    let (status, body) = get(&pool, &alice, "/api/v1/movies/1/trailer").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.to_string().contains("off"), "{body}");
}

#[tokio::test]
async fn on_the_chosen_trailer_is_served_to_those_who_may_see_the_title() {
    let (pool, alice, bob) = fixture().await;
    turn_on(&pool).await;
    assert_eq!(
        get(&pool, &alice, "/api/v1/movies/1/trailer").await,
        (
            StatusCode::OK,
            json!({ "site": "youtube", "key": "AAAAAAAAAAA", "name": "Bande-annonce",
                    "language": "fr",
                    "embed_url": "https://www.youtube-nocookie.com/embed/AAAAAAAAAAA" })
        ),
        "in the language its metadata was fetched in"
    );
    let (status, series) = get(&pool, &alice, "/api/v1/series/1/trailer").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        series["key"], "vKQi3bBA1y8",
        "English, its metadata's language"
    );
    // Bob may not see the series' library: as if it weren't there.
    assert_eq!(
        get(&pool, &bob, "/api/v1/series/1/trailer").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&pool, &bob, "/api/v1/movies/1/trailer").await.0,
        StatusCode::OK
    );
    // Unmatched, or no such title.
    for uri in ["/api/v1/movies/2/trailer", "/api/v1/movies/9/trailer"] {
        assert_eq!(
            get(&pool, &alice, uri).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
}

#[tokio::test]
async fn the_page_lets_youtubes_player_in_only_while_trailers_online_is_on() {
    let pool = crate::db::open_in_memory().await;
    let policy = |pool: SqlitePool| async move {
        let request = Request::get("/").body(Body::empty()).unwrap();
        let response = super::super::web::routes_with(pool)
            .oneshot(request)
            .await
            .unwrap();
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_owned()
    };
    let off = policy(pool.clone()).await;
    assert!(!off.contains("frame-src"), "{off}");
    turn_on(&pool).await;
    let on = policy(pool.clone()).await;
    assert_eq!(
        on,
        format!("{off}; frame-src https://www.youtube-nocookie.com")
    );
    assert!(on.contains("frame-ancestors 'none'") && !on.contains("unsafe"));
}
