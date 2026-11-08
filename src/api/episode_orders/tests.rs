use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::tmdb::testing::{FakeTmdb, Reply};

struct Fixture {
    app: Router,
    admin: String,
}

/// "Some Anime" in a library: TMDB lists one season of 4 episodes, the files are named S01E01,
/// S01E02 and S02E01, S02E02; TMDB has one group fitting them. Series 2 isn't matched.
async fn fixture() -> (Fixture, SqlitePool, FakeTmdb) {
    let pool = crate::db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'anime', 'shows', '/a');
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (100, 'Some Anime', 10);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name) VALUES
             (100, 1, 1, 1001, 'One'), (100, 1, 2, 1002, 'Two'), (100, 1, 3, 1003, 'Three'),
             (100, 1, 4, 1004, 'Four');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id)
             VALUES (1, 1, 'Some Anime', 'Some Anime', 1, 100),
                    (2, 1, 'Unknown Show', 'Unknown Show', 1, NULL);
         INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
                 episode, first_seen_generation, seen_generation) VALUES
             (1, 'Some Anime/S01E01.mkv', 1, 0, 1, 1, 1, 1, 1),
             (1, 'Some Anime/S01E02.mkv', 1, 0, 1, 1, 2, 1, 1),
             (1, 'Some Anime/S02E01.mkv', 1, 0, 1, 2, 1, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let admin = crate::auth::testing::user(&pool, "admin", true).await;
    let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "t")
        .await
        .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/tv/100/episode_groups",
        vec![Reply::ok(json!({"results": [
            {"id": "aaaa", "name": "Seasons", "type": 6},
            {"id": "bbbb", "name": "Absolute", "type": 2}
        ]}))],
    );
    fake.reply(
        "/tv/episode_group/aaaa",
        vec![Reply::ok(json!({"id": "aaaa", "name": "Seasons", "type": 6, "groups": [
            {"name": "Season 1", "order": 1, "episodes": [{"id": 1001, "order": 0}, {"id": 1002, "order": 1}]},
            {"name": "Season 2", "order": 2, "episodes": [{"id": 1003, "order": 0}, {"id": 1004, "order": 1}]}
        ]}))],
    );
    fake.reply(
        "/tv/episode_group/bbbb",
        vec![Reply::ok(
            json!({"id": "bbbb", "name": "Absolute", "type": 2, "groups": [
                {"name": "Episodes", "order": 0, "episodes": [
                    {"id": 1001, "order": 0}, {"id": 1002, "order": 1},
                    {"id": 1003, "order": 2}, {"id": 1004, "order": 3}]}
            ]}),
        )],
    );
    let tmdb = Arc::new(fake.start().await);
    let app = routes(pool.clone(), Some(tmdb)).merge(crate::api::media::routes(
        pool.clone(),
        std::env::temp_dir(),
        crate::tmdb::Images::new().unwrap(),
    ));
    (Fixture { app, admin }, pool, fake)
}

async fn send(f: &Fixture, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", f.admin))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn an_admin_lists_a_series_orders_and_picks_one() {
    let (f, _pool, fake) = fixture().await;
    let (status, series) = send(&f, "GET", "/api/v1/series/1", None).await;
    assert_eq!(status, StatusCode::OK, "{series}");
    assert_eq!(series["episode_order"]["group_id"], Value::Null);
    assert_eq!(
        series["episode_order"]["suggested"],
        Value::Null,
        "no groups known yet"
    );

    // Listing fetches the groups the first time, and not again.
    let (status, listed) = send(&f, "GET", "/api/v1/series/1/orders", None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["groups"].as_array().unwrap().len(), 2);
    assert_eq!(listed["groups"][0]["group_id"], "aaaa");
    assert_eq!(listed["groups"][0]["exact"], false, "S02E02 isn't here");
    assert_eq!(listed["groups"][0]["covered"], 3);
    assert_eq!(listed["suggested"]["group_id"], "aaaa");
    send(&f, "GET", "/api/v1/series/1/orders", None).await;
    assert_eq!(fake.log.lock().unwrap().len(), 3);
    let (_, series) = send(&f, "GET", "/api/v1/series/1", None).await;
    assert_eq!(series["episode_order"]["suggested"]["name"], "Seasons");

    let (status, chosen) = send(
        &f,
        "PUT",
        "/api/v1/series/1/order",
        Some(json!({"group_id": "aaaa"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chosen}");
    assert_eq!(chosen["order"]["chosen_by"], "admin");
    assert_eq!(chosen["suggested"], Value::Null);
    let (_, series) = send(&f, "GET", "/api/v1/series/1", None).await;
    assert_eq!(series["episode_order"]["name"], "Seasons");
    let seasons: Vec<(i64, i64)> = series["seasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["season"].as_i64().unwrap(),
                s["episodes"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(seasons, vec![(1, 2), (2, 2)]);
    let (status, season) = send(&f, "GET", "/api/v1/series/1/seasons/2", None).await;
    assert_eq!(status, StatusCode::OK, "{season}");
    assert_eq!(season["episodes"][0]["name"], "Three");

    // And back to TMDB's own order.
    let (status, chosen) = send(
        &f,
        "PUT",
        "/api/v1/series/1/order",
        Some(json!({"group_id": null})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chosen}");
    assert_eq!(chosen["order"]["group_id"], Value::Null);
    assert_eq!(chosen["order"]["chosen_by"], "admin");
}

#[tokio::test]
async fn orders_are_refused_for_what_isnt_there() {
    let (f, _pool, _fake) = fixture().await;
    let put = |id: i64, group: Value| {
        let f = &f;
        async move {
            send(
                f,
                "PUT",
                &format!("/api/v1/series/{id}/order"),
                Some(json!({"group_id": group})),
            )
            .await
            .0
        }
    };
    assert_eq!(put(99, json!("aaaa")).await, StatusCode::NOT_FOUND);
    assert_eq!(put(2, json!("aaaa")).await, StatusCode::CONFLICT);
    assert_eq!(put(1, json!("../x")).await, StatusCode::BAD_REQUEST);
    // Not fetched yet, so not one of the series' groups.
    assert_eq!(
        put(1, json!("aaaa")).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, _) = send(&f, "GET", "/api/v1/series/2/orders", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
}
