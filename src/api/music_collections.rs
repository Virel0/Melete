//! Collections: a listener's saved song filters, each a name for several genres, a span of
//! years and "only songs I like", as the songs list takes them (`/music/tracks`'s `genres`,
//! `year_from`, `year_to` and `liked`). Each user's are their own: no one else lists, reads
//! or deletes them. They hold no music, only the filter, so what they show is always what the
//! listener may see now.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::bad_request;
use crate::db::unix_now;

/// Most collections one user keeps.
pub const MAX_COLLECTIONS: i64 = 50;
/// Most genres in one.
const MAX_GENRES: usize = 50;
/// Longest name, in characters.
const MAX_NAME: usize = 100;

/// The collection routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/collections", get(list).post(create))
        .route("/api/v1/music/collections/{id}", delete(remove))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    name: String,
    genres: String,
    year_from: Option<i64>,
    year_to: Option<i64>,
    liked: bool,
    created_at: i64,
}

impl Row {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "genres": serde_json::from_str::<Value>(&self.genres).unwrap_or_else(|_| json!([])),
            "year_from": self.year_from,
            "year_to": self.year_to,
            "liked": self.liked,
            "created_at": self.created_at,
        })
    }
}

const COLUMNS: &str = "id, name, genres, year_from, year_to, liked, created_at";

/// The caller's collections, by name.
async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM music_collections WHERE user_id = ? ORDER BY name COLLATE NOCASE, id"
    )))
    .bind(current.user.id)
    .fetch_all(&pool)
    .await?;
    Ok(Json(json!({
        "items": rows.iter().map(Row::to_json).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewCollection {
    name: String,
    #[serde(default)]
    genres: Vec<String>,
    year_from: Option<i64>,
    year_to: Option<i64>,
    #[serde(default)]
    liked: bool,
}

/// What's wrong with `new`, in a sentence, or its genres tidied: trimmed, empty ones and
/// repeats (in any case) dropped.
fn checked(new: &NewCollection) -> Result<Vec<String>, ApiError> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(bad_request("name is 1 to 100 characters"));
    }
    let mut genres: Vec<String> = Vec::new();
    for genre in &new.genres {
        let genre = genre.trim();
        if genre.contains(';') {
            return Err(bad_request("a genre has no semicolon"));
        }
        if !genre.is_empty() && !genres.iter().any(|g| g.eq_ignore_ascii_case(genre)) {
            genres.push(genre.to_owned());
        }
    }
    if genres.len() > MAX_GENRES {
        return Err(bad_request("a collection has up to 50 genres"));
    }
    for year in [new.year_from, new.year_to].into_iter().flatten() {
        if !(1..=9999).contains(&year) {
            return Err(bad_request("years are 1 to 9999"));
        }
    }
    if let (Some(from), Some(to)) = (new.year_from, new.year_to)
        && from > to
    {
        return Err(bad_request("year_from is at most year_to"));
    }
    if genres.is_empty() && new.year_from.is_none() && new.year_to.is_none() && !new.liked {
        return Err(bad_request(
            "a collection needs a genre, a year or liked songs",
        ));
    }
    Ok(genres)
}

/// Saves a new collection for the caller; a name they already use is refused.
async fn create(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(new): Json<NewCollection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let genres = checked(&new)?;
    let me = current.user.id;
    let mut tx = pool.begin().await?;
    let (count, taken): (i64, bool) = sqlx::query_as(
        "SELECT count(*), coalesce(max(name = ? COLLATE NOCASE), 0)
         FROM music_collections WHERE user_id = ?",
    )
    .bind(new.name.trim())
    .bind(me)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "you have a collection of that name",
        ));
    }
    if count >= MAX_COLLECTIONS {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "you can keep up to 50 collections; delete one first",
        ));
    }
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO music_collections (user_id, name, genres, year_from, year_to, liked,
             created_at) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING {COLUMNS}"
    )))
    .bind(me)
    .bind(new.name.trim())
    .bind(serde_json::to_string(&genres)?)
    .bind(new.year_from)
    .bind(new.year_to)
    .bind(new.liked)
    .bind(unix_now())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.to_json())))
}

