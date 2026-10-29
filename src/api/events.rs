//! Live updates for every app: one event stream per device, so an app learns
//! of a change as it happens instead of polling for it. Server-Sent Events, looked at every 2
//! seconds, each kind sent at the start and then whenever it changes:
//!
//! - `sync`: the newest change in the Aoide sync log the device would pull, `{"cursor": n}`;
//!   past its own cursor, it pulls now. Its own pushes aren't counted when it says which
//!   device it is (`?device=`), so a push isn't echoed back to the device that made it.
//! - `requests`: the user's newest requests with how each is getting on.
//! - `library`: how many movies, episodes, albums and tracks there are, with a version that
//!   changes whenever one is added or goes; Discover may then have changed too.
//! - `scan`: the scans running now in libraries the user may see, each with the folders it has
//!   read (written down once a second); sent once one runs, and as `[]` once none does.
//! - `favourites`: the version of the user's favourite movies and series, raised by each
//!   change, and how many there are, `{"version", "total"}`; sent once they've had any.
//! - `pins`: the user's pinned albums, artists and playlists, `{"version", "items"}`;
//!   sent once they've pinned anything.
//! - `household`: what everyone else who shares their listening is playing and played lately,
//!   as `GET /api/v1/music/household` lists `people`; sent once anyone else has chosen.
//! - `listening`: the listening-together sessions the device is in, each time one
//!   changes; sent once the device is in one, and as `[]` once it's in none again.
//!
//! Only what the way in opens is sent: no `sync` without music, and so on. The
//! stream ends when its session signs out and after 10 minutes; clients reconnect, and work
//! by polling when it's down.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router, middleware};
use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use crate::access::Access;
use crate::ways_in::{Opens, Part};

/// How many of a user's newest requests are followed.
const REQUESTS: i64 = 50;

/// The event stream, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/events", get(events))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

#[derive(Deserialize)]
struct EventsQuery {
    /// The Aoide device id the app syncs as, so its own pushes aren't announced to it.
    device: Option<String>,
}

/// The newest sync-log change `user_id` would pull, leaving out what `device` pushed.
pub async fn sync_cursor(
    pool: &SqlitePool,
    user_id: i64,
    device: Option<&str>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT coalesce(max(seq), 0) FROM aoide_ops
         WHERE NOT (user_id = ?1 AND device_id IS ?2) AND (
                 user_id = ?1
              OR playlist_id IN (SELECT playlist_id FROM aoide_playlist_owners WHERE owner_user_id = ?1)
              OR playlist_id IN (SELECT playlist_id FROM aoide_playlist_shares WHERE grantee_user_id = ?1))",
    )
    .bind(user_id)
    .bind(device)
    .fetch_one(pool)
    .await
}

/// What's in the library, as counts and a version that changes when anything comes or goes;
/// video and music only as the way in opens them.
#[cfg(test)]
pub async fn library(
    pool: &SqlitePool,
    video: bool,
    music: bool,
) -> sqlx::Result<serde_json::Value> {
    library_for(pool, &Access::everything(), video, music).await
}

