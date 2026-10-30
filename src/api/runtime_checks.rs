//! A wanted title's running-time checks (admins only): how many of its downloads were held back
//! and why, and "The runtime is right after all", which lets Melete search for it again.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::runtime_checks::{self, Status};
use crate::search::Title;

/// The routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/runtime-checks/{kind}/{id}", get(status))
        .route("/api/v1/runtime-checks/{kind}/{id}/right", post(right))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// The wanted title a path names: "movie" or "series", and its row id.
fn title(kind: &str, id: i64) -> Result<Title, ApiError> {
    match kind {
        "movie" => Ok(Title::Movie(id)),
        "series" => Ok(Title::Series(id)),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "kind is movie or series",
        )),
    }
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such wanted title")
}

async fn status(
    State(pool): State<SqlitePool>,
    Path((kind, id)): Path<(String, i64)>,
) -> Result<Json<Status>, ApiError> {
    runtime_checks::status(&pool, title(&kind, id)?)
        .await?
        .map(Json)
        .ok_or_else(not_found)
}

async fn right(
    State(pool): State<SqlitePool>,
    Path((kind, id)): Path<(String, i64)>,
) -> Result<Json<Status>, ApiError> {
    runtime_checks::say_right(&pool, title(&kind, id)?)
        .await?
        .map(Json)
        .ok_or_else(not_found)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn send(app: &Router, token: &str, method: &str, uri: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
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

    #[tokio::test]
    async fn says_where_a_title_stands_and_takes_the_override() {
        let pool = crate::rss::tests::setup().await;
        let dune = crate::rss::tests::want_dune(&pool).await;
        let password = auth::generate_password().unwrap();
        let user = auth::create_user(&pool, "admin", password, true)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        for n in 1..=3 {
            let failure = runtime_checks::Failure {
                at: n,
                download_id: n,
                release: format!("Fake{n}"),
                reason: "it runs 1 h 32 m; the film runs 2 h 46 m".into(),
            };
            runtime_checks::record_failure(&pool, Title::Movie(dune), failure, Some(5520.0))
                .await
                .unwrap();
        }
        let app = routes(pool);
        let uri = format!("/api/v1/runtime-checks/movie/{dune}");
        let (status, body) = send(&app, &token, "GET", &uri).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            (
                &body["failures"],
                &body["stopped"],
                &body["reasons"][2]["release"]
            ),
            (&json!(3), &json!(true), &json!("Fake3"))
        );
        let (status, body) = send(&app, &token, "POST", &format!("{uri}/right")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            (&body["failures"], &body["stopped"], &body["overridden"]),
            (&json!(0), &json!(false), &json!(true))
        );
        assert_eq!(body["override_seconds"], json!(5520.0));
        let (status, _) = send(&app, &token, "GET", "/api/v1/runtime-checks/movie/999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(&app, &token, "GET", "/api/v1/runtime-checks/album/1").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
