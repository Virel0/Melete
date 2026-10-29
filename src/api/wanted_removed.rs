//! Titles taken off the wanted list (admins only): seeing them, letting one come back
//! with the next import or list sync, or adding it again now.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use super::requests::refused;
use crate::wanted_edit::Kind;
use crate::wanted_removed::{self, NotAdded, Removal};
use crate::{musicbrainz, tmdb};

#[derive(Clone)]
struct RemovedState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
}

/// The routes for titles taken off the wanted list, for admins only.
pub fn routes(
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/wanted/removed", get(list))
        .route("/api/v1/wanted/removed/allow", post(allow))
        .route("/api/v1/wanted/removed/add", post(add))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(RemovedState {
            pool,
            tmdb,
            musicbrainz,
        })
}

async fn list(State(state): State<RemovedState>) -> Result<Json<Vec<Removal>>, ApiError> {
    Ok(Json(wanted_removed::list(&state.pool).await?))
}

/// A removal, as listed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Which {
    kind: String,
    key: String,
}

impl Which {
    fn kind(&self) -> Result<Kind, ApiError> {
        Kind::parse(&self.kind).ok_or_else(|| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "kind is movie, series, artist or album",
            )
        })
    }
}

const NOT_FOUND: &str = "it isn't taken off the list";

/// Forgets a removal: the next import or list sync may add the title again.
async fn allow(
    State(state): State<RemovedState>,
    Json(which): Json<Which>,
) -> Result<StatusCode, ApiError> {
    if wanted_removed::allow(&state.pool, which.kind()?, &which.key).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::new(StatusCode::NOT_FOUND, NOT_FOUND))
    }
}

#[derive(Serialize)]
struct Added {
    kind: String,
    /// Its id on the wanted list of its kind.
    wanted_id: i64,
}

/// Adds a removed title to the wanted list again now, as an approved request would.
async fn add(
    State(state): State<RemovedState>,
    Json(which): Json<Which>,
) -> Result<Json<Added>, ApiError> {
    let kind = which.kind()?;
    match wanted_removed::add_again(
        &state.pool,
        state.tmdb.as_deref(),
        state.musicbrainz.as_deref(),
        kind,
        &which.key,
    )
    .await?
    {
        Ok(wanted_id) => Ok(Json(Added {
            kind: which.kind,
            wanted_id,
        })),
        Err(NotAdded::NotFound) => Err(ApiError::new(StatusCode::NOT_FOUND, NOT_FOUND)),
        Err(NotAdded::Refused(why)) => Err(refused(why)),
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
    use crate::db;

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
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
    async fn admins_see_removed_titles_and_let_them_back() {
        let pool = db::open_in_memory().await;
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
        let (admin, viewer) = (tokens[0].clone(), tokens[1].clone());
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
                 VALUES (1, 438631, 'Dune', 1, 'radarr', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Taken off through the wanted list's own route, so who did it is kept.
        let app = routes(pool.clone(), None, None).merge(super::super::import::routes(pool, None));
        let (status, _) = send(&app, "DELETE", "/api/v1/wanted/movies/1", &admin, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, listed) = send(&app, "GET", "/api/v1/wanted/removed", &admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            listed,
            json!([{ "kind": "movie", "key": "438631", "title": "Dune",
                     "removed_at": listed[0]["removed_at"], "removed_by": "alice" }])
        );

        // Adding it again needs TMDB, which isn't set up here: refused, and still listed.
        let dune = json!({ "kind": "movie", "key": "438631" });
        let (status, body) = send(
            &app,
            "POST",
            "/api/v1/wanted/removed/add",
            &admin,
            Some(dune.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("TMDB"));
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/wanted/removed/allow",
            &admin,
            Some(json!({ "kind": "film", "key": "1" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/wanted/removed/allow",
            &admin,
            Some(dune.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/wanted/removed/allow",
            &admin,
            Some(dune.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/wanted/removed/add",
            &admin,
            Some(dune.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (_, listed) = send(&app, "GET", "/api/v1/wanted/removed", &admin, None).await;
        assert_eq!(listed, json!([]));

        for (method, uri) in [
            ("GET", "/api/v1/wanted/removed"),
            ("POST", "/api/v1/wanted/removed/allow"),
            ("POST", "/api/v1/wanted/removed/add"),
        ] {
            let (status, _) = send(&app, method, uri, &viewer, Some(dune.clone())).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
    }
}
