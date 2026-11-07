//! What each listener started lately, for Home's Jump back in: albums, playlists and
//! stations (radio from a song), each recorded when it's started, and each removable. Removing
//! one hides it until it's started again; an album removed stays out of the albums Home finds
//! in the play history too. Each user's list is their own, and shows only what they may see
//! now: an album or song in a library closed to them, or a playlist no longer shared with them,
//! is left out.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router, middleware};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::bad_request;
use super::music::{Shown, albums_by_id, tracks_by_id};
use crate::aoide::playlists;
use crate::db::now_ms;

/// Most entries kept for one user; the oldest go first.
const KEPT: i64 = 50;
/// Most shown.
const SHOWN: usize = 20;
/// An entry's time (?4, now) for user ?1: later than any of theirs, so the latest started is
/// always first, even within one millisecond.
const LATER: &str = "max(?4, coalesce((SELECT max(played_at) + 1 FROM music_recent
    WHERE user_id = ?1), 0))";

#[derive(Clone)]
struct RecentState {
    pool: SqlitePool,
    /// Whether covers come from the Cover Art Archive (with MusicBrainz matching on).
    archive: bool,
}

/// The recent routes, for everyone signed in. `archive`: whether covers come from the Cover
/// Art Archive.
pub fn routes(pool: SqlitePool, archive: bool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/recent", get(list))
        .route(
            "/api/v1/music/recent/{kind}/{item}",
            put(record).delete(remove),
        )
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(RecentState { pool, archive })
}

/// What an entry is: an album's id, a playlist's id, or a station's seed song's id.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Album,
    Playlist,
    Station,
}

impl Kind {
    fn parse(kind: &str) -> Result<Self, ApiError> {
        match kind {
            "album" => Ok(Self::Album),
            "playlist" => Ok(Self::Playlist),
            "station" => Ok(Self::Station),
            _ => Err(bad_request("kind is album, playlist or station")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Playlist => "playlist",
            Self::Station => "station",
        }
    }
}

/// What `kind` and `item` are as the caller may see them now, or `None`.
async fn resolve(
    state: &RecentState,
    current: &CurrentUser,
    entries: &[(String, String)],
) -> Result<HashMap<(String, String), Value>, ApiError> {
    let pool = &state.pool;
    let shown = Shown {
        archive: state.archive,
    };
    let numbers = |kind: &str| -> Vec<i64> {
        entries
            .iter()
            .filter(|(k, _)| k == kind)
            .filter_map(|(_, i)| i.parse().ok())
            .collect()
    };
    let albums = albums_by_id(pool, &current.access, shown, &numbers("album")).await?;
    let songs = tracks_by_id(pool, &current.access, current.user.id, &numbers("station")).await?;
    // Playlists live in the sync log, closed to someone who may see only some libraries.
    let lists = if entries.iter().any(|(k, _)| k == "playlist")
        && current.access.sees_all_music(pool).await?
    {
        let folders = playlists::folders(pool, current.user.id).await?;
        playlists::visible(pool, current.user.id)
            .await?
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    super::playlists::summary(p, current.user.id, &folders),
                )
            })
            .collect()
    } else {
        HashMap::new()
    };
    let mut found = HashMap::new();
    for (kind, item) in entries {
        let number = item.parse::<i64>().ok();
        let value = match kind.as_str() {
            "album" => number.and_then(|id| albums.get(&id)).cloned(),
            "station" => number.and_then(|id| songs.get(&id)).cloned(),
            "playlist" => lists.get(item).cloned(),
            _ => None,
        };
        if let Some(value) = value {
            found.insert((kind.clone(), item.clone()), value);
        }
    }
    Ok(found)
}

/// The caller's entries, the latest started first, each with the album, the playlist or the
/// station's seed song as the API shows them; and the albums they removed, for Home to leave
/// out of what it finds in their play history.
async fn list(
    State(state): State<RecentState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<(String, String, i64, bool)> = sqlx::query_as(
        "SELECT kind, item, played_at, removed FROM music_recent WHERE user_id = ?
         ORDER BY played_at DESC, kind, item",
    )
    .bind(current.user.id)
    .fetch_all(&state.pool)
    .await?;
    let shown: Vec<(String, String)> = rows
        .iter()
        .filter(|r| !r.3)
        .map(|r| (r.0.clone(), r.1.clone()))
        .collect();
    let found = resolve(&state, &current, &shown).await?;
    let items: Vec<Value> = rows
        .iter()
        .filter(|r| !r.3)
        .filter_map(|(kind, item, played_at, _)| {
            let value = found.get(&(kind.clone(), item.clone()))?;
            let mut entry = json!({
                "kind": kind,
                "id": item,
                "played_at": played_at.div_euclid(1000),
                "album": null,
                "playlist": null,
                "track": null,
            });
            let slot = match kind.as_str() {
                "album" => "album",
                "playlist" => "playlist",
                _ => "track",
            };
            entry[slot] = value.clone();
            Some(entry)
        })
        .take(SHOWN)
        .collect();
    let removed: Vec<i64> = rows
        .iter()
        .filter(|r| r.3 && r.0 == "album")
        .filter_map(|r| r.1.parse().ok())
        .collect();
    Ok(Json(json!({ "items": items, "removed_albums": removed })))
}

