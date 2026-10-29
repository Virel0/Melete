//! What each user may see, set by admins: the libraries (every one, or some by name) and the
//! highest content rating. Admins see everything whatever is set here.

use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::auth::{AppState, CurrentUser, require_admin};
use super::{ApiError, bad_request};
use crate::access::{self, Restriction};

/// The highest age a limit may be.
const OLDEST: i64 = 18;

/// The routes, for admins.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/users/{id}/access", get(show).put(replace))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

/// A layer, inside [`super::auth::require_user`], letting through only users who may see every
/// music library: Aoide's sync and what listeners share carry tracks in payloads Melete doesn't
/// read, so they can't be narrowed to some libraries.
pub(super) async fn require_all_music(
    State(pool): State<SqlitePool>,
    request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let sees_all = match request.extensions().get::<CurrentUser>() {
        Some(current) => current.access.sees_all_music(&pool).await?,
        None => false,
    };
    if !sees_all {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "this account sees only some music libraries, which Aoide's sync, playlists, \
             listening together and sharing don't support",
        ));
    }
    Ok(next.run(request).await)
}

/// Whether the user is an admin; a 404 when there's no such user.
async fn is_admin(pool: &SqlitePool, user_id: i64) -> Result<bool, ApiError> {
    let admin: Option<bool> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    admin.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such user"))
}

async fn answer(pool: &SqlitePool, user_id: i64) -> Result<Json<Value>, ApiError> {
    let admin = is_admin(pool, user_id).await?;
    let restriction = access::restriction(pool, user_id).await?;
    let (libraries, max_age) = match restriction {
        None => (None, None),
        Some(r) => (r.libraries, r.max_age),
    };
    let names: Option<Vec<String>> = match libraries {
        None => None,
        Some(ids) => Some(
            sqlx::query_scalar(
                "SELECT name FROM libraries WHERE id IN (SELECT value FROM json_each(?))
                 ORDER BY name",
            )
            .bind(serde_json::to_string(&ids)?)
            .fetch_all(pool)
            .await?,
        ),
    };
    Ok(Json(json!({
        "user_id": user_id,
        "admin": admin,
        "libraries": names,
        "max_age": max_age,
    })))
}

async fn show(
    State(pool): State<SqlitePool>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    answer(&pool, user_id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccessBody {
    /// Library names; null for every library.
    libraries: Option<Vec<String>>,
    max_age: Option<i64>,
}

async fn replace(
    State(pool): State<SqlitePool>,
    Path(user_id): Path<i64>,
    Json(body): Json<AccessBody>,
) -> Result<Json<Value>, ApiError> {
    is_admin(&pool, user_id).await?;
    if body.max_age.is_some_and(|a| !(0..=OLDEST).contains(&a)) {
        return Err(bad_request(&format!("max_age is 0 to {OLDEST}, or null")));
    }
    let libraries = match &body.libraries {
        None => None,
        Some(names) => {
            let mut ids = Vec::new();
            for name in names {
                let id: Option<i64> = sqlx::query_scalar("SELECT id FROM libraries WHERE name = ?")
                    .bind(name)
                    .fetch_optional(&pool)
                    .await?;
                let id =
                    id.ok_or_else(|| bad_request(&format!("no library is called “{name}”")))?;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            Some(ids)
        }
    };
    let restriction = Restriction {
        libraries,
        max_age: body.max_age,
    };
    access::set_restriction(&pool, user_id, Some(&restriction)).await?;
    answer(&pool, user_id).await
}

#[cfg(test)]
mod tests;
