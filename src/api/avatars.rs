//! Profile pictures: setting one's own (an upload, or a pick from the gallery), removing it,
//! anyone's for anyone signed in, and the gallery to pick from.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::aoide::library::{self, MAX_IMAGE_BYTES};
use crate::avatars::{self, Gallery};

/// The most gallery pictures in one page.
const MAX_PAGE: usize = 200;

#[derive(Clone)]
struct AvatarState {
    pool: SqlitePool,
    gallery: Arc<Gallery>,
}

/// The routes, with the gallery from `MELETE_AVATARS_DIR` and Melete's own pictures.
pub fn routes(pool: SqlitePool) -> Router {
    let official = std::path::Path::new(avatars::OFFICIAL_DIR);
    let gallery = Gallery {
        official: official.is_dir().then(|| official.to_path_buf()),
        folder: std::env::var_os("MELETE_AVATARS_DIR")
            .filter(|v| !v.is_empty())
            .map(Into::into),
    };
    routes_with(pool, gallery)
}

/// [`routes`] with a gallery of the caller's.
pub fn routes_with(pool: SqlitePool, gallery: Gallery) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route(
            "/api/v1/me/avatar",
            put(upload)
                .delete(remove)
                .layer(DefaultBodyLimit::max(MAX_IMAGE_BYTES)),
        )
        .route("/api/v1/me/avatar/gallery", put(pick))
        .route("/api/v1/users/{id}/avatar", get(of_user))
        .route("/api/v1/avatars", get(categories))
        .route("/api/v1/avatars/pictures", get(pictures))
        .route("/api/v1/avatars/picture", get(picture))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(AvatarState {
            pool,
            gallery: Arc::new(gallery),
        })
}

/// What `/me` and lists of people say of a user's picture.
async fn answer(pool: &SqlitePool, user_id: i64) -> Result<Json<Value>, ApiError> {
    let url = avatars::url_of(pool, user_id).await?;
    Ok(Json(json!({ "avatar_url": url })))
}

/// Sets one's picture from an upload: JPEG, PNG or WebP, up to 5 MB, its bytes starting as
/// its type's do.
async fn upload(
    State(state): State<AvatarState>,
    Extension(current): Extension<CurrentUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let mime = super::aoide_library::image_type(&headers, &body)?;
    if !library::looks_like(&mime, &body) {
        return Err(super::bad_request(
            "the file isn't the JPEG, PNG or WebP image its Content-Type says",
        ));
    }
    avatars::set(&state.pool, current.user.id, &mime, &body).await?;
    answer(&state.pool, current.user.id).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pick {
    /// A picture's id from the gallery.
    id: String,
}

/// Sets one's picture to a copy of one from the gallery.
async fn pick(
    State(state): State<AvatarState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Pick>,
) -> Result<Json<Value>, ApiError> {
    let no_such = || ApiError::new(StatusCode::NOT_FOUND, "no such picture in the gallery");
    let path = state.gallery.file(&body.id).ok_or_else(no_such)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| no_such())?;
    let mime = ["image/jpeg", "image/png", "image/webp"]
        .into_iter()
        .find(|m| library::looks_like(m, &bytes))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "that gallery picture isn't a JPEG, PNG or WebP image",
            )
        })?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "that gallery picture is over 5 MB",
        ));
    }
    avatars::set(&state.pool, current.user.id, mime, &bytes).await?;
    answer(&state.pool, current.user.id).await
}

/// Removes one's picture.
async fn remove(
    State(state): State<AvatarState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    avatars::clear(&state.pool, current.user.id).await?;
    answer(&state.pool, current.user.id).await
}

/// Someone's picture, for anyone signed in.
async fn of_user(
    State(state): State<AvatarState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let avatar = avatars::get(&state.pool, id)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no picture"))?;
    Ok(image(avatar.mime, avatar.bytes, &avatar.hash))
}

/// An image, kept by the browser: the address carries the picture's hash, so a new picture has
/// a new address.
fn image(mime: String, bytes: Vec<u8>, tag: &str) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_owned(),
            ),
            (header::ETAG, format!("\"{tag}\"")),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
        ],
        bytes,
    )
        .into_response()
}

/// The gallery's categories, Melete's own first.
async fn categories(State(state): State<AvatarState>) -> Json<Value> {
    Json(json!({ "categories": state.gallery.categories() }))
}

#[derive(Deserialize)]
struct Page {
    category: String,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

/// A page of a category's pictures: each one's id and address.
async fn pictures(
    State(state): State<AvatarState>,
    Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
    let limit = page.limit.unwrap_or(60).clamp(1, MAX_PAGE);
    let (total, ids) = state
        .gallery
        .pictures(&page.category, page.offset, limit)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such category"))?;
    let pictures: Vec<Value> = ids
        .into_iter()
        .map(|id| {
            // reqwest's URL type encodes the id; only its path and query are kept.
            let url = reqwest::Url::parse_with_params(
                "http://melete/api/v1/avatars/picture",
                [("id", &id)],
            )
            .map(|u| format!("{}?{}", u.path(), u.query().unwrap_or_default()))
            .unwrap_or_default();
            json!({ "id": id, "url": url })
        })
        .collect();
    Ok(Json(json!({ "total": total, "pictures": pictures })))
}

#[derive(Deserialize)]
struct One {
    id: String,
}

/// One gallery picture.
async fn picture(
    State(state): State<AvatarState>,
    Query(one): Query<One>,
) -> Result<Response, ApiError> {
    let no_such = || ApiError::new(StatusCode::NOT_FOUND, "no such picture in the gallery");
    let path = state.gallery.file(&one.id).ok_or_else(no_such)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| no_such())?;
    let mime = ["image/jpeg", "image/png", "image/webp"]
        .into_iter()
        .find(|m| library::looks_like(m, &bytes))
        .ok_or_else(no_such)?;
    let tag = library::sha256_hex(&bytes);
    Ok(image(mime.to_owned(), bytes, &tag))
}

#[cfg(test)]
mod tests;