/// Records that the caller started `kind` `item` now, bringing back one they'd removed. Only
/// something they may see is recorded.
async fn record(
    State(state): State<RecentState>,
    Extension(current): Extension<CurrentUser>,
    Path((kind, item)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let kind = Kind::parse(&kind)?;
    let entry = (kind.as_str().to_owned(), item.clone());
    if !resolve(&state, &current, std::slice::from_ref(&entry))
        .await?
        .contains_key(&entry)
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "nothing of that kind and id",
        ));
    }
    let me = current.user.id;
    let mut tx = state.pool.begin().await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO music_recent (user_id, kind, item, played_at, removed)
         VALUES (?1, ?2, ?3, {LATER}, 0)
         ON CONFLICT (user_id, kind, item) DO UPDATE SET played_at = excluded.played_at,
             removed = 0"
    )))
    .bind(me)
    .bind(kind.as_str())
    .bind(&item)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM music_recent WHERE user_id = ?1 AND rowid NOT IN
             (SELECT rowid FROM music_recent WHERE user_id = ?1
              ORDER BY played_at DESC LIMIT ?2)",
    )
    .bind(me)
    .bind(KEPT)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes `kind` `item` out of the caller's Jump back in until they start it again. An album
/// that was never recorded (Home found it in the play history) is remembered as removed too.
async fn remove(
    State(state): State<RecentState>,
    Extension(current): Extension<CurrentUser>,
    Path((kind, item)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let kind = Kind::parse(&kind)?;
    if item.is_empty() || item.len() > 200 {
        return Err(bad_request("the id is 1 to 200 characters"));
    }
    if kind == Kind::Album {
        if item.parse::<i64>().is_err() {
            return Err(bad_request("an album's id is a number"));
        }
        // Kept as the latest, so the oldest entries go before it does.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO music_recent (user_id, kind, item, played_at, removed)
             VALUES (?1, 'album', ?3, {LATER}, 1)
             ON CONFLICT (user_id, kind, item) DO UPDATE SET removed = 1"
        )))
        .bind(current.user.id)
        .bind("album")
        .bind(&item)
        .bind(now_ms())
        .execute(&state.pool)
        .await?;
        return Ok(StatusCode::NO_CONTENT);
    }
    let changed = sqlx::query(
        "UPDATE music_recent SET removed = 1 WHERE user_id = ? AND kind = ? AND item = ?",
    )
    .bind(current.user.id)
    .bind(kind.as_str())
    .bind(&item)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not in your Jump back in",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
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
        let user = crate::auth::testing::user(pool, name, false).await;
        let token = auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        (user.id, token)
    }

    async fn call(app: &Router, token: &str, method: &str, uri: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
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

    /// What `token`'s Jump back in lists, as "kind:id".
    async fn listed(app: &Router, token: &str) -> (Vec<String>, Value) {
        let (status, body) = call(app, token, "GET", "/api/v1/music/recent").await;
        assert_eq!(status, StatusCode::OK);
        let items = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                format!(
                    "{}:{}",
                    e["kind"].as_str().unwrap(),
                    e["id"].as_str().unwrap()
                )
            })
            .collect();
        (items, body["removed_albums"].clone())
    }

    #[tokio::test]
    async fn each_listener_keeps_their_own_recent_albums_playlists_and_stations() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m'),
                 (2, 'more', 'music', '/n');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation) VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1),
                 (2, 2, 'k2', 'Two', 'Birch', 'b', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres,
                 has_picture) VALUES (7, 1, 'a/1.flac', 1, 1, 1, 1, 1, 1, 'Song', '[]', 0),
                 (8, 2, 'b/1.flac', 1, 1, 1, 1, 2, 1, 'Other', '[]', 0);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, alice_token) = user(&pool, "alice").await;
        let (bob, bob_token) = user(&pool, "bob").await;
        let op = r#"{"opId":"p","entity":"playlists","entityId":"pl-1","operation":"upsert","payload":{"id":"pl-1","name":"Mix","updatedAt":1},"createdAt":1}"#;
        let op = sync::validate(&RawValue::from_string(op.into()).unwrap()).unwrap();
        sync::push(&pool, alice, "phone", vec![op], 1)
            .await
            .unwrap();
        let app = routes(pool.clone(), false);

        for path in ["album/1", "playlist/pl-1", "station/7"] {
            let uri = format!("/api/v1/music/recent/{path}");
            assert_eq!(
                call(&app, &alice_token, "PUT", &uri).await.0,
                StatusCode::NO_CONTENT
            );
        }
        let (items, removed) = listed(&app, &alice_token).await;
        assert_eq!(items.len(), 3);
        let mut sorted = items.clone();
        sorted.sort();
        assert_eq!(sorted, ["album:1", "playlist:pl-1", "station:7"]);
        assert_eq!(removed, json!([]));
        let (_, body) = call(&app, &alice_token, "GET", "/api/v1/music/recent").await;
        let entry = |kind: &str| {
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["kind"] == kind)
                .unwrap()
                .clone()
        };
        assert_eq!(entry("album")["album"]["title"], "One");
        assert_eq!(entry("playlist")["playlist"]["name"], "Mix");
        assert_eq!(entry("station")["track"]["title"], "Song");

        // Bob has none of Alice's; her playlist isn't his to record; nothing unknown is.
        assert_eq!(listed(&app, &bob_token).await.0, Vec::<String>::new());
        for (path, want) in [
            ("playlist/pl-1", StatusCode::NOT_FOUND),
            ("album/99", StatusCode::NOT_FOUND),
            ("album/x", StatusCode::NOT_FOUND),
            ("song/7", StatusCode::BAD_REQUEST),
        ] {
            let uri = format!("/api/v1/music/recent/{path}");
            assert_eq!(call(&app, &bob_token, "PUT", &uri).await.0, want, "{path}");
        }
        assert_eq!(
            call(&app, &bob_token, "DELETE", "/api/v1/music/recent/station/7")
                .await
                .0,
            StatusCode::NOT_FOUND,
            "not in his"
        );

        // Removing hides one until it's played again; an album removed stays out of the
        // history's albums too, even one never recorded.
        let remove = "/api/v1/music/recent/station/7";
        assert_eq!(
            call(&app, &alice_token, "DELETE", remove).await.0,
            StatusCode::NO_CONTENT
        );
        let away = "/api/v1/music/recent/album/2";
        assert_eq!(
            call(&app, &alice_token, "DELETE", away).await.0,
            StatusCode::NO_CONTENT
        );
        let (items, removed) = listed(&app, &alice_token).await;
        assert!(!items.contains(&"station:7".to_owned()));
        assert_eq!(removed, json!([2]));
        let again = "/api/v1/music/recent/station/7";
        assert_eq!(
            call(&app, &alice_token, "PUT", again).await.0,
            StatusCode::NO_CONTENT
        );
        let (items, _) = listed(&app, &alice_token).await;
        assert_eq!(items[0], "station:7", "back, and the latest");

        // Someone who may see only the second library sees nothing of the first: Alice, now
        // limited, loses the album and the station from the first, and playlists close.
        sqlx::query(
            "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             VALUES (?, 0, NULL, 0)",
        )
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, 2)")
            .bind(alice)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(listed(&app, &alice_token).await.0, Vec::<String>::new());
        let theirs = "/api/v1/music/recent/station/8";
        assert_eq!(
            call(&app, &alice_token, "PUT", theirs).await.0,
            StatusCode::NO_CONTENT
        );
        let first = "/api/v1/music/recent/album/1";
        assert_eq!(
            call(&app, &alice_token, "PUT", first).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(listed(&app, &alice_token).await.0, ["station:8"]);
        let _ = bob;
    }

    #[tokio::test]
    async fn only_the_latest_fifty_are_kept() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');",
        )
        .execute(&pool)
        .await
        .unwrap();
        for id in 1..=55 {
            sqlx::query(
                "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                     seen_generation) VALUES (?, 1, ?, 'A', 'B', ?, 1)",
            )
            .bind(id)
            .bind(format!("k{id}"))
            .bind(format!("f{id}"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let (alice, token) = user(&pool, "alice").await;
        let app = routes(pool.clone(), false);
        for id in 1..=55 {
            let uri = format!("/api/v1/music/recent/album/{id}");
            assert_eq!(
                call(&app, &token, "PUT", &uri).await.0,
                StatusCode::NO_CONTENT
            );
        }
        let kept: Vec<String> =
            sqlx::query_scalar("SELECT item FROM music_recent WHERE user_id = ?")
                .bind(alice)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(kept.len(), 50);
        assert!(!kept.contains(&"1".to_owned()), "the oldest went first");
        assert_eq!(listed(&app, &token).await.0.len(), 20, "twenty are shown");
    }
}
