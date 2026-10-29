//! Play history for the apps: each user's own listens, newest first, and a listen sent
//! by an app that doesn't sync (the web UI's player, say). Aoide's devices needn't send
//! anything here: their play events reach the history through sync. Each user can delete their
//! own listens, one or a time range at a time.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_playback, require_user};
use super::music::{bad_request, paging, tracks_by_id};
use crate::db::now_ms;
use crate::plays::{self, Forget, Listen, Source};

/// The longest id an app may give a listen.
const MAX_ID: usize = 128;
/// How far ahead of the server's clock a listen may say it started.
const CLOCK_SLACK_MS: i64 = 86_400_000;

/// The play history routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    // Keeping a listen is playing's to do; deleting them is for a full sign-in.
    let keeping = Router::new()
        .route("/api/v1/music/plays", axum::routing::post(add))
        .route_layer(middleware::from_fn_with_state(
            auth.clone(),
            require_playback,
        ));
    Router::new()
        .route("/api/v1/music/plays", get(list).delete(clear))
        .route("/api/v1/music/plays/{id}", axum::routing::delete(remove))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .merge(keeping)
        .with_state(pool)
}

#[derive(sqlx::FromRow)]
struct PlayRow {
    id: i64,
    source: String,
    track_id: Option<i64>,
    device: String,
    started_at: i64,
    ended_at: Option<i64>,
    position_ms: Option<i64>,
    completed: bool,
    skipped: bool,
    activity: Option<String>,
}

const PLAY_COLUMNS: &str = "id, source, track_id, device, started_at, ended_at, position_ms,
    completed, skipped, activity";

/// Plays as the API shows them: times in Unix seconds, each with its track as the music API
/// shows it, or null when it isn't in the library now.
async fn to_json(
    pool: &SqlitePool,
    access: &crate::access::Access,
    me: i64,
    rows: &[PlayRow],
) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<i64> = rows.iter().filter_map(|r| r.track_id).collect();
    let tracks = tracks_by_id(pool, access, me, &ids).await?;
    Ok(rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "track_id": r.track_id,
                "track": r.track_id.and_then(|t| tracks.get(&t)).cloned(),
                "device": r.device,
                "started_at": r.started_at.div_euclid(1000),
                "ended_at": r.ended_at.map(|t| t.div_euclid(1000)),
                "position_seconds": r.position_ms.map(|p| p as f64 / 1000.0),
                "completed": r.completed,
                "skipped": r.skipped,
                "activity": r.activity,
                "source": r.source,
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct ListQuery {
    /// Only listens started at or after this, in Unix seconds.
    since: Option<i64>,
    /// Only listens with this activity tag (case-insensitive).
    activity: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// The caller's listens, newest first.
async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let (limit, offset) = paging(query.limit, query.offset)?;
    plays::catch_up(&pool).await?;
    let since = query.since.map(|s| s.saturating_mul(1000));
    let activity = query.activity.filter(|a| !a.trim().is_empty());
    let filter = "FROM music_plays WHERE user_id = ?1 AND (?2 IS NULL OR started_at >= ?2)
         AND (?3 IS NULL OR activity = trim(?3) COLLATE NOCASE)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {filter}")))
        .bind(current.user.id)
        .bind(since)
        .bind(&activity)
        .fetch_one(&pool)
        .await?;
    let rows: Vec<PlayRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PLAY_COLUMNS} {filter} ORDER BY started_at DESC, id DESC LIMIT ?4 OFFSET ?5"
    )))
    .bind(current.user.id)
    .bind(since)
    .bind(&activity)
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await?;
    Ok(Json(
        json!({ "total": total, "items": to_json(&pool, &current.access, current.user.id, &rows).await? }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewPlay {
    /// The app's own id for the listen: sent again, the listen is changed, not added twice.
    id: Option<String>,
    track_id: i64,
    /// Unix seconds.
    started_at: i64,
    ended_at: Option<i64>,
    /// How far into the track it ended.
    position_seconds: Option<f64>,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    skipped: bool,
    activity: Option<String>,
    /// The device's name; the session's device name when left out.
    device: Option<String>,
}

/// Keeps a listen from an app that doesn't sync.
async fn add(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<NewPlay>,
) -> Result<Json<Value>, ApiError> {
    let started_at = body.started_at.saturating_mul(1000);
    let ended_at = body.ended_at.map(|t| t.saturating_mul(1000));
    if started_at <= 0 || started_at > now_ms() + CLOCK_SLACK_MS {
        return Err(bad_request(
            "started_at must be a time in Unix seconds, not in the future",
        ));
    }
    if ended_at.is_some_and(|end| end < started_at) {
        return Err(bad_request("ended_at can't be before started_at"));
    }
    let position_ms = match body.position_seconds {
        Some(p) if p.is_finite() && p >= 0.0 => Some((p * 1000.0).round() as i64),
        Some(_) => return Err(bad_request("position_seconds must be 0 or more")),
        None => None,
    };
    if body.completed && body.skipped {
        return Err(bad_request("a listen is completed or skipped, not both"));
    }
    let event_id = match body.id.map(|id| id.trim().to_owned()) {
        Some(id) if !id.is_empty() && id.chars().count() <= MAX_ID => id,
        Some(_) => {
            return Err(bad_request(&format!("id must be 1 to {MAX_ID} characters")));
        }
        None => crate::auth::random_token("server-")?,
    };
    if plays::forgotten(&pool, current.user.id, Source::Api, &event_id).await? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "you deleted this listen from your history, so it isn't kept again",
        ));
    }
    let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM music_tracks WHERE id = ?)")
        .bind(body.track_id)
        .fetch_one(&pool)
        .await?;
    if !known {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    let device = match body.device.map(|d| d.trim().to_owned()) {
        Some(d) if !d.is_empty() => d.chars().take(MAX_ID).collect(),
        _ => sqlx::query_scalar("SELECT name FROM tokens WHERE id = ?")
            .bind(current.token_id)
            .fetch_optional(&pool)
            .await?
            .unwrap_or_else(|| "unknown device".to_owned()),
    };
    let listen = Listen {
        event_id,
        track_key: body.track_id.to_string(),
        device,
        started_at,
        ended_at,
        position_ms,
        completed: body.completed,
        skipped: body.skipped,
        activity: plays::activity(body.activity.as_deref()),
    };
    let id = plays::record(&pool, current.user.id, Source::Api, &listen).await?;
    let row: PlayRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PLAY_COLUMNS} FROM music_plays WHERE id = ?"
    )))
    .bind(id)
    .fetch_one(&pool)
    .await?;
    let mut shown = to_json(
        &pool,
        &current.access,
        current.user.id,
        std::slice::from_ref(&row),
    )
    .await?;
    Ok(Json(shown.remove(0)))
}