/// [`library`], counting only what a user may see.
pub async fn library_for(
    pool: &SqlitePool,
    access: &Access,
    video: bool,
    music: bool,
) -> sqlx::Result<serde_json::Value> {
    let (movies, movie_max, episodes, episode_max, albums, album_max, tracks, track_max): (
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT
             (SELECT count(*) FROM movies x WHERE x.missing_since IS NULL AND {movie}),
             (SELECT coalesce(max(x.id), 0) FROM movies x WHERE {movie}),
             (SELECT count(*) FROM media_files x WHERE x.series_id IS NOT NULL
                 AND x.missing_since IS NULL AND {file}),
             (SELECT coalesce(max(x.id), 0) FROM media_files x WHERE x.series_id IS NOT NULL
                 AND {file}),
             (SELECT count(*) FROM music_albums x WHERE x.missing_since IS NULL AND {album}),
             (SELECT coalesce(max(x.id), 0) FROM music_albums x WHERE {album}),
             (SELECT count(*) FROM music_tracks x WHERE x.missing_since IS NULL AND {track}),
             (SELECT coalesce(max(x.id), 0) FROM music_tracks x WHERE {track})",
        movie = access.movie_sql("x"),
        file = access.file_sql("x"),
        album = access.library_sql("x.library_id"),
        track = access.library_sql("x.library_id"),
    )))
    .fetch_one(pool)
    .await?;
    let mut body = serde_json::Map::new();
    let mut version = Vec::new();
    if video {
        body.insert("movies".into(), json!(movies));
        body.insert("episodes".into(), json!(episodes));
        version.extend([movies, movie_max, episodes, episode_max]);
    }
    if music {
        body.insert("albums".into(), json!(albums));
        body.insert("tracks".into(), json!(tracks));
        version.extend([albums, album_max, tracks, track_max]);
    }
    let version: Vec<String> = version.iter().map(i64::to_string).collect();
    body.insert("version".into(), json!(version.join(".")));
    Ok(serde_json::Value::Object(body))
}

