//! `GET /api/v1/admin/health` (admins only): what needs attention in each admin area, for the
//! web UI's area switcher and any app that wants to show it.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::admin_health::{self, Health};
use crate::transcode::Transcoder;

/// The admin health route, for admins only, without the transcoder's findings.
#[cfg(test)]
pub fn routes(pool: SqlitePool) -> Router {
    routes_with(pool, None)
}

/// The admin health route, for admins only, with the transcoder's findings when given.
pub fn routes_with(pool: SqlitePool, transcoder: Option<Arc<Transcoder>>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/admin/health", get(health))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(HealthState { pool, transcoder })
}

#[derive(Clone)]
struct HealthState {
    pool: SqlitePool,
    transcoder: Option<Arc<Transcoder>>,
}

async fn health(State(state): State<HealthState>) -> Result<Json<Health>, ApiError> {
    let mut health = admin_health::check(&state.pool).await?;
    if let Some(reason) = state.transcoder.as_ref().and_then(|t| t.gpu_problem()) {
        admin_health::gpu_unavailable(&mut health, &reason);
    }
    Ok(Json(health))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::{StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    #[tokio::test]
    async fn admins_see_every_area_and_others_are_refused() {
        let pool = db::open_in_memory().await;
        let app = routes(pool.clone());
        let mut tokens = Vec::new();
        for (name, admin) in [("alice", true), ("bob", false)] {
            let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
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
        let ask = |token: &str| {
            Request::get("/api/v1/admin/health")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(ask(&tokens[0])).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let areas: Vec<&str> = body["areas"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["area"].as_str().unwrap())
            .collect();
        assert_eq!(areas, admin_health::AREAS);
        assert_eq!(body["areas"][3]["findings"][0]["code"], "no_indexers");
        assert_eq!(body["areas"][3]["findings"][0]["level"], "problem");

        let response = app.oneshot(ask(&tokens[1])).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_gpu_that_fails_its_test_is_a_system_problem() {
        let dir = tempfile::tempdir().unwrap();
        let (_bin, ffmpeg) = crate::transcode::testing::ffmpeg_without_gpu();
        let transcoder = Transcoder::new(
            ffmpeg,
            crate::transcode::Hardware::Nvenc,
            dir.path().join("t"),
            4,
        );
        let pool = db::open_in_memory().await;
        let app = routes_with(pool.clone(), Some(transcoder.clone()));
        let user = auth::create_user(&pool, "alice", "alice-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        let system = |app: Router| {
            let request = Request::get("/api/v1/admin/health")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            async move {
                let response = app.oneshot(request).await.unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                body["areas"][5]["findings"].clone()
            }
        };
        assert_eq!(system(app.clone()).await, serde_json::json!([]));

        transcoder.check_gpu().await;
        let findings = system(app).await;
        assert_eq!(findings[0]["code"], "gpu_unavailable");
        assert_eq!(findings[0]["level"], "problem");
        let message = findings[0]["message"].as_str().unwrap();
        assert!(
            message.starts_with("Video conversions are using the CPU: ")
                && message.contains("Cannot load libcuda.so.1"),
            "{message}"
        );
    }
}