#[derive(Deserialize)]
struct ClearQuery {
    /// Unix seconds: listens started at or after this.
    from: Option<i64>,
    /// Unix seconds: listens started before this.
    to: Option<i64>,
    /// Every listen.
    #[serde(default)]
    all: bool,
}

/// Deletes the caller's listens started in a time range, or all of them.
async fn clear(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ClearQuery>,
) -> Result<Json<Value>, ApiError> {
    let ranged = query.from.is_some() || query.to.is_some();
    if query.all && ranged {
        return Err(bad_request(
            "all deletes every listen; leave out from and to",
        ));
    }
    if !query.all && !ranged {
        return Err(bad_request(
            "say which listens: from, to or both (Unix seconds), or all=true",
        ));
    }
    if let (Some(from), Some(to)) = (query.from, query.to)
        && to <= from
    {
        return Err(bad_request("to must be after from"));
    }
    let ms = |t: Option<i64>| t.map(|t| t.saturating_mul(1000));
    let deleted = plays::forget(
        &pool,
        current.user.id,
        Forget::Between(ms(query.from), ms(query.to)),
    )
    .await?;
    Ok(Json(json!({ "deleted": deleted })))
}

/// Deletes one of the caller's listens.
async fn remove(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    match plays::forget(&pool, current.user.id, Forget::One(id)).await? {
        0 => Err(ApiError::new(StatusCode::NOT_FOUND, "no such listen")),
        _ => Ok(StatusCode::NO_CONTENT),
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn user(pool: &SqlitePool, name: &str, device: &str) -> (i64, String) {
        let user = auth::create_user(pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(pool, user.id, TokenKind::Session, device)
            .await
            .unwrap()
            .1;
        (user.id, token)
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

    async fn library(pool: &SqlitePool) {
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'Album', 'Alice', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres)
                 VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'First', 'Alice', '[]'),
                        (8, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Second', 'Alice', '[]');"#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn keeps_each_users_listens_with_how_they_ended() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let (alice, alice_token) = user(&pool, "alice", "Living room iPad").await;
        let (_, bob_token) = user(&pool, "bob", "Phone").await;
        // Alice's phone synced one listen through Aoide.
        sqlx::query(
            "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                 payload, created_at, received_at)
             VALUES ('o1', ?, 'phone', 'play_events', 'e1', 'upsert',
                 '{\"jellyfinId\":\"8\",\"startedAt\":3000000,\"completed\":true,\"activity\":\"Focus\"}',
                 1, 1)",
        )
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone());

        let (status, play) = call(
            &app,
            &alice_token,
            "POST",
            "/api/v1/music/plays",
            Some(json!({ "id": "web-1", "track_id": 7, "started_at": 4000,
                         "position_seconds": 31.5, "skipped": true })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{play}");
        assert_eq!(
            (
                &play["track"]["title"],
                &play["device"],
                &play["position_seconds"],
                &play["skipped"],
                &play["source"]
            ),
            (
                &json!("First"),
                &json!("Living room iPad"),
                &json!(31.5),
                &json!(true),
                &json!("api")
            )
        );
        // Sent again, finished after all: the same listen.
        let (_, again) = call(
            &app,
            &alice_token,
            "POST",
            "/api/v1/music/plays",
            Some(
                json!({ "id": "web-1", "track_id": 7, "started_at": 4000, "ended_at": 4200,
                         "completed": true, "activity": "focus" }),
            ),
        )
        .await;
        assert_eq!(again["id"], play["id"]);

        let (status, page) = call(&app, &alice_token, "GET", "/api/v1/music/plays", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["total"], 2);
        let items: Vec<(&Value, &Value, &Value, &Value)> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    &p["track_id"],
                    &p["started_at"],
                    &p["completed"],
                    &p["source"],
                )
            })
            .collect();
        assert_eq!(
            items,
            [
                (&json!(7), &json!(4000), &json!(true), &json!("api")),
                (&json!(8), &json!(3000), &json!(true), &json!("sync")),
            ],
            "newest first, from both sources"
        );
        let (_, focus) = call(
            &app,
            &alice_token,
            "GET",
            "/api/v1/music/plays?activity=FOCUS&since=3500",
            None,
        )
        .await;
        assert_eq!(focus["total"], 1);
        assert_eq!(focus["items"][0]["track_id"], 7);
        let (_, theirs) = call(&app, &bob_token, "GET", "/api/v1/music/plays", None).await;
        assert_eq!(theirs["total"], 0, "each user sees only their own");
    }

    #[tokio::test]
    async fn refuses_listens_that_cant_be() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let (_, token) = user(&pool, "alice", "Phone").await;
        let app = routes(pool);
        for (body, want) in [
            (
                json!({ "track_id": 99, "started_at": 10 }),
                StatusCode::NOT_FOUND,
            ),
            (
                json!({ "track_id": 7, "started_at": 0 }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "track_id": 7, "started_at": 4_000_000_000_i64 }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "track_id": 7, "started_at": 10, "ended_at": 5 }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "track_id": 7, "started_at": 10, "completed": true, "skipped": true }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "track_id": 7, "started_at": 10, "position_seconds": -1 }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "track_id": 7, "started_at": 10, "id": "" }),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let (status, answer) = call(
                &app,
                &token,
                "POST",
                "/api/v1/music/plays",
                Some(body.clone()),
            )
            .await;
            assert_eq!(status, want, "{body} → {answer}");
        }
        // Without an id, each listen is its own.
        for _ in 0..2 {
            let (status, _) = call(
                &app,
                &token,
                "POST",
                "/api/v1/music/plays",
                Some(json!({ "track_id": 7, "started_at": 10 })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let (_, page) = call(&app, &token, "GET", "/api/v1/music/plays", None).await;
        assert_eq!(page["total"], 2);
    }

    #[tokio::test]
    async fn deleting_listens_reaches_everything_that_reads_them() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let (alice, alice_token) = user(&pool, "alice", "Phone").await;
        let (bob, bob_token) = user(&pool, "bob", "Phone").await;
        sqlx::query(
            "INSERT INTO music_listening_sharing (user_id, sharing, changed_at) VALUES (?, 1, 0)",
        )
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone()).merge(super::super::music_home::routes(pool.clone(), false));
        let now = now_ms() / 1000;
        // Alice heard both tracks an hour ago, the first again ten minutes ago; Bob the first.
        for (token, id, track, at) in [
            (&alice_token, "a1", 7, now - 3600),
            (&alice_token, "a2", 8, now - 3500),
            (&alice_token, "a3", 7, now - 600),
            (&bob_token, "b1", 7, now - 600),
        ] {
            let (status, _) = call(
                &app,
                token,
                "POST",
                "/api/v1/music/plays",
                Some(json!({ "id": id, "track_id": track, "started_at": at,
                             "ended_at": at + 200, "completed": true })),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        let ids = |page: &Value| -> Vec<i64> {
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["id"].as_i64().unwrap())
                .collect()
        };
        let (_, mine) = call(&app, &alice_token, "GET", "/api/v1/music/plays", None).await;
        let (_, bobs) = call(&app, &bob_token, "GET", "/api/v1/music/plays", None).await;
        let (latest, bobs_id) = (ids(&mine)[0], ids(&bobs)[0]);

        // Only her own: Bob's listen isn't Alice's to delete.
        let uri = |id: i64| format!("/api/v1/music/plays/{id}");
        let (status, _) = call(&app, &alice_token, "DELETE", &uri(bobs_id), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(&app, &alice_token, "DELETE", &uri(latest), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = call(&app, &alice_token, "DELETE", &uri(latest), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "gone already");
        // Sent again by the app, it stays deleted.
        let (status, _) = call(
            &app,
            &alice_token,
            "POST",
            "/api/v1/music/plays",
            Some(json!({ "id": "a3", "track_id": 7, "started_at": now - 600 })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // What the household sees and Home follow.
        let snapshot = super::super::music_household::snapshot(&pool, bob)
            .await
            .unwrap()
            .unwrap();
        let heard: Vec<&Value> = snapshot[0]["recently_played"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| &r["track"]["id"])
            .collect();
        assert_eq!(heard, [&json!(8), &json!(7)]);
        let (_, home) = call(&app, &alice_token, "GET", "/api/v1/music/home", None).await;
        let most: Vec<(i64, i64)> = home["most_played"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["track"]["id"].as_i64().unwrap(),
                    m["plays"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(most, [(8, 1), (7, 1)]);

        // A range, then the rest; the range's ends must make sense.
        for bad in [
            "/api/v1/music/plays",
            "/api/v1/music/plays?all=true&from=1",
            "/api/v1/music/plays?from=10&to=10",
        ] {
            let (status, _) = call(&app, &alice_token, "DELETE", bad, None).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        }
        let range = format!("/api/v1/music/plays?from={}&to={}", now - 3600, now - 3500);
        let (status, done) = call(&app, &alice_token, "DELETE", &range, None).await;
        assert_eq!((status, &done), (StatusCode::OK, &json!({ "deleted": 1 })));
        // What Next, Discover and the mixes read.
        let history = crate::aoide::listening::play_events(&pool, alice)
            .await
            .unwrap();
        let tracks: Vec<&str> = history.iter().map(|p| p.track.as_str()).collect();
        assert_eq!(tracks, ["8"]);
        let (_, done) = call(
            &app,
            &alice_token,
            "DELETE",
            "/api/v1/music/plays?all=true",
            None,
        )
        .await;
        assert_eq!(done, json!({ "deleted": 1 }));
        let (_, home) = call(&app, &alice_token, "GET", "/api/v1/music/home", None).await;
        assert_eq!(home["recently_played"], json!([]));
        let snapshot = super::super::music_household::snapshot(&pool, bob)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot[0]["recently_played"], json!([]));
        let (_, bobs) = call(&app, &bob_token, "GET", "/api/v1/music/plays", None).await;
        assert_eq!(bobs["total"], 1, "Bob's are his own");
    }
}
