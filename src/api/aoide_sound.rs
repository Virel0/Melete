//! The aoide-sidecar's measurement routes: `/aoide/sound-bounds`
//! and `/aoide/audio-analysis` serve what `crate::sound` measured and keep what a client
//! measured itself; `/aoide/beat-grid` serves grids and keys, `/aoide/arrangement` sections,
//! phrases and vocals; `/aoide/analysis/coverage` says how far the library has got. A track not measured yet is reported pending and goes to the
//! front of the queue.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_music_sync};
use crate::sound::{self, ANALYZER, tempo};

mod keeping;
mod progress;
mod serving;

use keeping::*;
use progress::*;
use serving::*;

/// Ids per request, either way.
const MAX_IDS: usize = 200;
/// What a client may send: the contract's ranges.
const MINIMUM_LOUDNESS_LUFS: f64 = -100.0;
const MINIMUM_BPM: f64 = 20.0;
const MAXIMUM_BPM: f64 = 400.0;

/// The measurement routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/aoide/sound-bounds", get(get_bounds).post(post_bounds))
        .route(
            "/aoide/audio-analysis",
            get(get_analysis).post(post_analysis),
        )
        .route("/aoide/beat-grid", get(get_grids))
        .route("/aoide/arrangement", get(get_arrangements))
        .route("/aoide/analysis/coverage", get(coverage))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_music_sync))
        .with_state(pool)
}

