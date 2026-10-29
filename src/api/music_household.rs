//! What the household is listening to, for those who choose to share it: off unless a
//! user turns it on for themselves, and seen only by the household's own signed-in users. What
//! each sharer is playing now (an Aoide device's queue as it last pushed it, or a listen an app
//! has started and not ended) and what they played in the last week, from the play history.
//! Nothing leaves the server.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::track_values;
use crate::aoide::listening::normalise;
use crate::db::now_ms;

/// A track counts as still playing until this long after it would have ended.
const SLACK_MS: i64 = 60_000;
/// How far back "recently played" goes, and how many listens it shows.
const RECENT_MS: i64 = 7 * 86_400_000;
const RECENT: i64 = 10;

/// The household listening routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/household", get(household))
        .route(
            "/api/v1/music/household/sharing",
            get(sharing).put(set_sharing),
        )
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

async fn shares(pool: &SqlitePool, user_id: i64) -> sqlx::Result<bool> {
    let sharing: Option<bool> =
        sqlx::query_scalar("SELECT sharing FROM music_listening_sharing WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(sharing.unwrap_or(false))
}

/// Present tracks' lengths in milliseconds.
async fn lengths(pool: &SqlitePool, ids: &[i64]) -> sqlx::Result<HashMap<i64, i64>> {
    let rows: Vec<(i64, Option<f64>)> = sqlx::query_as(
        "SELECT id, duration_seconds FROM music_tracks
         WHERE missing_since IS NULL AND id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, seconds)| (id, (seconds.unwrap_or(0.0) * 1000.0) as i64))
        .collect())
}

/// What a user is playing now, if anything: (track, device, since in ms).
async fn now_playing(
    pool: &SqlitePool,
    user_id: i64,
    now: i64,
) -> sqlx::Result<Option<(i64, String, i64)>> {
    let mut best: Option<(i64, String, i64)> = None;
    // An Aoide device's queue, judged by when the server received it: playing while the track
    // at its position would still be going.
    let mut queued = Vec::new();
    for queue in crate::aoide::library::queues(pool, user_id).await? {
        let Ok(payload) = serde_json::from_str::<Value>(&queue.payload) else {
            continue;
        };
        let position = payload["position"].as_u64().unwrap_or(0) as usize;
        let Some(track) = payload["trackIds"]
            .as_array()
            .and_then(|ids| ids.get(position))
            .and_then(Value::as_str)
            .and_then(|id| normalise(id).parse::<i64>().ok())
        else {
            continue;
        };
        let elapsed = payload["elapsedMs"].as_i64().unwrap_or(0).max(0);
        let device = payload["deviceName"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&queue.device_id)
            .to_owned();
        queued.push((track, device, queue.received_at, elapsed));
    }
    let ids: Vec<i64> = queued.iter().map(|q| q.0).collect();
    let length = lengths(pool, &ids).await?;
    for (track, device, received, elapsed) in queued {
        let Some(ms) = length.get(&track) else {
            continue;
        };
        let started = received - elapsed;
        if started + ms + SLACK_MS > now && best.as_ref().is_none_or(|b| started > b.2) {
            best = Some((track, device, started));
        }
    }
    // A listen an app started and hasn't ended.
    let open: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT track_id, device, started_at FROM music_plays
         WHERE user_id = ? AND track_id IS NOT NULL AND ended_at IS NULL AND completed = 0
           AND skipped = 0
         ORDER BY started_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if let Some((track, device, started)) = open {
        let length = lengths(pool, &[track]).await?;
        if let Some(ms) = length.get(&track)
            && started + ms + SLACK_MS > now
            && best.as_ref().is_none_or(|b| started > b.2)
        {
            best = Some((track, device, started));
        }
    }
    Ok(best)
}

