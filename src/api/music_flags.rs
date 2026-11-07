//! Track flags for apps that don't sync (the web UI): "not interested" and "don't count
//! plays", read from the caller's Aoide sync log and set by writing into it what a device of
//! theirs would ([`crate::aoide::track_flags`]), so every device follows. Like the rest of
//! what's in that log, they're closed to a user who may see only some music libraries.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::{bad_request, tracks_by_id};
use super::music_likes::write_failed;
use crate::aoide::listening::normalise;
use crate::aoide::track_flags::{self, Flag};
use crate::aoide::writing::session_device;
use crate::db::now_ms;

/// The flag routes, for everyone signed in who may see every music library.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/flags", get(list))
        .route("/api/v1/music/tracks/{id}/flags", put(set))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// The caller's flagged tracks (either flag set), the latest change first, each with its track
/// as the music API shows it, or null when it isn't in the library now (its content key says
/// what it was).
async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let flagged: Vec<_> = track_flags::all(&pool, current.user.id)
        .await?
        .into_iter()
        .filter(|f| f.not_interested || f.dont_count)
        .collect();
    let ids: Vec<i64> = flagged
        .iter()
        .filter_map(|f| normalise(&f.track).parse().ok())
        .collect();
    let tracks = tracks_by_id(&pool, &current.access, current.user.id, &ids).await?;
    let items: Vec<Value> = flagged
        .iter()
        .map(|f| {
            let track = normalise(&f.track)
                .parse::<i64>()
                .ok()
                .and_then(|id| tracks.get(&id).cloned());
            json!({
                "track": track,
                "content_key": f.content_key,
                "not_interested": f.not_interested,
                "dont_count": f.dont_count,
                "changed_at": f.updated_at.div_euclid(1000),
            })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    not_interested: Option<bool>,
    dont_count: Option<bool>,
}

/// Sets or clears a track's flags: those the body names, the others as they were.
async fn set(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<Change>,
) -> Result<Json<Value>, ApiError> {
    let changes: Vec<(Flag, bool)> = [
        (Flag::NotInterested, body.not_interested),
        (Flag::DontCount, body.dont_count),
    ]
    .into_iter()
    .filter_map(|(flag, on)| Some((flag, on?)))
    .collect();
    if changes.is_empty() {
        return Err(bad_request("say which: not_interested, dont_count or both"));
    }
    let setting = changes.iter().any(|(_, on)| *on);
    if setting && !current.access.sees_track(&pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    let device = session_device(current.token_id);
    for (flag, on) in changes {
        let done = track_flags::set(&pool, current.user.id, &device, id, flag, on, now_ms())
            .await
            .map_err(write_failed)?;
        if !done {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
        }
    }
    let now = track_flags::all(&pool, current.user.id)
        .await?
        .into_iter()
        .find(|f| normalise(&f.track) == id.to_string());
    Ok(Json(json!({
        "not_interested": now.as_ref().is_some_and(|f| f.not_interested),
        "dont_count": now.as_ref().is_some_and(|f| f.dont_count),
    })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::aoide::listening;
    use crate::auth::{self, TokenKind};

    async fn setup() -> (SqlitePool, i64, String, i64) {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'Album', 'Alice', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres)
                 VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'First', 'Alice', '[]'),
                        (8, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Second', 'Alice', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::testing::user(&pool, "alice", false).await;
        let (token_id, token) = auth::issue_token(&pool, user.id, TokenKind::Session, "Laptop")
            .await
            .unwrap();
        (pool, user.id, token, token_id)
    }

    async fn call(
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

    async fn flag_ops(pool: &SqlitePool) -> Vec<(String, String, Value)> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT device_id, entity_id, payload FROM aoide_ops WHERE entity = 'track_flags' ORDER BY seq",
        )
        .fetch_all(pool)
        .await
        .unwrap();
        rows.into_iter()
            .map(|(d, e, p)| (d, e, serde_json::from_str(&p).unwrap()))
            .collect()
    }

    #[tokio::test]
    async fn setting_a_flag_writes_the_row_a_device_would() {
        let (pool, user, token, token_id) = setup().await;
        let app = routes(pool.clone());

        let (status, now) = call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/tracks/7/flags",
            Some(json!({ "not_interested": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{now}");
        assert_eq!(now, json!({ "not_interested": true, "dont_count": false }));
        let ops = flag_ops(&pool).await;
        assert_eq!(ops.len(), 1);
        let (device, entity, row) = &ops[0];
        assert_eq!(device, &format!("token-{token_id}"));
        assert_eq!(entity, row["id"].as_str().unwrap());
        let at = row["updatedAt"].clone();
        assert_eq!(
            row,
            &json!({
                "id": row["id"].clone(), "jellyfinId": "7", "contentKey": row["contentKey"].clone(),
                "notInterested": true, "dontCount": false, "updatedAt": at.clone(),
                "deleted": false, "originDevice": format!("token-{token_id}"),
                "fieldUpdatedAt": { "notInterested": at }
            })
        );
        assert!(
            listening::not_interested(&pool, user)
                .await
                .unwrap()
                .contains("7")
        );

        // Already so: nothing more is written.
        call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/tracks/7/flags",
            Some(json!({ "not_interested": true })),
        )
        .await;
        assert_eq!(flag_ops(&pool).await.len(), 1);

        let (_, listed) = call(&app, &token, "GET", "/api/v1/music/flags", None).await;
        assert_eq!(listed["items"].as_array().unwrap().len(), 1);
        assert_eq!(listed["items"][0]["track"]["title"], json!("First"));
        assert_eq!(listed["items"][0]["not_interested"], json!(true));

        // Cleared, it leaves the list and the flags the server reads.
        call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/tracks/7/flags",
            Some(json!({ "not_interested": false })),
        )
        .await;
        assert!(
            listening::not_interested(&pool, user)
                .await
                .unwrap()
                .is_empty()
        );
        let (_, listed) = call(&app, &token, "GET", "/api/v1/music/flags", None).await;
        assert_eq!(listed["items"], json!([]));
    }

    #[tokio::test]
    async fn a_devices_row_keeps_its_id_and_its_other_flag() {
        let (pool, user, token, _) = setup().await;
        // The phone flagged track 8 "don't count" before stamps existed.
        sqlx::query(
            "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                 payload, created_at, received_at)
             VALUES ('p1', ?, 'phone', 'track_flags', 'ROW-8', 'upsert',
                 '{\"id\":\"ROW-8\",\"jellyfinId\":\"8\",\"contentKey\":\"alice|album|second|0\",\"notInterested\":false,\"dontCount\":true,\"updatedAt\":1000,\"deleted\":false,\"originDevice\":\"phone\"}',
                 1, 1)",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone());

        let (status, now) = call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/tracks/8/flags",
            Some(json!({ "not_interested": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(now, json!({ "not_interested": true, "dont_count": true }));
        let ops = flag_ops(&pool).await;
        let (_, entity, row) = ops.last().unwrap();
        assert_eq!((entity.as_str(), &row["id"]), ("ROW-8", &json!("ROW-8")));
        assert_eq!(
            (&row["dontCount"], &row["notInterested"]),
            (&json!(true), &json!(true))
        );
        assert_eq!(
            row["fieldUpdatedAt"]["dontCount"],
            json!(1000),
            "not claimed by this change"
        );
        assert_eq!(row["fieldUpdatedAt"]["notInterested"], row["updatedAt"]);
        let held = track_flags::all(&pool, user).await.unwrap();
        assert!(
            held.iter()
                .any(|f| f.track == "8" && f.dont_count && f.not_interested)
        );
    }

    #[tokio::test]
    async fn refuses_a_change_that_names_nothing_or_no_track() {
        let (pool, _, token, _) = setup().await;
        let app = routes(pool.clone());
        let (status, _) = call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/tracks/7/flags",
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(
            &app,
            &token,
            "PUT",
            "/api/v1/music/tracks/99/flags",
            Some(json!({ "dont_count": true })),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(flag_ops(&pool).await.is_empty());
    }
}
