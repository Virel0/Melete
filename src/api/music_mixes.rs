//! Ambient mixes for the apps: the mixes and how many tracks fit each, and a mix's next
//! tracks, asked for as it plays. Chosen by how the tracks sound and the listener's taste;
//! nothing leaves the server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::{bad_request, tracks_by_id};
use crate::aoide::listening;
use crate::aoide::taste::{self, LibraryTrack, TasteProfile};
use crate::db::now_ms;
use crate::mixes::{self, Candidate, MIXES, Sound};

const DAY_MS: i64 = 86_400_000;
const MAX_QUEUE: usize = 1000;

/// The mix routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/mixes", get(list))
        .route("/api/v1/music/mixes/{key}/next", post(next))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

#[derive(sqlx::FromRow)]
struct SoundRow {
    id: i64,
    duration_seconds: Option<f64>,
    loudness_lufs: f64,
    bpm: Option<f64>,
    bpm_confidence: Option<f64>,
    vocals: Option<String>,
    album_id: Option<i64>,
}

#[derive(Deserialize)]
struct Span {
    #[serde(rename = "startMs")]
    start: f64,
    #[serde(rename = "endMs")]
    end: f64,
}

/// How much of a track has singing, from its vocal spans: none when that couldn't be told.
fn vocal_share(spans: Option<&str>, seconds: Option<f64>) -> Option<f64> {
    let spans: Vec<Span> = serde_json::from_str(spans?).ok()?;
    let length_ms = seconds.filter(|s| *s > 0.0)? * 1000.0;
    let sung: f64 = spans.iter().map(|s| (s.end - s.start).max(0.0)).sum();
    Some((sung / length_ms).clamp(0.0, 1.0))
}

