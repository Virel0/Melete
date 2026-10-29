//! The blocklist (admins only): releases never to be grabbed again, added from a search
//! result or a download, listed, and taken off again.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::blocklist::{self, Entry, From};

/// The blocklist routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/blocklist", get(list).post(add))
        .route("/api/v1/blocklist/{id}", delete(remove))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Entry>>, ApiError> {
    Ok(Json(blocklist::list(&pool).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewEntry {
    /// A search result.
    release_id: Option<i64>,
    /// A download.
    download_id: Option<i64>,
}

/// Blocklists a search result's or a download's release. Answers the entry, also when it was
/// on the blocklist already.
async fn add(
    State(pool): State<SqlitePool>,
    Json(body): Json<NewEntry>,
) -> Result<(StatusCode, Json<Entry>), ApiError> {
    let from = match (body.release_id, body.download_id) {
        (Some(id), None) => From::Release(id),
        (None, Some(id)) => From::Download(id),
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "send release_id or download_id, one of them",
            ));
        }
    };
    match blocklist::add(&pool, from).await? {
        Some(entry) => Ok((StatusCode::CREATED, Json(entry))),
        None => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            match from {
                From::Release(_) => "no such release",
                From::Download(_) => "no such download",
            },
        )),
    }
}

async fn remove(
    State(pool): State<SqlitePool>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if blocklist::remove(&pool, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "it isn't on the blocklist",
        ))
    }
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

    async fn send(
        app: &Router,
        token: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
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
    async fn admins_blocklist_results_and_downloads_and_take_them_off() {
        let pool = crate::db::open_in_memory().await;
        let token = |name: &'static str, admin: bool| {
            let pool = pool.clone();
            async move {
                let user = auth::create_user(&pool, name, "a-long-password".into(), admin)
                    .await
                    .unwrap()
                    .unwrap();
                auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                    .await
                    .unwrap()
                    .1
            }
        };
        let (admin, viewer) = (token("admin", true).await, token("viewer", false).await);
        sqlx::raw_sql(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at) VALUES (1, 603, 'The Matrix', 1, 'radarr', 0);
             INSERT INTO searches (id, wanted_movie_id, searched_at, indexers) VALUES (1, 1, 0, '[]');
             INSERT INTO release_decisions (id, search_id, title, accepted, decision) VALUES
                 (5, 1, 'The.Matrix.1999.1080p.BluRay.x264-BAD', 1, '{}');
             INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, added_at, updated_at)
                 VALUES (7, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'The.Matrix.1999.2160p.WEB-DL-OTHER', 1, '{}', 'failed', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone());

        let (status, _) = send(&app, &viewer, "GET", "/api/v1/blocklist", None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        for body in [json!({}), json!({ "release_id": 5, "download_id": 7 })] {
            let (status, _) = send(&app, &admin, "POST", "/api/v1/blocklist", Some(body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (status, _) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/blocklist",
            Some(json!({ "release_id": 99 })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, from_result) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/blocklist",
            Some(json!({ "release_id": 5 })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{from_result}");
        assert_eq!(
            from_result["title"],
            "The.Matrix.1999.1080p.BluRay.x264-BAD"
        );
        assert_eq!(from_result["source"], "torrent");
        let (status, again) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/blocklist",
            Some(json!({ "release_id": 5 })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(again["id"], from_result["id"], "on the list once");
        let (status, _) = send(
            &app,
            &admin,
            "POST",
            "/api/v1/blocklist",
            Some(json!({ "download_id": 7 })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        // Blocked by name (case aside), or by the download's torrent under any name.
        assert!(
            blocklist::blocks(&pool, "the.matrix.1999.1080p.bluray.x264-bad", None)
                .await
                .unwrap()
        );
        assert!(
            blocklist::blocks(
                &pool,
                "Renamed",
                Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            )
            .await
            .unwrap()
        );
        assert!(!blocklist::blocks(&pool, "Renamed", None).await.unwrap());

        let (status, listed) = send(&app, &admin, "GET", "/api/v1/blocklist", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed.as_array().unwrap().len(), 2);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        let documented: std::collections::BTreeSet<&str> =
            spec["components"]["schemas"]["BlocklistEntry"]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
        let actual: std::collections::BTreeSet<&str> = listed[0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(actual, documented);

        let uri = format!("/api/v1/blocklist/{}", from_result["id"]);
        assert_eq!(
            send(&app, &admin, "DELETE", &uri, None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            send(&app, &admin, "DELETE", &uri, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert!(
            !blocklist::blocks(&pool, "The.Matrix.1999.1080p.BluRay.x264-BAD", None)
                .await
                .unwrap()
        );
    }
}
