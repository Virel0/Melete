use axum::body::Body;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};

async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
    let user = auth::testing::user(pool, name, admin).await;
    auth::issue_token(pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

#[tokio::test]
async fn admins_see_a_series_episodes_without_styled_subtitles() {
    let pool = crate::db::open_in_memory().await;
    let (anime, shows) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    crate::subtitles::unstyled::tests::anime_episodes(&pool, anime.path(), shows.path()).await;
    let (admin, viewer) = (
        token(&pool, "admin", true).await,
        token(&pool, "viewer", false).await,
    );
    let app = routes(pool.clone());

    let (status, episodes) = get(&app, &admin, "/api/v1/series/1/unstyled-subtitles").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        episodes,
        json!([{ "file_id": 2, "season": 1, "episode": 2, "group": "SubsPlease" }])
    );
    // A series that isn't anime has none to show.
    let (_, episodes) = get(&app, &admin, "/api/v1/series/3/unstyled-subtitles").await;
    assert_eq!(episodes, json!([]));
    let (status, _) = get(&app, &viewer, "/api/v1/series/1/unstyled-subtitles").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
