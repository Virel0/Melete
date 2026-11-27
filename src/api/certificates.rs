//! HTTPS's certificate got by Melete itself, for admins: how it stands (the CA, the account's
//! URI for a CAA record, the certificate in use, renewing), and asking for one now rather than
//! when it's due. Its settings are under Outside services, Certificates; the deSEC token and
//! the keys are never in an answer.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::acme;

/// The certificate routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/certificates", get(show))
        .route("/api/v1/certificates/renew", post(renew))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

async fn show(State(pool): State<SqlitePool>) -> Result<Json<acme::Status>, ApiError> {
    let config = crate::settings::load(&pool).await?;
    Ok(Json(acme::status(&pool, &config).await?))
}

/// Starts getting a certificate now; how it stands then.
async fn renew(
    State(pool): State<SqlitePool>,
) -> Result<(StatusCode, Json<acme::Status>), ApiError> {
    let config = crate::settings::load(&pool).await?;
    if config.acme.is_none() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "set Certificates up under Outside services first",
        ));
    }
    acme::ask_now();
    Ok((
        StatusCode::ACCEPTED,
        Json(acme::status(&pool, &config).await?),
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
        let user = crate::auth::testing::user(pool, name, admin).await;
        auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1
    }

    async fn send(app: &Router, method: &str, uri: &str, token: &str) -> (StatusCode, Value) {
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
    async fn admins_see_how_it_stands_with_the_documented_fields() {
        let pool = crate::db::open_in_memory().await;
        let (admin, viewer) = (
            token(&pool, "admin", true).await,
            token(&pool, "bob", false).await,
        );
        let app = routes(pool.clone());
        for (method, uri) in [
            ("GET", "/api/v1/certificates"),
            ("POST", "/api/v1/certificates/renew"),
        ] {
            assert_eq!(
                send(&app, method, uri, &viewer).await.0,
                StatusCode::FORBIDDEN
            );
        }
        let (status, off) = send(&app, "GET", "/api/v1/certificates", &admin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (&off["on"], &off["certificate"]),
            (&Value::Bool(false), &Value::Null)
        );
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        let documented: std::collections::BTreeSet<&str> =
            spec["components"]["schemas"]["Certificates"]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
        let answered: std::collections::BTreeSet<&str> = off
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(answered, documented);
        // Nothing to get while it's off.
        let (status, _) = send(&app, "POST", "/api/v1/certificates/renew", &admin).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }
}