/// Everyone else who shares, by name: what they're playing and played lately. `None` when
/// nobody else has ever chosen to share, which the event stream leaves out.
pub async fn snapshot(pool: &SqlitePool, me: i64) -> anyhow::Result<Option<Value>> {
    let chosen: Vec<(i64, String, bool)> = sqlx::query_as(
        "SELECT u.id, u.username, s.sharing FROM music_listening_sharing s
         JOIN users u ON u.id = s.user_id WHERE u.id <> ? ORDER BY u.username COLLATE NOCASE",
    )
    .bind(me)
    .fetch_all(pool)
    .await?;
    if chosen.is_empty() {
        return Ok(None);
    }
    crate::plays::catch_up(pool).await?;
    let now = now_ms();
    let mut people = Vec::new();
    let mut wanted = Vec::new();
    for (id, name, _) in chosen.into_iter().filter(|c| c.2) {
        let playing = now_playing(pool, id, now).await?;
        let recent: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT track_id, started_at FROM music_plays
             WHERE user_id = ? AND track_id IS NOT NULL AND skipped = 0
               AND (ended_at IS NOT NULL OR completed = 1) AND started_at >= ?
             ORDER BY started_at DESC, id DESC LIMIT ?",
        )
        .bind(id)
        .bind(now - RECENT_MS)
        .bind(RECENT)
        .fetch_all(pool)
        .await?;
        wanted.extend(playing.iter().map(|p| p.0));
        wanted.extend(recent.iter().map(|r| r.0));
        people.push((name, playing, recent));
    }
    let tracks = track_values(pool, me, &wanted).await?;
    let people: Vec<Value> = people
        .into_iter()
        .map(|(name, playing, recent)| {
            let now_playing = playing.and_then(|(track, device, since)| {
                Some(json!({ "track": tracks.get(&track)?, "device": device,
                             "since": since.div_euclid(1000) }))
            });
            let recently: Vec<Value> = recent
                .iter()
                .filter_map(|(track, at)| {
                    Some(json!({ "track": tracks.get(track)?, "started_at": at.div_euclid(1000) }))
                })
                .collect();
            json!({ "user": name, "now_playing": now_playing, "recently_played": recently })
        })
        .collect();
    Ok(Some(json!(people)))
}

async fn household(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let me = current.user.id;
    let people = snapshot(&pool, me).await?.unwrap_or_else(|| json!([]));
    Ok(Json(
        json!({ "sharing": shares(&pool, me).await?, "people": people }),
    ))
}

