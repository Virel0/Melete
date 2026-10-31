//! Aoide's playlists for apps that don't sync: the playlists a user owns or that are shared
//! with them, one with its tracks as Melete has them, and editing one's own, written into the
//! sync log as an Aoide device would write it (`edits`).

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::aoide::listening::normalise;
use crate::aoide::playlists::{self, Playlist};

mod edits;

use edits::*;

/// The playlist routes, for everyone signed in who may see every music library.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/playlists", get(list).post(create))
        .route(
            "/api/v1/playlists/{id}",
            get(one).put(update).delete(remove_playlist),
        )
        .route(
            "/api/v1/playlists/{id}/image",
            put(set_image)
                .delete(remove_image)
                .layer(DefaultBodyLimit::max(
                    crate::aoide::library::MAX_IMAGE_BYTES,
                )),
        )
        .route("/api/v1/playlists/{id}/items", post(add_items))
        .route(
            "/api/v1/playlists/{id}/items/{item}",
            put(move_item).delete(remove_item),
        )
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

fn str_of<'a>(row: &'a playlists::Row, key: &str) -> Option<&'a str> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// A playlist without its items.
pub(super) fn summary(
    p: &Playlist,
    me: i64,
    folders: &std::collections::HashMap<String, String>,
) -> Value {
    let r = &p.row;
    let image = str_of(r, "imageHash").map(str::to_ascii_lowercase);
    json!({
        "id": p.id,
        "name": str_of(r, "name").unwrap_or("Untitled playlist"),
        "notes": str_of(r, "notes"),
        // Folders are each user's own: only the owner's playlist is in one.
        "folder": if p.owner_id == me { str_of(r, "folderId").and_then(|f| folders.get(f)) } else { None },
        "smart": matches!(r.get("isSmart"), Some(Value::Bool(true))) || r.get("isSmart").and_then(Value::as_i64) == Some(1),
        "image_url": image.map(|h| format!("/aoide/images/{h}")),
        "owner": p.owner,
        "mine": p.owner_id == me,
        "can_edit": p.can_edit,
        "tracks": p.items.len(),
        "updated_at": r.get("updatedAt").and_then(Value::as_i64),
    })
}

async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let me = current.user.id;
    let (all, folders) = (
        playlists::visible(&pool, me).await?,
        playlists::folders(&pool, me).await?,
    );
    Ok(Json(all.iter().map(|p| summary(p, me, &folders)).collect()))
}

