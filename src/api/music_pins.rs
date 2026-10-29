//! Pinned albums, artists and playlists: up to six per user, kept on the server so
//! every device shows the same, in the user's order. A small API of its own rather than an
//! entity in Aoide's sync log: the six-pin limit and checking that each pin is something the
//! user can see need one place to decide, apps that don't sync (the web UI) pin too, and a
//! change reaches every device through the event stream's `pins`.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::{ALBUM_COLUMNS, AlbumRow, Shown, artists_by_id, bad_request};
use crate::db::unix_now;

/// Most pins one user keeps.
pub const MAX_PINS: usize = 6;

#[derive(Clone)]
struct PinState {
    pool: SqlitePool,
    /// Whether covers come from the Cover Art Archive (with MusicBrainz matching on).
    archive: bool,
}

/// The pin routes, for everyone signed in. `archive`: whether covers come from the Cover Art Archive.
pub fn routes(pool: SqlitePool, archive: bool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/pins", get(list).put(replace))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(PinState { pool, archive })
}

/// A user's pins in order, as (kind, item), and their version (0 when never pinned).
pub async fn pins(pool: &SqlitePool, user_id: i64) -> sqlx::Result<(Vec<(String, String)>, i64)> {
    let items: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, item FROM music_pins WHERE user_id = ? ORDER BY position")
            .bind(user_id)
            .fetch_all(pool)
            .await?;
    let version: Option<i64> =
        sqlx::query_scalar("SELECT version FROM music_pin_lists WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok((items, version.unwrap_or(0)))
}

/// What the event stream sends as `pins`: none for a user who has never pinned anything.
pub async fn event(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Option<String>> {
    let (items, version) = pins(pool, user_id).await?;
    if version == 0 {
        return Ok(None);
    }
    let items: Vec<Value> = items
        .iter()
        .map(|(kind, id)| json!({ "kind": kind, "id": id }))
        .collect();
    Ok(Some(
        json!({ "version": version, "items": items }).to_string(),
    ))
}

/// The pins with what each is, as the music API and the playlists API show it; null for one
/// that's gone from the library or is no longer shared with the user.
async fn shown(state: &PinState, current: &CurrentUser) -> Result<Value, ApiError> {
    let user_id = current.user.id;
    let (items, version) = pins(&state.pool, user_id).await?;
    let numbers = |kind: &str| -> Vec<i64> {
        items
            .iter()
            .filter(|(k, _)| k == kind)
            .filter_map(|(_, id)| id.parse().ok())
            .collect()
    };
    let album_ids = numbers("album");
    let albums: Vec<AlbumRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ALBUM_COLUMNS} FROM music_albums a
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.missing_since IS NULL AND a.id IN (SELECT value FROM json_each(?)) AND {}",
        current.access.album_sql("a")
    )))
    .bind(serde_json::to_string(&album_ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(&state.pool)
    .await?;
    let artists = artists_by_id(&state.pool, &current.access, &numbers("artist")).await?;
    let shown_as = Shown::to(&state.pool, state.archive, user_id).await?;
    let playlists = if items.iter().any(|(k, _)| k == "playlist") {
        crate::aoide::playlists::visible(&state.pool, user_id).await?
    } else {
        Vec::new()
    };
    let folders = if playlists.is_empty() {
        Default::default()
    } else {
        crate::aoide::playlists::folders(&state.pool, user_id).await?
    };
    let shown: Vec<Value> = items
        .iter()
        .map(|(kind, id)| {
            let number: Option<i64> = id.parse().ok();
            let album = (kind == "album")
                .then(|| albums.iter().find(|a| Some(a.id) == number))
                .flatten()
                .map(|a| a.to_json(shown_as));
            let artist = (kind == "artist")
                .then(|| number.and_then(|n| artists.get(&n).cloned()))
                .flatten();
            let playlist = (kind == "playlist")
                .then(|| playlists.iter().find(|p| &p.id == id))
                .flatten()
                .map(|p| super::playlists::summary(p, user_id, &folders));
            json!({ "kind": kind, "id": id, "album": album, "artist": artist,
                    "playlist": playlist })
        })
        .collect();
    Ok(json!({ "version": version, "items": shown }))
}

async fn list(
    State(state): State<PinState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(shown(&state, &current).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    kind: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewPins {
    items: Vec<Pin>,
}

/// Puts the user's pins as given, in that order: pinning, unpinning and moving are all this.
async fn replace(
    State(state): State<PinState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<NewPins>,
) -> Result<Json<Value>, ApiError> {
    let me = current.user.id;
    if body.items.len() > MAX_PINS {
        return Err(bad_request(&format!("at most {MAX_PINS} pins")));
    }
    let mut seen = HashSet::new();
    for pin in &body.items {
        if !seen.insert((pin.kind.as_str(), pin.id.as_str())) {
            return Err(bad_request("a pin is listed twice"));
        }
    }
    let mut playlists: Option<Vec<String>> = None;
    for pin in &body.items {
        let known = match pin.kind.as_str() {
            "album" => match pin.id.parse::<i64>() {
                Ok(id) => {
                    sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM music_albums
                         WHERE id = ? AND missing_since IS NULL)",
                    )
                    .bind(id)
                    .fetch_one(&state.pool)
                    .await?
                        && current.access.sees_album(&state.pool, id).await?
                }
                Err(_) => false,
            },
            "artist" => match pin.id.parse::<i64>() {
                Ok(id) => !artists_by_id(&state.pool, &current.access, &[id])
                    .await?
                    .is_empty(),
                Err(_) => false,
            },
            "playlist" => {
                if playlists.is_none() {
                    playlists = Some(
                        crate::aoide::playlists::visible(&state.pool, me)
                            .await?
                            .into_iter()
                            .map(|p| p.id)
                            .collect(),
                    );
                }
                playlists.iter().flatten().any(|p| *p == pin.id)
            }
            _ => return Err(bad_request("a pin's kind is album, artist or playlist")),
        };
        if !known {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                format!("there's no {} {:?} to pin", pin.kind, pin.id),
            ));
        }
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO music_pin_lists (user_id, version, updated_at) VALUES (?1, 1, ?2)
         ON CONFLICT (user_id) DO UPDATE SET version = version + 1, updated_at = ?2",
    )
    .bind(me)
    .bind(unix_now())
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM music_pins WHERE user_id = ?")
        .bind(me)
        .execute(&mut *tx)
        .await?;
    for (position, pin) in body.items.iter().enumerate() {
        sqlx::query("INSERT INTO music_pins (user_id, position, kind, item) VALUES (?, ?, ?, ?)")
            .bind(me)
            .bind(i64::try_from(position).unwrap_or(0))
            .bind(&pin.kind)
            .bind(&pin.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(shown(&state, &current).await?))
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

    async fn call(app: &Router, token: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(if body.is_some() { "PUT" } else { "GET" })
            .uri("/api/v1/music/pins")
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
    async fn each_user_keeps_up_to_six_pins_in_their_order() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_artists (id, key, name) VALUES (1, 'name:alder', 'Alder'),
                 (2, 'name:gone', 'Gone');
             INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                 seen_generation, missing_since) VALUES
                 (1, 1, 'k1', 'One', 1, 'Alder', 'a', 1, NULL),
                 (2, 1, 'k2', 'Two', 1, 'Alder', 'b', 1, NULL),
                 (3, 1, 'k3', 'Old', 2, 'Gone', 'c', 1, 5);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, alice_token) = user(&pool, "alice").await;
        let (_, bob_token) = user(&pool, "bob").await;
        let op = r#"{"opId":"p","entity":"playlists","entityId":"pl-1","operation":"upsert","payload":{"id":"pl-1","name":"Mix","updatedAt":1},"createdAt":1}"#;
        let op = sync::validate(&RawValue::from_string(op.into()).unwrap()).unwrap();
        sync::push(&pool, alice, "phone", vec![op], 1)
            .await
            .unwrap();
        let app = routes(pool.clone(), false);

        let (_, empty) = call(&app, &alice_token, None).await;
        assert_eq!(empty, json!({ "version": 0, "items": [] }));
        assert_eq!(event(&pool, alice).await.unwrap(), None, "never pinned");

        let (status, pinned) = call(
            &app,
            &alice_token,
            Some(json!({ "items": [
                { "kind": "playlist", "id": "pl-1" },
                { "kind": "album", "id": "2" },
                { "kind": "artist", "id": "1" },
            ]})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{pinned}");
        assert_eq!(pinned["version"], 1);
        assert_eq!(pinned["items"][0]["playlist"]["name"], "Mix");
        assert_eq!(pinned["items"][0]["album"], Value::Null);
        assert_eq!(pinned["items"][1]["album"]["title"], "Two");
        assert_eq!(pinned["items"][2]["artist"]["name"], "Alder");
        assert_eq!(
            event(&pool, alice).await.unwrap().unwrap(),
            r#"{"items":[{"id":"pl-1","kind":"playlist"},{"id":"2","kind":"album"},{"id":"1","kind":"artist"}],"version":1}"#
        );
        // Another device sees the same; bob has his own.
        assert_eq!(call(&app, &alice_token, None).await.1, pinned);
        assert_eq!(call(&app, &bob_token, None).await.1["items"], json!([]));

        // Moving and unpinning are a new list.
        let (_, moved) = call(
            &app,
            &alice_token,
            Some(json!({ "items": [{ "kind": "artist", "id": "1" }, { "kind": "album", "id": "2" }] })),
        )
        .await;
        assert_eq!(moved["version"], 2);
        assert_eq!(moved["items"][0]["kind"], "artist");
        assert_eq!(moved["items"].as_array().unwrap().len(), 2);

        for (items, want) in [
            (
                json!(vec![json!({ "kind": "album", "id": "1" }); 7]),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!([{ "kind": "album", "id": "1" }, { "kind": "album", "id": "1" }]),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!([{ "kind": "song", "id": "1" }]),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!([{ "kind": "album", "id": "3" }]),
                StatusCode::NOT_FOUND,
            ),
            (
                json!([{ "kind": "album", "id": "x" }]),
                StatusCode::NOT_FOUND,
            ),
            (
                json!([{ "kind": "artist", "id": "2" }]),
                StatusCode::NOT_FOUND,
            ),
            // Alice's playlist isn't shared with bob.
            (
                json!([{ "kind": "playlist", "id": "pl-1" }]),
                StatusCode::NOT_FOUND,
            ),
        ] {
            let token = if want == StatusCode::NOT_FOUND && items[0]["kind"] == "playlist" {
                &bob_token
            } else {
                &alice_token
            };
            let (status, answer) = call(&app, token, Some(json!({ "items": items }))).await;
            assert_eq!(status, want, "{items} → {answer}");
        }
        assert_eq!(
            call(&app, &alice_token, None).await.1["version"],
            2,
            "refusals change nothing"
        );

        // A pinned album gone from the library stays pinned, shown as null.
        sqlx::query("UPDATE music_albums SET missing_since = 9 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();
        let (_, now) = call(&app, &alice_token, None).await;
        assert_eq!(now["items"][1]["kind"], "album");
        assert_eq!(now["items"][1]["album"], Value::Null);
    }
}
