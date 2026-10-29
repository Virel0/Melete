//! The API's base: health and the API description, open to all; the library routes behind an admin;
//! and accounts.

use super::*;

/// The whole HTTP API: health is open, library data needs an admin, accounts need a user.
#[cfg(test)]
pub fn app(pool: SqlitePool) -> Router {
    app_with(pool, Arc::default())
}

/// As [`app`], with the sign-in limiter given, so other routes that check a password count
/// toward the same limits.
#[cfg(test)]
pub fn app_with(pool: SqlitePool, limiter: Arc<crate::auth::LoginLimiter>) -> Router {
    app_kept(
        pool,
        &Kept {
            limiter,
            ..Kept::default()
        },
    )
}

/// As [`app`], with sign-in's lockouts and Quick Connect's codes held in `kept`.
pub fn app_kept(pool: SqlitePool, kept: &Kept) -> Router {
    let state = auth::AppState {
        pool: pool.clone(),
        limiter: kept.limiter.clone(),
    };
    router()
        .merge(
            library_routes(pool).route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth::require_admin,
            )),
        )
        .merge(auth::routes(state, kept.quick_connect.clone()))
}

/// Health and the API description, open to all.
pub fn router() -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/openapi.json", get(openapi))
}

pub(super) async fn openapi() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        OPENAPI,
    )
}

/// The libraries, a scan now and what matching couldn't settle. Unguarded here: the caller puts
/// them behind the admin check.
pub fn library_routes(pool: SqlitePool) -> Router {
    Router::new()
        .route("/api/v1/libraries", get(list_libraries))
        .route("/api/v1/libraries/scan", axum::routing::post(scan_soon))
        .route(
            "/api/v1/libraries/{name}/scan",
            axum::routing::post(scan_library),
        )
        .route(
            "/api/v1/movies/{id}/rescan",
            axum::routing::post(rescan_movie),
        )
        .route(
            "/api/v1/series/{id}/rescan",
            axum::routing::post(rescan_series),
        )
        .route("/api/v1/metadata/unmatched", get(list_unmatched))
        .with_state(pool)
}

/// Asks for a scan (and TMDB matching and probing) now rather than at the next scheduled run.
pub(super) async fn scan_soon() -> StatusCode {
    crate::jobs::request_rescan();
    StatusCode::ACCEPTED
}

/// Asks for one library to be scanned now, and its new files matched and read.
pub(super) async fn scan_library(
    State(pool): State<SqlitePool>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE name = ?")
        .bind(&name)
        .fetch_optional(&pool)
        .await?;
    let id = id.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such library"))?;
    crate::jobs::request_folder_scan(id, None);
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn rescan_movie(
    State(pool): State<SqlitePool>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<StatusCode, ApiError> {
    rescan_title(&pool, "movies", id).await
}

pub(super) async fn rescan_series(
    State(pool): State<SqlitePool>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Result<StatusCode, ApiError> {
    rescan_title(&pool, "series", id).await
}

/// Asks for a movie's or series' folder alone to be scanned now (and its new files matched
/// and read), for "I just copied something in".
async fn rescan_title(pool: &SqlitePool, table: &str, id: i64) -> Result<StatusCode, ApiError> {
    let found: Option<(i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT library_id, rel_path FROM {table} WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let (library, folder) =
        found.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such title"))?;
    crate::jobs::request_folder_scan(library, Some(folder));
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}