/// The scans running now in libraries the user may see, once one has run since the stream
/// opened (`scanned`), so a stream opened with none running sends nothing; `[]` after.
async fn scans(
    pool: &SqlitePool,
    access: &Access,
    (video, music): (bool, bool),
    scanned: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<Option<String>> {
    use std::sync::atomic::Ordering;
    let library_sql = access.library_sql("p.library_id");
    let running = crate::library::progress::running(pool, &library_sql, video, music).await?;
    if !running.is_empty() {
        scanned.store(true, Ordering::Relaxed);
    }
    Ok(match scanned.load(Ordering::Relaxed) {
        true => Some(serde_json::to_string(&running)?),
        false => None,
    })
}

async fn events(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    opens: Option<Extension<Opens>>,
    Query(query): Query<EventsQuery>,
) -> Result<Response, ApiError> {
    let device = query.device.filter(|d| !d.is_empty());
    if device.as_ref().is_some_and(|d| d.chars().count() > 256) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "device is at most 256 characters",
        ));
    }
    let opens = opens.map_or(Opens(None), |Extension(o)| o);
    let (music, video, requests) = (
        opens.has(Part::Music),
        opens.has(Part::Video),
        opens.has(Part::Requests),
    );
    let user = current.user.id;
    let token = current.token_id;
    let access = current.access.clone();
    // Aoide's sync and what listeners share can't be narrowed to some music libraries.
    let shared_music = music && current.access.sees_all_music(&pool).await?;
    // Whether the device has been in a session, and whether a scan has run, since the stream
    // opened.
    let listened = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let scanned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stream = super::progress::stream_of(pool, &current, false, move |pool| {
        let device = device.clone();
        let access = access.clone();
        let listened = listened.clone();
        let scanned = scanned.clone();
        async move {
            let mut kinds = Vec::new();
            if shared_music {
                let cursor = sync_cursor(&pool, user, device.as_deref()).await?;
                kinds.push(("sync", json!({ "cursor": cursor }).to_string()));
            }
            if requests {
                let (_, items) =
                    crate::requests::list(&pool, Some(user), None, REQUESTS, 0).await?;
                kinds.push(("requests", serde_json::to_string(&items)?));
            }
            if music || video {
                let counts = library_for(&pool, &access, video, music).await?;
                kinds.push(("library", counts.to_string()));
            }
            if (music || video)
                && let Some(scans) = scans(&pool, &access, (video, music), &scanned).await?
            {
                kinds.push(("scan", scans));
            }
            if video && let Some(favourites) = super::favourites::event(&pool, user).await? {
                kinds.push(("favourites", favourites));
            }
            if music && let Some(pins) = super::music_pins::event(&pool, user).await? {
                kinds.push(("pins", pins));
            }
            if shared_music
                && let Some(people) = super::music_household::snapshot(&pool, user).await?
            {
                kinds.push(("household", people.to_string()));
            }
            if shared_music {
                let sessions = crate::listen::for_device(&pool, token).await?;
                use std::sync::atomic::Ordering;
                if !sessions.is_empty() {
                    listened.store(true, Ordering::Relaxed);
                }
                if listened.load(Ordering::Relaxed) {
                    kinds.push(("listening", serde_json::to_string(&sessions)?));
                }
            }
            Ok(kinds)
        }
    });
    Ok(stream.into_response())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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

    async fn push(pool: &SqlitePool, user: i64, device: &str, op_id: &str, playlist: Option<&str>) {
        let op = match playlist {
            Some(p) => format!(
                r#"{{"opId":"{op_id}","entity":"playlist_items","entityId":"{op_id}","operation":"upsert","payload":{{"id":"{op_id}","playlistId":"{p}"}},"createdAt":1}}"#
            ),
            None => format!(
                r#"{{"opId":"{op_id}","entity":"likes","entityId":"{op_id}","operation":"upsert","payload":{{"id":"{op_id}"}},"createdAt":1}}"#
            ),
        };
        let op = sync::validate(&RawValue::from_string(op).unwrap()).unwrap();
        sync::push(pool, user, device, vec![op], 1).await.unwrap();
    }

    #[tokio::test]
    async fn a_devices_own_pushes_arent_announced_to_it() {
        let pool = crate::db::open_in_memory().await;
        let ((alice, _), (bob, _)) = (user(&pool, "alice").await, user(&pool, "bob").await);
        push(&pool, alice, "phone", "a1", None).await; // seq 1
        push(&pool, alice, "desk", "a2", None).await; // seq 2
        push(&pool, alice, "phone", "a3", None).await; // seq 3
        assert_eq!(sync_cursor(&pool, alice, None).await.unwrap(), 3);
        assert_eq!(sync_cursor(&pool, alice, Some("phone")).await.unwrap(), 2);
        assert_eq!(sync_cursor(&pool, alice, Some("desk")).await.unwrap(), 3);
        // Bob's own log is his; a playlist shared with him brings its changes.
        push(&pool, alice, "phone", "own", Some("p")).await; // seq 4
        assert_eq!(sync_cursor(&pool, bob, None).await.unwrap(), 0);
        crate::aoide::library::share(&pool, "p", bob, true, 1)
            .await
            .unwrap();
        assert_eq!(sync_cursor(&pool, bob, None).await.unwrap(), 4);
    }

    #[tokio::test]
    async fn the_library_version_changes_when_anything_comes_or_goes() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres)
                 VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'A1', '[]'), (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'A2', '[]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let before = library(&pool, true, true).await.unwrap();
        assert_eq!(
            (&before["albums"], &before["tracks"], &before["movies"]),
            (&json!(1), &json!(2), &json!(0))
        );
        // One goes and another comes: the counts stay, the version doesn't.
        sqlx::raw_sql(
            "UPDATE music_tracks SET missing_since = 1 WHERE id = 2;
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres)
                 VALUES (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'A3', '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let after = library(&pool, true, true).await.unwrap();
        assert_eq!(after["tracks"], before["tracks"]);
        assert_ne!(after["version"], before["version"]);
        let music_only = library(&pool, false, true).await.unwrap();
        assert!(music_only.get("movies").is_none() && music_only.get("tracks").is_some());
    }

    /// The kinds of event a stream sends at once, and its body to read on from.
    async fn open(
        app: &Router,
        token: &str,
        uri: &str,
        opens: Option<Opens>,
    ) -> (Vec<String>, Body) {
        let mut request = Request::get(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        if let Some(opens) = opens {
            request.extensions_mut().insert(opens);
        }
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let mut kinds = Vec::new();
        // Everything is sent at the start; then nothing until something changes.
        while let Ok(Some(Ok(frame))) =
            tokio::time::timeout(Duration::from_millis(500), body.frame()).await
        {
            if let Ok(data) = frame.into_data() {
                kinds.push(String::from_utf8(data.to_vec()).unwrap());
            }
        }
        (kinds, body)
    }

    fn kind(event: &str) -> &str {
        event
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("event: ")
    }

    #[tokio::test]
    async fn pins_are_sent_once_there_are_any_and_when_they_change() {
        let pool = crate::db::open_in_memory().await;
        let (alice, token) = user(&pool, "alice").await;
        sqlx::query(
            "INSERT INTO music_pin_lists (user_id, version, updated_at) VALUES (?, 1, 0);
             INSERT INTO music_pins (user_id, position, kind, item) VALUES (?, 0, 'album', '7')",
        )
        .bind(alice)
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone());
        let (first, mut body) = open(&app, &token, "/api/v1/events", None).await;
        let pins = first.iter().find(|e| kind(e) == "pins").expect("pins sent");
        assert!(pins.contains(r#"{"id":"7","kind":"album"}"#), "{pins}");

        sqlx::query("UPDATE music_pin_lists SET version = 2; DELETE FROM music_pins")
            .execute(&pool)
            .await
            .unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("the change was sent")
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        let next = String::from_utf8(next.to_vec()).unwrap();
        assert_eq!(kind(&next), "pins");
        assert!(next.contains(r#"{"items":[],"version":2}"#), "{next}");
        // Not without music.
        let (video, _) = open(
            &app,
            &token,
            "/api/v1/events",
            Some(Opens(Some(vec![Part::Video]))),
        )
        .await;
        assert!(video.iter().all(|e| kind(e) != "pins"));
    }

    #[tokio::test]
    async fn favourites_are_sent_once_there_have_been_any_and_only_with_video() {
        let pool = crate::db::open_in_memory().await;
        let (alice, token) = user(&pool, "alice").await;
        let app = routes(pool.clone());
        let (first, _) = open(&app, &token, "/api/v1/events", None).await;
        assert!(first.iter().all(|e| kind(e) != "favourites"));
        sqlx::query("INSERT INTO favourite_versions (user_id, version) VALUES (?, 4)")
            .bind(alice)
            .execute(&pool)
            .await
            .unwrap();
        let (sent, _) = open(&app, &token, "/api/v1/events", None).await;
        let favourites = sent.iter().find(|e| kind(e) == "favourites").unwrap();
        assert!(
            favourites.contains(r#"{"total":0,"version":4}"#),
            "{favourites}"
        );
        let music = Some(Opens(Some(vec![Part::Music])));
        let (music, _) = open(&app, &token, "/api/v1/events", music).await;
        assert!(music.iter().all(|e| kind(e) != "favourites"));
    }

    #[tokio::test]
    async fn the_household_is_sent_once_anyone_else_has_chosen_to_share() {
        let pool = crate::db::open_in_memory().await;
        let (_, token) = user(&pool, "alice").await;
        let (bob, _) = user(&pool, "bob").await;
        let app = routes(pool.clone());
        let (first, _) = open(&app, &token, "/api/v1/events", None).await;
        assert!(
            first.iter().all(|e| kind(e) != "household"),
            "nobody shares"
        );

        sqlx::query("INSERT INTO music_listening_sharing VALUES (?, 1, 0)")
            .bind(bob)
            .execute(&pool)
            .await
            .unwrap();
        let (first, mut body) = open(&app, &token, "/api/v1/events", None).await;
        let household = first
            .iter()
            .find(|e| kind(e) == "household")
            .expect("household sent");
        assert!(household.contains(r#""user":"bob""#), "{household}");

        sqlx::query("UPDATE music_listening_sharing SET sharing = 0")
            .execute(&pool)
            .await
            .unwrap();
        let next = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("the change was sent")
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        let next = String::from_utf8(next.to_vec()).unwrap();
        assert_eq!(kind(&next), "household");
        assert!(next.contains("data: []"), "{next}");
    }

    #[tokio::test]
    async fn one_stream_tells_a_device_what_changed() {
        let pool = crate::db::open_in_memory().await;
        let (alice, token) = user(&pool, "alice").await;
        let app = routes(pool.clone());
        let (first, mut body) = open(&app, &token, "/api/v1/events?device=phone", None).await;
        let kinds: Vec<&str> = first.iter().map(|e| kind(e)).collect();
        assert_eq!(kinds, ["sync", "requests", "library"]);
        assert!(first[0].contains(r#"data: {"cursor":0}"#), "{}", first[0]);

        // Its own push isn't news to it; another device's is.
        push(&pool, alice, "phone", "mine", None).await;
        push(&pool, alice, "desk", "theirs", None).await;
        let next = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("the change was sent")
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        let next = String::from_utf8(next.to_vec()).unwrap();
        assert_eq!(kind(&next), "sync");
        assert!(next.contains(r#"{"cursor":2}"#), "{next}");

        // Through a way that opens only music, requests aren't sent, nor video counts.
        let (music, _) = open(
            &app,
            &token,
            "/api/v1/events",
            Some(Opens(Some(vec![Part::Music]))),
        )
        .await;
        let kinds: Vec<&str> = music.iter().map(|e| kind(e)).collect();
        assert_eq!(kinds, ["sync", "library"]);
        assert!(!music[1].contains("movies"));
        let (none, _) = open(&app, &token, "/api/v1/events", Some(Opens(Some(vec![])))).await;
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn a_scan_is_sent_while_it_runs_then_as_none() {
        let pool = crate::db::open_in_memory().await;
        let (_, token) = user(&pool, "alice").await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone());
        let (first, mut body) = open(&app, &token, "/api/v1/events", None).await;
        assert!(first.iter().all(|e| kind(e) != "scan"), "none running");

        // Progress is this database's own, so another test's scan never shows here.
        sqlx::query(
            "INSERT INTO scan_progress (library_id, whole, folders_done, started_at, updated_at)
             VALUES (1, 1, 12, 5, 5)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let running = next_event(&mut body).await;
        assert_eq!(kind(&running), "scan");
        assert!(
            running.contains(
                r#"[{"library":"movies","kind":"movies","whole":true,"folders_done":12,"started_at":5}]"#
            ),
            "{running}"
        );
        sqlx::query("DELETE FROM scan_progress")
            .execute(&pool)
            .await
            .unwrap();
        let done = next_event(&mut body).await;
        assert_eq!(
            (kind(&done), done.lines().nth(1)),
            ("scan", Some("data: []"))
        );
        // Not through a way in that opens neither video nor music.
        sqlx::query(
            "INSERT INTO scan_progress (library_id, whole, folders_done, started_at, updated_at)
             VALUES (1, 1, 0, 5, 5)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let requests = Some(Opens(Some(vec![Part::Requests])));
        let (only_requests, _) = open(&app, &token, "/api/v1/events", requests).await;
        assert!(only_requests.iter().all(|e| kind(e) != "scan"));
    }

    async fn next_event(body: &mut Body) -> String {
        let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
            .await
            .expect("the change was sent")
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        String::from_utf8(frame.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_device_hears_of_its_listening_sessions() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation) VALUES (1, 1, 'k', 'A', 'B', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres)
                 VALUES (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'One', '[]');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, token) = user(&pool, "alice").await;
        let token_id: i64 = sqlx::query_scalar("SELECT id FROM tokens WHERE user_id = ?")
            .bind(alice)
            .fetch_one(&pool)
            .await
            .unwrap();
        let app = routes(pool.clone());
        let (first, mut body) = open(&app, &token, "/api/v1/events", None).await;
        assert!(
            first.iter().all(|e| kind(e) != "listening"),
            "nothing until it's in a session"
        );
        let state = crate::listen::State {
            queue: vec![1],
            index: 0,
            position_ms: 0,
            playing: true,
        };
        let id = crate::listen::start(&pool, alice, token_id, &state)
            .await
            .unwrap()
            .unwrap();
        let started = next_event(&mut body).await;
        assert_eq!(kind(&started), "listening");
        assert!(started.contains(r#""queue":[1]"#), "{started}");
        crate::listen::leave(&pool, token_id, id).await.unwrap();
        let left = next_event(&mut body).await;
        assert_eq!(
            (kind(&left), left.lines().nth(1)),
            ("listening", Some("data: []"))
        );
    }
}
