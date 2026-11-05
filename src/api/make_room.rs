//! Make room (admins only): the titles suggested for freeing space, each with its action and the
//! space it would free, and the total (`make_room`). Nothing is changed here.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};

/// The route, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/admin/make-room", get(list))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// The suggestions, the strongest first, and the space they'd free together.
async fn list(State(pool): State<SqlitePool>) -> Result<Json<Value>, ApiError> {
    let extra: Vec<String> = crate::settings::value(&pool, "MELETE_MAJOR_STUDIOS")
        .await?
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let items = crate::make_room::suggestions(&pool, &extra).await?;
    let total: i64 = items.iter().map(|s| s.saves).sum();
    Ok(Json(json!({ "saves": total, "items": items })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn get(app: &Router, token: &str) -> (StatusCode, Value) {
        let request = Request::get("/api/v1/admin/make-room")
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
    async fn admins_see_the_suggestions_and_what_they_free_together() {
        let pool = crate::db::open_in_memory().await;
        let old = crate::db::unix_now() - 730 * 86_400;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year,
                 seen_generation) VALUES (1, 1, 'A', 'A', 2001, 1), (2, 1, 'B', 'B', 2002, 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
                 movie_id, first_seen_generation, seen_generation)
                 VALUES (1, 1, 'A/a.mkv', 3000, 1, {old}, 1, 1, 1),
                     (2, 1, 'B/b.mkv', 5000, 1, {old}, 2, 1, 1);"
        )))
        .execute(&pool)
        .await
        .unwrap();
        let admin = crate::auth::testing::user(&pool, "admin", true).await;
        let viewer = crate::auth::testing::user(&pool, "viewer", false).await;
        let token = |id| {
            let pool = pool.clone();
            async move {
                auth::issue_token(&pool, id, TokenKind::Session, "t")
                    .await
                    .unwrap()
                    .1
            }
        };
        let (admin, viewer) = (token(admin.id).await, token(viewer.id).await);
        let app = routes(pool.clone());
        let (status, body) = get(&app, &admin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["saves"], 8000);
        assert_eq!(body["items"][0]["action"], "delete");
        assert_eq!(
            body["items"][0]["reasons"],
            json!(["never_watched", "untouched"])
        );
        assert_eq!(get(&app, &viewer).await.0, StatusCode::FORBIDDEN);
    }
}