/// What the mixes choose from for one listener: every present, measured track, with their
/// taste for it; what they heard in the last day; and what they flagged not interested.
async fn candidates(
    pool: &SqlitePool,
    user_id: i64,
    now: i64,
) -> Result<(Vec<Candidate>, HashSet<i64>, HashSet<i64>), ApiError> {
    let rows: Vec<SoundRow> = sqlx::query_as(
        "SELECT t.id, t.duration_seconds, a.loudness_lufs, a.bpm, a.bpm_confidence, r.vocals,
             t.album_id
         FROM music_tracks t
         JOIN music_audio_analysis a ON a.track_id = t.id AND a.error IS NULL
             AND a.loudness_lufs IS NOT NULL
         LEFT JOIN music_arrangements r ON r.track_id = t.id AND r.error IS NULL
         WHERE t.missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?;
    let library = super::aoide_next::library(pool).await?;
    let by_id: HashMap<&str, &LibraryTrack> = library
        .iter()
        .map(|t| (t.track.id.as_str(), &t.track))
        .collect();
    let history = listening::play_events(pool, user_id).await?;
    let (genres, artists, _) = taste::listened(&history, &by_id, now - taste::WINDOW_MS);
    let profile = TasteProfile { genres, artists };
    let heard: HashSet<i64> = history
        .iter()
        .filter(|e| e.started_at >= now - DAY_MS)
        .filter_map(|e| e.track.parse().ok())
        .collect();
    let flagged: HashSet<i64> = listening::not_interested(pool, user_id)
        .await?
        .iter()
        .filter_map(|t| t.parse().ok())
        .collect();
    let artists_of: HashMap<&str, &str> = library
        .iter()
        .map(|t| (t.track.id.as_str(), t.track_artist.as_str()))
        .collect();
    let candidates = rows
        .into_iter()
        .map(|row| {
            let key = row.id.to_string();
            let track = by_id.get(key.as_str());
            Candidate {
                id: row.id,
                sound: Sound {
                    loudness: row.loudness_lufs,
                    bpm: row
                        .bpm
                        .zip(row.bpm_confidence)
                        .and_then(|(bpm, confidence)| {
                            (confidence >= crate::sound::tempo::MINIMUM_CONFIDENCE).then_some(bpm)
                        }),
                    vocals: vocal_share(row.vocals.as_deref(), row.duration_seconds),
                },
                taste: track.map_or(0.0, |t| taste::taste(&t.artist, &t.genres, &profile, None)),
                album: row.album_id,
                artist: artists_of
                    .get(key.as_str())
                    .copied()
                    .unwrap_or("")
                    .to_owned(),
            }
        })
        .collect();
    Ok((candidates, heard, flagged))
}

/// Every ambient mix, with how many of the library's tracks fit it for the caller.
async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let (candidates, _, flagged) = candidates(&pool, current.user.id, now_ms()).await?;
    Ok(Json(json!(
        MIXES
            .iter()
            .map(|mix| {
                let fitting = candidates
                    .iter()
                    .filter(|c| !flagged.contains(&c.id) && mixes::fit(&c.sound, mix).is_some())
                    .count();
                json!({ "key": mix.key, "name": mix.name, "about": mix.about,
                        "tracks": fitting })
            })
            .collect::<Vec<_>>()
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NextBody {
    /// The tracks this mix has given so far, oldest first.
    #[serde(default)]
    queue: Vec<i64>,
    limit: Option<usize>,
}

/// A mix's next tracks, left out what it has given already.
async fn next(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(key): Path<String>,
    Json(body): Json<NextBody>,
) -> Result<Json<Value>, ApiError> {
    let Some(mix) = mixes::mix(&key) else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such mix"));
    };
    let limit = body.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) || body.queue.len() > MAX_QUEUE {
        return Err(bad_request(&format!(
            "limit is 1 to 100, and queue at most {MAX_QUEUE} ids"
        )));
    }
    let now = now_ms();
    let me = current.user.id;
    let (candidates, heard, flagged) = candidates(&pool, me, now).await?;
    let ids = mixes::next(
        mix,
        &candidates,
        &body.queue,
        &mixes::Listener {
            user: me,
            day: now.div_euclid(DAY_MS),
            heard_lately: &heard,
            flagged: &flagged,
        },
        limit,
    );
    let tracks = tracks_by_id(&pool, &current.access, me, &ids).await?;
    let tracks: Vec<&Value> = ids.iter().filter_map(|id| tracks.get(id)).collect();
    Ok(Json(
        json!({ "key": mix.key, "name": mix.name, "tracks": tracks }),
    ))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn call(
        app: &Router,
        token: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(if body.is_some() { "POST" } else { "GET" })
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

    #[test]
    fn singing_is_measured_against_the_track() {
        let spans = r#"[{"startMs":0,"endMs":30000},{"startMs":60000,"endMs":90000}]"#;
        assert_eq!(vocal_share(Some(spans), Some(120.0)), Some(0.5));
        assert_eq!(vocal_share(Some("[]"), Some(120.0)), Some(0.0));
        assert_eq!(vocal_share(None, Some(120.0)), None, "couldn't be told");
        assert_eq!(vocal_share(Some(spans), None), None);
    }

    #[tokio::test]
    async fn mixes_choose_by_sound_and_go_on_as_they_play() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'Quiet', 'Alder', 'a', 1), (2, 1, 'k2', 'Loud', 'Birch', 'b', 1),
                        (3, 1, 'k3', 'Calm', 'Cedar', 'c', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
                 duration_seconds) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Q1', 'Alder', '[]', 200),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Q2', 'Alder', '[]', 200),
                 (3, 1, 'b/1', 1, 1, 1, 1, 2, 1, 'L1', 'Birch', '[]', 200),
                 (4, 1, 'c/1', 1, 1, 1, 1, 3, 1, 'C1', 'Cedar', '[]', 200),
                 (5, 1, 'c/2', 1, 1, 1, 1, 3, 1, 'Unmeasured', 'Cedar', '[]', 200);
             INSERT INTO music_audio_analysis (track_id, modified_at, analyzer, source,
                 loudness_lufs, true_peak_dbfs, bpm, bpm_confidence, measured_at) VALUES
                 (1, 1, 1, 'server', -20, -3, 72, 0.9, 1),
                 (2, 1, 1, 'server', -21, -3, 80, 0.9, 1),
                 (3, 1, 1, 'server', -6, -0.5, 128, 0.9, 1),
                 (4, 1, 1, 'server', -14, -2, 95, 0.9, 1);
             INSERT INTO music_arrangements (track_id, modified_at, analyzer, sections, vocals,
                 measured_at) VALUES
                 (1, 1, 1, '[]', '[]', 1), (2, 1, 1, '[]', '[]', 1),
                 (3, 1, 1, '[]', '[{"startMs":0,"endMs":180000}]', 1),
                 (4, 1, 1, '[]', '[{"startMs":0,"endMs":100000}]', 1);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1;
        let app = routes(pool.clone());

        let (status, list) = call(&app, &token, "/api/v1/music/mixes", None).await;
        assert_eq!(status, StatusCode::OK);
        let counts: Vec<(&str, i64)> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["key"].as_str().unwrap(), m["tracks"].as_i64().unwrap()))
            .collect();
        // Quiet ones in Chill, Focus and Sleep; the calm sung one only in Chill; the loud one in
        // Energy; the unmeasured one nowhere.
        assert_eq!(
            counts,
            [("chill", 3), ("focus", 2), ("sleep", 2), ("energy", 1)]
        );

        let (status, first) = call(
            &app,
            &token,
            "/api/v1/music/mixes/sleep/next",
            Some(json!({ "limit": 1 })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["name"], "Sleep");
        let one = first["tracks"][0]["id"].as_i64().unwrap();
        assert!([1, 2].contains(&one));
        // Asked again with what it gave, it goes on to the other.
        let (_, second) = call(
            &app,
            &token,
            "/api/v1/music/mixes/sleep/next",
            Some(json!({ "queue": [one], "limit": 5 })),
        )
        .await;
        let ids: Vec<i64> = second["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, [3 - one]);

        assert_eq!(
            call(
                &app,
                &token,
                "/api/v1/music/mixes/party/next",
                Some(json!({}))
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            call(
                &app,
                &token,
                "/api/v1/music/mixes/sleep/next",
                Some(json!({ "limit": 0 }))
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
}
