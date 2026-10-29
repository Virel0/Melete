//! Motion covers made from still covers: each user's choice to see them or not, and, for
//! admins, how far making them has got.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin, require_user};
use crate::motion::{self, Coverage};

/// Each user's motion cover choice, for everyone signed in; how far making them has got, for
/// admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let users = Router::new()
        .route("/api/v1/music/motion-covers", get(choice).put(set_choice))
        .route_layer(middleware::from_fn_with_state(auth.clone(), require_user));
    let admins = Router::new()
        .route("/api/v1/music/motion-covers/status", get(status))
        .route_layer(middleware::from_fn_with_state(auth, require_admin));
    users.merge(admins).with_state(pool)
}

async fn view(pool: &SqlitePool, user_id: i64) -> Result<Value, ApiError> {
    Ok(json!({
        "show_generated": motion::chosen(pool, user_id).await?,
        "on": crate::settings::motion_covers(pool).await?,
    }))
}

async fn choice(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(view(&pool, current.user.id).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Choice {
    show_generated: bool,
}

/// Shows or hides motion covers made from still covers, for yourself.
async fn set_choice(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Choice>,
) -> Result<Json<Value>, ApiError> {
    motion::choose(&pool, current.user.id, body.show_generated).await?;
    Ok(Json(view(&pool, current.user.id).await?))
}

async fn status(State(pool): State<SqlitePool>) -> Result<Json<Coverage>, ApiError> {
    Ok(Json(motion::coverage(&pool).await?))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
        let user = crate::auth::create_user(pool, name, "user-password".into(), admin)
            .await
            .unwrap()
            .unwrap();
        crate::auth::issue_token(pool, user.id, crate::auth::TokenKind::Session, "t")
            .await
            .unwrap()
            .1
    }

    async fn call(app: &Router, token: &str, uri: &str, put: Option<Value>) -> (StatusCode, Value) {
        let builder = Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let request = match put {
            Some(body) => builder
                .method("PUT")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => builder.body(Body::empty()),
        }
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
    async fn each_user_chooses_for_themselves_and_only_admins_see_progress() {
        let pool = crate::db::open_in_memory().await;
        let app = routes(pool.clone());
        let alice = token(&pool, "alice", false).await;
        let bob = token(&pool, "bob", false).await;
        let admin = token(&pool, "admin", true).await;
        let uri = "/api/v1/music/motion-covers";
        assert_eq!(
            call(&app, &alice, uri, None).await,
            (StatusCode::OK, json!({"show_generated": true, "on": false})),
            "shown until turned off; motion covers themselves are off by default"
        );
        assert_eq!(
            call(&app, &alice, uri, Some(json!({"show_generated": false}))).await,
            (
                StatusCode::OK,
                json!({"show_generated": false, "on": false})
            )
        );
        assert_eq!(call(&app, &bob, uri, None).await.1["show_generated"], true);
        assert_eq!(
            call(&app, &alice, uri, Some(json!({"show": false})))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );

        let status = "/api/v1/music/motion-covers/status";
        assert_eq!(
            call(&app, &alice, status, None).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&app, &admin, status, None).await,
            (
                StatusCode::OK,
                json!({"albums": 0, "done": 0, "queued": 0, "skipped": 0, "failed": 0,
                    "last_error": null})
            )
        );
    }
}