/// Deletes one of the caller's collections; anyone else's is answered as not there.
async fn remove(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let gone = sqlx::query("DELETE FROM music_collections WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(current.user.id)
        .execute(&pool)
        .await?
        .rows_affected();
    if gone == 0 {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such collection"));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn token(pool: &SqlitePool, name: &str) -> String {
        let user = crate::auth::testing::user(pool, name, false).await;
        crate::auth::issue_token(pool, user.id, crate::auth::TokenKind::Session, "t")
            .await
            .unwrap()
            .1
    }

    async fn call(
        app: &Router,
        token: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let body = body.map_or_else(Body::empty, |b| Body::from(b.to_string()));
        let response = app
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn each_listener_keeps_their_own_collections() {
        let pool = crate::db::open_in_memory().await;
        let alice = token(&pool, "alice").await;
        let bob = token(&pool, "bob").await;
        let app = routes(pool.clone());
        let list = "/api/v1/music/collections";
        let jazz = json!({ "name": " Late jazz ", "genres": ["Jazz", " jazz", "Soul", ""],
            "year_from": 1955, "year_to": 1969 });
        let (status, made) = call(&app, &alice, "POST", list, Some(jazz.clone())).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(made["name"], "Late jazz");
        assert_eq!(
            made["genres"],
            json!(["Jazz", "Soul"]),
            "trimmed, repeats dropped"
        );
        assert_eq!(made["liked"], false);
        let liked = json!({ "name": "Loved", "liked": true });
        assert_eq!(
            call(&app, &alice, "POST", list, Some(liked)).await.0,
            StatusCode::CREATED
        );
        // The same name again (in any case) is refused.
        let again = json!({ "name": "LATE JAZZ", "liked": true });
        assert_eq!(
            call(&app, &alice, "POST", list, Some(again)).await.0,
            StatusCode::CONFLICT
        );
        let (_, mine) = call(&app, &alice, "GET", list, None).await;
        let names: Vec<&str> = mine["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Late jazz", "Loved"]);

        // Bob sees none of Alice's, can't delete them, and may use the same name.
        let (_, his) = call(&app, &bob, "GET", list, None).await;
        assert_eq!(his["items"], json!([]));
        let id = made["id"].as_i64().unwrap();
        let one = format!("{list}/{id}");
        assert_eq!(
            call(&app, &bob, "DELETE", &one, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(&app, &bob, "POST", list, Some(jazz)).await.0,
            StatusCode::CREATED
        );
        assert_eq!(
            call(&app, &alice, "DELETE", &one, None).await.0,
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            call(&app, &alice, "DELETE", &one, None).await.0,
            StatusCode::NOT_FOUND
        );
        let (_, mine) = call(&app, &alice, "GET", list, None).await;
        assert_eq!(mine["items"].as_array().unwrap().len(), 1);
        let (_, his) = call(&app, &bob, "GET", list, None).await;
        assert_eq!(
            his["items"].as_array().unwrap().len(),
            1,
            "his own is untouched"
        );
    }

    #[tokio::test]
    async fn a_collection_needs_a_name_and_something_to_filter_by() {
        let pool = crate::db::open_in_memory().await;
        let alice = token(&pool, "alice").await;
        let app = routes(pool.clone());
        let list = "/api/v1/music/collections";
        for body in [
            json!({ "name": "Nothing" }),
            json!({ "name": "  ", "liked": true }),
            json!({ "name": "Backwards", "year_from": 2000, "year_to": 1990 }),
            json!({ "name": "Odd", "genres": ["Rock;Pop"] }),
            json!({ "name": "Far", "year_to": 12345 }),
        ] {
            assert_eq!(
                call(&app, &alice, "POST", list, Some(body.clone())).await.0,
                StatusCode::BAD_REQUEST,
                "{body}"
            );
        }
        let unknown = json!({ "name": "Extra", "liked": true, "mood": "calm" });
        assert_eq!(
            call(&app, &alice, "POST", list, Some(unknown)).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        for n in 0..MAX_COLLECTIONS {
            let body = json!({ "name": format!("C{n}"), "liked": true });
            assert_eq!(
                call(&app, &alice, "POST", list, Some(body)).await.0,
                StatusCode::CREATED
            );
        }
        let over = json!({ "name": "One more", "liked": true });
        assert_eq!(
            call(&app, &alice, "POST", list, Some(over)).await.0,
            StatusCode::CONFLICT
        );
    }
}