#[derive(Deserialize)]
struct Ids {
    ids: Option<String>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

/// The asked-for ids that could be tracks, each once. An id that isn't a number is no track
/// of ours and is left out, like any unknown id.
fn requested(ids: Option<&str>) -> Result<Vec<i64>, ApiError> {
    let mut distinct: Vec<&str> = Vec::new();
    for id in ids.unwrap_or_default().split(',').map(str::trim) {
        if !id.is_empty() && !distinct.contains(&id) {
            distinct.push(id);
        }
    }
    if distinct.len() > MAX_IDS {
        return Err(bad(format!(
            "{} ids exceeds the limit of {MAX_IDS}. Split the request.",
            distinct.len()
        )));
    }
    let mut parsed: Vec<i64> = distinct.iter().filter_map(|id| id.parse().ok()).collect();
    parsed.sort_unstable();
    parsed.dedup();
    Ok(parsed)
}

fn json_ids(ids: &[i64]) -> String {
    serde_json::to_string(ids).expect("a list of whole numbers always serializes")
}

/// Answers with what's current; reports and queues what isn't. A row with an error, or a
/// track that isn't present, is left out: the client plays the file whole either way.
fn answer<T>(
    rows: Vec<(i64, bool, Option<String>, T)>,
    project: impl Fn(T) -> Value,
    key: &str,
) -> Value {
    let mut found = Map::new();
    let mut pending = Vec::new();
    for (id, current, error, row) in rows {
        if !current {
            pending.push(id);
        } else if error.is_none() {
            found.insert(id.to_string(), project(row));
        }
    }
    sound::REQUESTS.request(&pending);
    let pending: Vec<String> = pending.iter().map(i64::to_string).collect();
    json!({ key: found, "pending": pending })
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    async fn send(
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

    /// The keys of a documented schema and of a value, sorted, to compare.
    fn keys(spec: &Value, schema: &str, value: &Value) -> (Vec<String>, Vec<String>) {
        let mut documented: Vec<String> = spec["components"]["schemas"][schema]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut actual: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
        documented.sort();
        actual.sort();
        (actual, documented)
    }

    #[tokio::test]
    async fn serves_whats_current_queues_the_rest_and_keeps_what_clients_send() {
        let pool = crate::db::open_in_memory().await;
        // 101 measured; 102 measured before its file changed; 103 never; 104 failed; 105 gone;
        // 106 measured with nothing worth reporting; 107's analysis is from an older version.
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title, missing_since) VALUES
                 (101, 1, 'a', 1, 100, 1, 1, 0, 'A', NULL), (102, 1, 'b', 1, 200, 1, 1, 0, 'B', NULL),
                 (103, 1, 'c', 1, 100, 1, 1, 0, 'C', NULL), (104, 1, 'd', 1, 100, 1, 1, 0, 'D', NULL),
                 (105, 1, 'e', 1, 100, 1, 1, 0, 'E', 9), (106, 1, 'f', 1, 100, 1, 1, 0, 'F', NULL),
                 (107, 1, 'g', 1, 100, 1, 1, 0, 'G', NULL);
             INSERT INTO music_sound_bounds (track_id, modified_at, analyzer, source, error,
                 sound_start_ms, sound_end_ms, measured_at) VALUES
                 (101, 100, 2, 'server', NULL, 1940, 5200, 0), (102, 100, 2, 'server', NULL, 0, 9, 0),
                 (104, 100, 2, 'server', 'ffmpeg rejected the file', NULL, NULL, 0),
                 (106, 100, 2, 'server', NULL, NULL, NULL, 0), (107, 100, 2, 'server', NULL, NULL, NULL, 0);
             INSERT INTO music_audio_analysis (track_id, modified_at, analyzer, source, error,
                 loudness_lufs, true_peak_dbfs, bpm, bpm_confidence, bpm_stability, measured_at) VALUES
                 (101, 100, 2, 'server', NULL, -9.7, -0.3, 128, 0.82, 1.0, 0),
                 (102, 100, 2, 'server', NULL, -9.0, -1.0, NULL, NULL, NULL, 0),
                 (104, 100, 2, 'server', 'ffmpeg rejected the file', NULL, NULL, NULL, NULL, NULL, 0),
                 (106, 100, 2, 'server', NULL, NULL, NULL, 96, 0.31, 0.8, 0),
                 (107, 100, 1, 'server', NULL, -8.0, -1.0, NULL, NULL, NULL, 0);
             INSERT INTO music_beat_grids (track_id, modified_at, analyzer, measured_at)
                 VALUES (101, 100, 2, 0), (104, 100, 2, 0), (106, 100, 2, 0), (107, 100, 2, 0);
             INSERT INTO music_arrangements (track_id, modified_at, analyzer, measured_at)
                 VALUES (101, 100, 2, 0), (104, 100, 2, 0), (106, 100, 2, 0), (107, 100, 2, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        let ids = "?ids=101,102,103,104,105,106,107,abc,101";

        let (status, bounds) = send(
            &app,
            &token,
            "GET",
            &format!("/aoide/sound-bounds{ids}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{bounds}");
        assert_eq!(
            bounds,
            json!({
                "bounds": { "101": { "soundStartMs": 1940, "soundEndMs": 5200 }, "106": null, "107": null },
                "pending": ["102", "103"],
            })
        );
        let (a, d) = keys(&spec, "AoideSoundBoundsResponse", &bounds);
        assert_eq!(a, d);
        let (a, d) = keys(&spec, "AoideSoundBounds", &bounds["bounds"]["101"]);
        assert_eq!(a, d);

        let (_, analysis) = send(
            &app,
            &token,
            "GET",
            &format!("/aoide/audio-analysis{ids}"),
            None,
        )
        .await;
        assert_eq!(
            analysis,
            json!({
                "analysis": {
                    "101": { "loudnessLufs": -9.7, "truePeakDbfs": -0.3, "bpm": 128.0,
                             "bpmConfidence": 0.82, "bpmStability": 1.0 },
                    // An unsure tempo is withheld, and then there's nothing to report.
                    "106": null,
                },
                "pending": ["102", "103", "107"],
            })
        );
        let (a, d) = keys(&spec, "AoideAudioAnalysis", &analysis["analysis"]["101"]);
        assert_eq!(a, d);

        let (_, coverage) = send(&app, &token, "GET", "/aoide/analysis/coverage", None).await;
        assert_eq!(
            (
                &coverage["tracks"],
                &coverage["soundBounds"],
                &coverage["audioAnalysis"],
                &coverage["measuring"]
            ),
            (&json!(6), &json!(4), &json!(3), &json!(3))
        );
        assert!(
            coverage["summary"]
                .as_str()
                .unwrap()
                .starts_with("3 of 6 tracks measured (50%). 3 waiting")
        );
        let (a, d) = keys(&spec, "AoideCoverage", &coverage);
        assert_eq!(a, d);

        // What a client measured is served; every entry is checked before any is kept.
        let sent = json!({ "bounds": { "103": { "soundStartMs": 0, "soundEndMs": 1000 }, "102": null, "abc": null } });
        assert_eq!(
            send(&app, &token, "POST", "/aoide/sound-bounds", Some(sent))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let refused = json!({ "bounds": { "103": { "soundStartMs": 5, "soundEndMs": 6 }, "101": { "soundStartMs": 9, "soundEndMs": 9 } } });
        let (status, error) =
            send(&app, &token, "POST", "/aoide/sound-bounds", Some(refused)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            error["error"].as_str().unwrap().starts_with("101:"),
            "{error}"
        );
        let (_, after) = send(&app, &token, "GET", "/aoide/sound-bounds?ids=102,103", None).await;
        assert_eq!(
            after,
            json!({ "bounds": { "102": null, "103": { "soundStartMs": 0, "soundEndMs": 1000 } }, "pending": [] })
        );

        let half = json!({ "analysis": { "103": { "loudnessLufs": -10.0 } } });
        assert_eq!(
            send(&app, &token, "POST", "/aoide/audio-analysis", Some(half))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let tempo_only = json!({ "analysis": { "103": { "bpm": 120.0, "bpmStability": 0.5 } } });
        assert_eq!(
            send(
                &app,
                &token,
                "POST",
                "/aoide/audio-analysis",
                Some(tempo_only)
            )
            .await
            .0,
            StatusCode::NO_CONTENT
        );
        let (_, after) = send(&app, &token, "GET", "/aoide/audio-analysis?ids=103", None).await;
        assert_eq!(
            after["analysis"]["103"],
            json!({ "loudnessLufs": null, "truePeakDbfs": null, "bpm": 120.0, "bpmConfidence": 1.0, "bpmStability": 0.5 }),
            "a tempo sent without a confidence is taken at its word"
        );

        let many: Vec<String> = (1..=201).map(|i| i.to_string()).collect();
        let too_many = format!("/aoide/sound-bounds?ids={}", many.join(","));
        assert_eq!(
            send(&app, &token, "GET", &too_many, None).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            send(&app, &token, "POST", "/aoide/sound-bounds", Some(json!({})))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn serves_grids_with_their_keys_and_counts_them() {
        let pool = crate::db::open_in_memory().await;
        // 201 has a grid; 202 a key but no grid; 203 nothing yet; 204 failed.
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title) VALUES
                 (201, 1, 'a', 1, 100, 1, 1, 0, 'A'), (202, 1, 'b', 1, 100, 1, 1, 0, 'B'),
                 (203, 1, 'c', 1, 100, 1, 1, 0, 'C'), (204, 1, 'd', 1, 100, 1, 1, 0, 'D');
             INSERT INTO music_beat_grids (track_id, modified_at, analyzer, error, segments,
                 beats_per_bar, downbeat_index, mix_in_ms, mix_out_ms, key_camelot,
                 key_confidence, measured_at) VALUES
                 (201, 100, 2, NULL,
                  '[{\"startMs\":0.0,\"endMs\":180000.0,\"anchorMs\":-5.0,\"bpm\":128.0,\"residualMs\":3.5,\"beats\":384}]',
                  4, 0, -5.0, 178120.0, '8A', 0.62, 0),
                 (202, 100, 2, NULL, NULL, NULL, NULL, NULL, NULL, '9A', 0.07, 0),
                 (204, 100, 2, 'ffmpeg rejected the file', NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();

        let (status, body) = send(
            &app,
            &token,
            "GET",
            "/aoide/beat-grid?ids=201,202,203,204",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let grid = json!({
            "segments": [{ "startMs": 0.0, "endMs": 180000.0, "anchorMs": -5.0, "bpm": 128.0,
                           "residualMs": 3.5, "beats": 384 }],
            "beatsPerBar": 4, "downbeatIndex": 0, "mixInMs": -5.0, "mixOutMs": 178120.0,
            "key": "8A", "keyConfidence": 0.62,
        });
        // The key rides with the grid: without one, there's nothing to serve.
        assert_eq!(
            body,
            json!({ "grids": { "201": grid, "202": null }, "pending": ["203"] })
        );
        for (schema, value) in [
            ("AoideBeatGridResponse", &body),
            ("AoideBeatGrid", &body["grids"]["201"]),
            ("AoideBeatSegment", &body["grids"]["201"]["segments"][0]),
        ] {
            let (a, d) = keys(&spec, schema, value);
            assert_eq!(a, d, "{schema}");
        }

        let (_, coverage) = send(&app, &token, "GET", "/aoide/analysis/coverage", None).await;
        assert_eq!(
            (&coverage["beatGrids"], &coverage["gridded"]),
            (&json!(3), &json!(1))
        );
    }

    #[tokio::test]
    async fn serves_arrangements_telling_no_vocals_from_unknown_ones() {
        let pool = crate::db::open_in_memory().await;
        // 301 nobody singing; 302 couldn't tell; 303 nothing yet; 304 no structure; 305 has
        // everything but a grid and an arrangement, so it's still waiting.
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title) VALUES
                 (301, 1, 'a', 1, 100, 1, 1, 0, 'A'), (302, 1, 'b', 1, 100, 1, 1, 0, 'B'),
                 (303, 1, 'c', 1, 100, 1, 1, 0, 'C'), (304, 1, 'd', 1, 100, 1, 1, 0, 'D'),
                 (305, 1, 'e', 1, 100, 1, 1, 0, 'E');
             INSERT INTO music_arrangements (track_id, modified_at, analyzer, sections,
                 phrase_bars, phrase_anchor_ms, vocals, measured_at) VALUES
                 (301, 100, 2, '[{\"startMs\":0.0,\"endMs\":90000.0,\"kind\":\"intro\",\"energy\":0.1}]',
                  16, 7500.0, '[]', 0),
                 (302, 100, 2, '[{\"startMs\":0.0,\"endMs\":90000.0,\"kind\":\"unknown\",\"energy\":0.5}]',
                  NULL, NULL, NULL, 0),
                 (304, 100, 2, NULL, NULL, NULL, NULL, 0);
             INSERT INTO music_sound_bounds (track_id, modified_at, analyzer, source, measured_at)
                 VALUES (305, 100, 2, 'server', 0);
             INSERT INTO music_audio_analysis (track_id, modified_at, analyzer, source, measured_at)
                 VALUES (305, 100, 2, 'server', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();

        let (status, body) = send(
            &app,
            &token,
            "GET",
            "/aoide/arrangement?ids=301,302,303,304",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body,
            json!({
                "arrangements": {
                    "301": { "sections": [{ "startMs": 0.0, "endMs": 90000.0, "kind": "intro", "energy": 0.1 }],
                             "phraseBars": 16, "phraseAnchorMs": 7500.0, "vocals": [] },
                    "302": { "sections": [{ "startMs": 0.0, "endMs": 90000.0, "kind": "unknown", "energy": 0.5 }],
                             "phraseBars": null, "phraseAnchorMs": null, "vocals": null },
                    "304": null,
                },
                "pending": ["303"],
            })
        );
        for (schema, value) in [
            ("AoideArrangementResponse", &body),
            ("AoideArrangement", &body["arrangements"]["301"]),
            ("AoideSection", &body["arrangements"]["301"]["sections"][0]),
        ] {
            let (a, d) = keys(&spec, schema, value);
            assert_eq!(a, d, "{schema}");
        }

        let (_, coverage) = send(&app, &token, "GET", "/aoide/analysis/coverage", None).await;
        assert_eq!(
            (
                &coverage["arrangements"],
                &coverage["described"],
                &coverage["measuring"]
            ),
            (&json!(3), &json!(2), &json!(5))
        );
    }
}