/// A playlist with its items in order, each with its track as the music API shows it, or null
/// when the track isn't in the library (its `content_key` says what it was).
async fn one(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

/// The playlist `id` as `me` sees it, with its items.
async fn detail(
    pool: &SqlitePool,
    access: &crate::access::Access,
    me: i64,
    id: &str,
) -> Result<Value, ApiError> {
    let Some(playlist) = playlists::visible(pool, me)
        .await?
        .into_iter()
        .find(|p| p.id == id)
    else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such playlist"));
    };
    let track_of = |item: &playlists::Row| -> Option<i64> {
        normalise(str_of(item, "jellyfinId")?).parse().ok()
    };
    let ids: Vec<i64> = playlist.items.iter().filter_map(track_of).collect();
    let tracks = super::music::tracks_by_id(pool, access, me, &ids).await?;
    let folders = playlists::folders(pool, me).await?;
    let mut body = summary(&playlist, me, &folders);
    body["items"] = playlist
        .items
        .iter()
        .map(|item| {
            json!({
                "id": str_of(item, "id"),
                "position": str_of(item, "position"),
                "content_key": str_of(item, "contentKey"),
                "track": track_of(item).and_then(|t| tracks.get(&t)).cloned(),
            })
        })
        .collect();
    Ok(body)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::value::RawValue;
    use tower::ServiceExt;

    use super::*;
    use crate::aoide::sync;
    use crate::auth::{self, TokenKind};

    async fn user(pool: &SqlitePool, name: &str) -> (i64, String) {
        let user = auth::create_user(pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        (user.id, token)
    }

    async fn push(pool: &SqlitePool, user: i64, ops: &[&str]) {
        let ops = ops
            .iter()
            .map(|op| sync::validate(&RawValue::from_string((*op).to_owned()).unwrap()).unwrap())
            .collect();
        sync::push(pool, user, "dev", ops, 1).await.unwrap();
    }

    async fn get(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
        let request = Request::get(uri)
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
    async fn shows_playlists_as_the_apps_have_them() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'Homogenic', 'Björk', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres)
                 VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Jóga', 'Björk', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let ((owner, owner_token), (friend, friend_token), (_, stranger_token)) = (
            user(&pool, "owner").await,
            user(&pool, "friend").await,
            user(&pool, "stranger").await,
        );
        push(
            &pool,
            owner,
            &[
                r#"{"opId":"f","entity":"folders","entityId":"d","operation":"upsert","payload":{"id":"d","name":"Moods","updatedAt":1},"createdAt":1}"#,
                r#"{"opId":"p1","entity":"playlists","entityId":"p","operation":"upsert","payload":{"id":"p","name":"Mix","folderId":"d","imageHash":"ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789","updatedAt":10,"originDevice":"phone"},"createdAt":10}"#,
                r#"{"opId":"i1","entity":"playlist_items","entityId":"i1","operation":"upsert","payload":{"id":"i1","playlistId":"p","jellyfinId":"7","contentKey":"björk|homogenic|jóga|305","position":"a1","updatedAt":11,"originDevice":"phone"},"createdAt":11}"#,
                r#"{"opId":"i2","entity":"playlist_items","entityId":"i2","operation":"upsert","payload":{"id":"i2","playlistId":"p","jellyfinId":"0123456789abcdef0123456789abcdef","contentKey":"gone|gone|gone|1","position":"a0","updatedAt":12,"originDevice":"phone"},"createdAt":12}"#,
                r#"{"opId":"i3","entity":"playlist_items","entityId":"i3","operation":"upsert","payload":{"id":"i3","playlistId":"p","jellyfinId":"7","position":"a2","updatedAt":13,"originDevice":"phone"},"createdAt":13}"#,
                r#"{"opId":"i3d","entity":"playlist_items","entityId":"i3","operation":"delete","payload":{"id":"i3","playlistId":"p","jellyfinId":"7","position":"a2","updatedAt":14,"originDevice":"phone"},"createdAt":14}"#,
                r#"{"opId":"x1","entity":"playlists","entityId":"gone","operation":"upsert","payload":{"id":"gone","name":"Old","updatedAt":5},"createdAt":5}"#,
                r#"{"opId":"x2","entity":"playlists","entityId":"gone","operation":"delete","payload":{"id":"gone","updatedAt":6},"createdAt":6}"#,
            ],
        )
        .await;
        crate::aoide::library::share(&pool, "p", friend, false, 20)
            .await
            .unwrap();
        // The friend can't edit it; renaming it is refused at push, and changes nothing here.
        push(
            &pool,
            friend,
            &[r#"{"opId":"r","entity":"playlists","entityId":"p","operation":"upsert","payload":{"id":"p","name":"Hijacked","updatedAt":99},"createdAt":99}"#],
        )
        .await;
        let app = routes(pool.clone());

        let (status, mine) = get(&app, &owner_token, "/api/v1/playlists").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            mine.as_array().unwrap().len(),
            1,
            "a deleted playlist isn't listed"
        );
        let p = &mine[0];
        assert_eq!(
            (
                &p["name"],
                &p["folder"],
                &p["mine"],
                &p["can_edit"],
                &p["tracks"],
                &p["owner"]
            ),
            (
                &json!("Mix"),
                &json!("Moods"),
                &json!(true),
                &json!(true),
                &json!(2),
                &json!("owner")
            )
        );
        assert_eq!(
            p["image_url"],
            "/aoide/images/abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );

        let (_, theirs) = get(&app, &friend_token, "/api/v1/playlists").await;
        assert_eq!(
            (
                &theirs[0]["name"],
                &theirs[0]["mine"],
                &theirs[0]["can_edit"],
                &theirs[0]["folder"]
            ),
            (&json!("Mix"), &json!(false), &json!(false), &Value::Null)
        );
        assert_eq!(
            get(&app, &stranger_token, "/api/v1/playlists").await.1,
            json!([])
        );

        let (status, one) = get(&app, &friend_token, "/api/v1/playlists/p").await;
        assert_eq!(status, StatusCode::OK);
        let items: Vec<(&Value, &Value)> = one["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (&i["id"], &i["track"]["title"]))
            .collect();
        assert_eq!(
            items,
            [(&json!("i2"), &Value::Null), (&json!("i1"), &json!("Jóga"))],
            "in order; a missing track is null"
        );
        assert_eq!(one["items"][0]["content_key"], "gone|gone|gone|1");
        assert_eq!(
            get(&app, &stranger_token, "/api/v1/playlists/p").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&app, &owner_token, "/api/v1/playlists/gone").await.0,
            StatusCode::NOT_FOUND
        );
    }
}
