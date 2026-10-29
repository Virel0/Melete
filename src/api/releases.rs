//! Showing how Melete reads a release name (admins), for answering "why did it think this was
//! 720p?".

use std::sync::Arc;

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::release;

/// The release name route, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool,
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/releases/parse", get(parse))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
}

#[derive(Deserialize)]
struct ParseQuery {
    name: String,
}

async fn parse(Query(query): Query<ParseQuery>) -> Result<Json<Value>, ApiError> {
    let name = query.name.trim();
    if name.is_empty() || name.chars().count() > 500 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "name is 1 to 500 characters",
        ));
    }
    let parsed = release::parse(name);
    let mut body = serde_json::to_value(&parsed)?;
    body["quality"] = Value::String(parsed.quality());
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::extract::Request;
    use axum::http::header;
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    async fn call(admin: bool, uri: &str) -> (StatusCode, Value) {
        let pool = db::open_in_memory().await;
        let user = auth::create_user(&pool, "u", "user-password".into(), admin)
            .await
            .unwrap()
            .unwrap();
        let (_, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let request = Request::get(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = routes(pool).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn shows_how_a_name_is_read() {
        let (status, body) = call(
            true,
            "/api/v1/releases/parse?name=Severance.S02E01.2160p.ATVP.WEB-DL.DDP5.1.DV.H.265-FLUX",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["title"], "Severance");
        assert_eq!(
            body["episodes"],
            json!({ "kind": "standard", "value": { "season": 2, "episodes": [1] } })
        );
        assert_eq!(body["quality"], "WEBDL-2160p");
        assert_eq!(body["source"], "web_dl");
        assert_eq!(
            (body["dolby_vision"].as_bool(), body["group"].as_str()),
            (Some(true), Some("FLUX"))
        );
    }

    #[tokio::test]
    async fn admins_only_and_names_are_bounded() {
        assert_eq!(
            call(false, "/api/v1/releases/parse?name=x").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(true, "/api/v1/releases/parse?name=%20").await.0,
            StatusCode::BAD_REQUEST
        );
    }
}