async fn sharing(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({ "sharing": shares(&pool, current.user.id).await? }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sharing {
    sharing: bool,
}

/// Turns sharing what you listen to on or off, for yourself.
async fn set_sharing(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Sharing>,
) -> Result<Json<Value>, ApiError> {
    sqlx::query(
        "INSERT INTO music_listening_sharing (user_id, sharing, changed_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id) DO UPDATE SET sharing = ?2, changed_at = ?3",
    )
    .bind(current.user.id)
    .bind(body.sharing)
    .bind(now_ms().div_euclid(1000))
    .execute(&pool)
    .await?;
    Ok(Json(json!({ "sharing": body.sharing })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use http_body_util::BodyExt;
    use serde_json::value::RawValue;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::plays::{self, Listen, Source};

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

    async fn call(
        app: &Router,
        token: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(if body.is_some() { "PUT" } else { "GET" })
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

    async fn listen(pool: &SqlitePool, user: i64, id: &str, track: i64, at: i64, ended: bool) {
        let listen = Listen {
            event_id: id.into(),
            track_key: track.to_string(),
            device: "Living room iPad".into(),
            started_at: at,
            ended_at: ended.then_some(at + 1000),
            position_ms: None,
            completed: ended,
            skipped: false,
            activity: None,
        };
        plays::record(pool, user, Source::Api, &listen)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn only_those_who_share_are_seen_and_only_by_the_household() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
                 duration_seconds) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'A1', 'Alder', '[]', 200),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'A2', 'Alder', '[]', 200),
                 (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'A3', 'Alder', '[]', 200);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, alice_token) = user(&pool, "alice").await;
        let (bob, bob_token) = user(&pool, "bob").await;
        let now = now_ms();
        listen(&pool, alice, "old", 1, now - 3_600_000, true).await;
        listen(&pool, alice, "playing", 2, now - 30_000, false).await;
        listen(&pool, bob, "b", 3, now - 60_000, true).await;
        let app = routes(pool.clone());

        // Nobody shares until they choose to.
        let (_, seen) = call(&app, &bob_token, "/api/v1/music/household", None).await;
        assert_eq!(seen, json!({ "sharing": false, "people": [] }));
        assert_eq!(snapshot(&pool, bob).await.unwrap(), None);

        let (status, set) = call(
            &app,
            &alice_token,
            "/api/v1/music/household/sharing",
            Some(json!({ "sharing": true })),
        )
        .await;
        assert_eq!((status, set), (StatusCode::OK, json!({ "sharing": true })));
        let (_, seen) = call(&app, &bob_token, "/api/v1/music/household", None).await;
        assert_eq!(seen["sharing"], false);
        let people = seen["people"].as_array().unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0]["user"], "alice");
        assert_eq!(people[0]["now_playing"]["track"]["title"], "A2");
        assert_eq!(people[0]["now_playing"]["device"], "Living room iPad");
        let recent: Vec<&Value> = people[0]["recently_played"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| &r["track"]["title"])
            .collect();
        assert_eq!(
            recent,
            [&json!("A1")],
            "what's playing isn't yet recently played"
        );
        // Alice doesn't see herself; bob doesn't share, so nobody is listed for her.
        let (_, hers) = call(&app, &alice_token, "/api/v1/music/household", None).await;
        assert_eq!(hers, json!({ "sharing": true, "people": [] }));

        // Turned off, alice is gone, but the stream still says so (an empty list).
        call(
            &app,
            &alice_token,
            "/api/v1/music/household/sharing",
            Some(json!({ "sharing": false })),
        )
        .await;
        assert_eq!(snapshot(&pool, bob).await.unwrap(), Some(json!([])));
        assert_eq!(
            call(&app, &alice_token, "/api/v1/music/household/sharing", None)
                .await
                .1,
            json!({ "sharing": false })
        );
    }

    #[tokio::test]
    async fn an_aoide_devices_queue_is_what_it_plays_until_the_track_would_end() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
                 duration_seconds) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'A1', 'Alder', '[]', 200),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'A2', 'Alder', '[]', 200);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let (alice, _) = user(&pool, "alice").await;
        let (bob, _) = user(&pool, "bob").await;
        sqlx::query("INSERT INTO music_listening_sharing VALUES (?, 1, 0)")
            .bind(alice)
            .execute(&pool)
            .await
            .unwrap();
        let push = |received: i64| {
            let pool = pool.clone();
            async move {
                let op = r#"{"opId":"q","entity":"queue_state","entityId":"phone","operation":"upsert","payload":{"deviceName":"Phone","trackIds":["1","2"],"position":1,"elapsedMs":10000},"createdAt":1}"#;
                let op = crate::aoide::sync::validate(&RawValue::from_string(op.into()).unwrap())
                    .unwrap();
                sqlx::query("DELETE FROM aoide_ops")
                    .execute(&pool)
                    .await
                    .unwrap();
                crate::aoide::sync::push(&pool, alice, "phone", vec![op], received)
                    .await
                    .unwrap();
            }
        };
        push(now_ms() - 60_000).await;
        let seen = snapshot(&pool, bob).await.unwrap().unwrap();
        assert_eq!(seen[0]["now_playing"]["track"]["title"], "A2");
        assert_eq!(seen[0]["now_playing"]["device"], "Phone");
        // Pushed long enough ago that the track would have ended: not playing.
        push(now_ms() - 400_000).await;
        let seen = snapshot(&pool, bob).await.unwrap().unwrap();
        assert_eq!(seen[0]["now_playing"], Value::Null);
    }
}
