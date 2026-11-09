//! The VPN beside Melete (Gluetun), for admins: how it is, and starting, stopping or restarting
//! it. Its control server's address and key are settings (Outside services, VPN); the key is
//! never in an answer.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::Serialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::gluetun;

/// The VPN routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/vpn", get(show))
        .route("/api/v1/vpn/{action}", post(act))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// How the VPN is, as the web UI shows it.
#[derive(Debug, Serialize)]
struct View {
    /// Whether its control server's address and key are set.
    set_up: bool,
    /// How it is, when its control server answered.
    state: Option<gluetun::State>,
    /// Why it couldn't be asked, in plain words; null when it answered.
    trouble: Option<String>,
}

/// The client for the control server as the settings now stand; None while it isn't set up.
async fn client(pool: &SqlitePool) -> Result<Option<gluetun::Client>, ApiError> {
    let config = crate::settings::load(pool).await?;
    Ok(match &config.vpn {
        Some(vpn) => Some(gluetun::Client::new(&vpn.url, vpn.key.clone())?),
        None => None,
    })
}

async fn view(client: Option<gluetun::Client>) -> View {
    let Some(client) = client else {
        return View {
            set_up: false,
            state: None,
            trouble: None,
        };
    };
    match client.state().await {
        Ok(state) => View {
            set_up: true,
            state: Some(state),
            trouble: None,
        },
        Err(why) => View {
            set_up: true,
            state: None,
            trouble: Some(why.to_string()),
        },
    }
}

async fn show(State(pool): State<SqlitePool>) -> Result<Json<View>, ApiError> {
    Ok(Json(view(client(&pool).await?).await))
}

/// `start`, `stop` or `restart`, then how it is.
async fn act(
    State(pool): State<SqlitePool>,
    Path(action): Path<String>,
) -> Result<Json<View>, ApiError> {
    let Some(vpn) = client(&pool).await? else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "set the VPN's control server and key under Outside services first",
        ));
    };
    let done = match action.as_str() {
        "start" => vpn.set_running(true).await,
        "stop" => vpn.set_running(false).await,
        "restart" => vpn.restart().await,
        _ => return Err(ApiError::new(StatusCode::NOT_FOUND, "no such action")),
    };
    if let Err(why) = done {
        return Err(ApiError::new(StatusCode::BAD_GATEWAY, why.to_string()));
    }
    Ok(Json(view(Some(vpn)).await))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;
    use crate::gluetun::testing;

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

    async fn store(pool: &SqlitePool, base: &str) {
        for (name, value) in [("MELETE_VPN_URL", base), ("MELETE_VPN_KEY", testing::KEY)] {
            sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES (?, ?, 0)")
                .bind(name)
                .bind(value)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn admins_see_and_steer_the_vpn() {
        let pool = db::open_in_memory().await;
        let (admin, viewer) = (
            token(&pool, "admin", true).await,
            token(&pool, "bob", false).await,
        );
        let app = routes(pool.clone());
        let (status, unset) = send(&app, "GET", "/api/v1/vpn", &admin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            unset,
            json!({ "set_up": false, "state": null, "trouble": null })
        );
        let (status, _) = send(&app, "POST", "/api/v1/vpn/stop", &admin).await;
        assert_eq!(status, StatusCode::CONFLICT);

        let (base, fake) = testing::start().await;
        store(&pool, &base).await;
        for (method, uri) in [("GET", "/api/v1/vpn"), ("POST", "/api/v1/vpn/stop")] {
            assert_eq!(
                send(&app, method, uri, &viewer).await.0,
                StatusCode::FORBIDDEN
            );
        }
        let (_, shown) = send(&app, "GET", "/api/v1/vpn", &admin).await;
        assert_eq!(shown["state"]["public_ip"], json!("203.0.113.7"));
        assert_eq!(shown["state"]["forwarded_port"], json!(51413));
        let (status, stopped) = send(&app, "POST", "/api/v1/vpn/stop", &admin).await;
        assert_eq!(
            (status, &stopped["state"]["running"]),
            (StatusCode::OK, &json!(false))
        );
        let (_, started) = send(&app, "POST", "/api/v1/vpn/restart", &admin).await;
        assert_eq!(started["state"]["running"], json!(true));
        assert_eq!(
            send(&app, "POST", "/api/v1/vpn/explode", &admin).await.0,
            StatusCode::NOT_FOUND
        );
        assert!(!started.to_string().contains(testing::KEY));

        // One open to anyone is refused, and left as it is.
        fake.lock().unwrap().open = vec!["*"];
        let (_, open) = send(&app, "GET", "/api/v1/vpn", &admin).await;
        assert!(
            open["trouble"]
                .as_str()
                .unwrap()
                .contains("answers without a key"),
            "{open}"
        );
        let (status, _) = send(&app, "POST", "/api/v1/vpn/stop", &admin).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(fake.lock().unwrap().running);
    }
}
