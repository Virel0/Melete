//! Make room (admins only): the titles suggested for freeing space, each with its action and the
//! space it would free, and the total (`make_room`); and removing a title an admin chooses, its
//! files to the library's trash (`upgrades::remove_title`).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::Deserialize;
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
        .route("/api/v1/admin/make-room/delete", post(delete))
        .route("/api/v1/admin/make-room/swap", post(swap))
        .route("/api/v1/admin/make-room/compress", post(compress))
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
    let swapping = crate::make_room::swapping::progress(&pool).await?;
    let compressing = crate::make_room::compressing::progress(&pool).await?;
    // Swaps search and grab only while downloads are on.
    let downloads = crate::settings::value(&pool, "MELETE_DOWNLOADS")
        .await?
        .as_deref()
        == Some("on");
    Ok(Json(json!({
        "saves": total,
        "items": items,
        "swapping": swapping,
        "compressing": compressing,
        "downloads": downloads,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Title {
    /// `movie` or `series`.
    kind: String,
    id: i64,
}

/// Moves a title's files to its library's trash, where they wait 30 days and can be put back.
async fn delete(
    State(pool): State<SqlitePool>,
    Json(title): Json<Title>,
) -> Result<Json<Value>, ApiError> {
    let series = series_of(&title)?;
    match crate::upgrades::remove_title(&pool, series, title.id).await? {
        Ok(removed) => Ok(Json(
            json!({ "files": removed.files, "size": removed.size }),
        )),
        Err(crate::upgrades::NotRemoved::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such title in the library",
        )),
        Err(crate::upgrades::NotRemoved::Failed(why)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("nothing was moved: {why}"),
        )),
    }
}

/// Starts swapping a title for a smaller release, slowly, and stops it being monitored.
async fn swap(
    State(pool): State<SqlitePool>,
    Json(title): Json<Title>,
) -> Result<Json<Value>, ApiError> {
    let series = series_of(&title)?;
    use crate::make_room::swapping::{self, NotStarted};
    match swapping::start(&pool, series, title.id).await? {
        Ok(files) => Ok(Json(json!({ "files": files }))),
        Err(NotStarted::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such title in the library",
        )),
        Err(NotStarted::NotWanted) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "it isn't on the wanted list, so there's no profile to search with",
        )),
        Err(NotStarted::NothingToSwap) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "it has no files to swap, or every one is being swapped already",
        )),
    }
}

/// Queues a title's H.264 (or older) files to be compressed to HEVC, one at a time.
async fn compress(
    State(pool): State<SqlitePool>,
    Json(title): Json<Title>,
) -> Result<Json<Value>, ApiError> {
    let series = series_of(&title)?;
    use crate::make_room::compressing::{self, NotStarted};
    match compressing::start(&pool, series, title.id).await? {
        Ok(files) => Ok(Json(json!({ "files": files }))),
        Err(NotStarted::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such title in the library",
        )),
        Err(NotStarted::NothingToCompress) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "it has no file that compresses (H.264 or older, not Dolby Vision), or all are queued",
        )),
    }
}

/// Whether a request names a series (or a movie); a 400 for anything else.
fn series_of(title: &Title) -> Result<bool, ApiError> {
    match title.kind.as_str() {
        "movie" => Ok(false),
        "series" => Ok(true),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "kind is movie or series",
        )),
    }
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

        // Removing one: refused here, as its file was never on disk.
        let delete = |token: &str, body: Value| {
            let request = Request::post("/api/v1/admin/make-room/delete")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let app = app.clone();
            async move { app.oneshot(request).await.unwrap().status() }
        };
        assert_eq!(
            delete(&viewer, json!({ "kind": "movie", "id": 1 })).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            delete(&admin, json!({ "kind": "album", "id": 1 })).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            delete(&admin, json!({ "kind": "movie", "id": 99 })).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            delete(&admin, json!({ "kind": "movie", "id": 1 })).await,
            StatusCode::CONFLICT,
            "a file not as scanned moves nothing"
        );
        // Swapping one that isn't wanted: there's no profile to search with.
        let swap = Request::post("/api/v1/admin/make-room/swap")
            .header(header::AUTHORIZATION, format!("Bearer {admin}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "kind": "movie", "id": 1 }).to_string()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(swap).await.unwrap().status(),
            StatusCode::CONFLICT
        );
        assert_eq!(body["swapping"], json!([]));
        assert_eq!(body["compressing"], json!([]));
        // Compressing one with no probe: nothing compresses.
        let compress = Request::post("/api/v1/admin/make-room/compress")
            .header(header::AUTHORIZATION, format!("Bearer {admin}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "kind": "movie", "id": 1 }).to_string()))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(compress).await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }
}
