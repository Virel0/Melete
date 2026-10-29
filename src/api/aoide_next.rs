//! The aoide-sidecar's listening routes, from the whole library and the
//! listener's sync log: `/aoide/next`, what plays next, and `/aoide/discover`, the records a
//! listener owns and hasn't heard through.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_music_sync};
use crate::aoide::automix;
use crate::aoide::discover::{self, DiscoverTrack};
use crate::aoide::listening;
use crate::aoide::next::{self, Measured, PlannerMixability};
use crate::aoide::planner::{Arrangement, Grid, Section, Segment, VocalSpan};
use crate::aoide::taste::LibraryTrack;
use crate::db::now_ms;

/// What plays next and what to discover, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/aoide/next", post(next_list))
        .route("/aoide/discover", get(discover_list))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth, require_music_sync))
        .with_state(pool)
}

#[derive(sqlx::FromRow)]
struct TrackRow {
    id: i64,
    artist: Option<String>,
    album_artist: String,
    album_id: Option<i64>,
    genres: String,
}

/// The first of several credited names, as the tags join them.
fn first_artist(artist: Option<&str>) -> Option<&str> {
    artist?
        .split([';', '\0'])
        .map(str::trim)
        .find(|a| !a.is_empty())
}

/// Every present track: its album artist (else its first artist, else "Unknown Artist"),
/// genres, own first artist and album.
pub(super) async fn library(pool: &SqlitePool) -> sqlx::Result<Vec<DiscoverTrack>> {
    let rows: Vec<TrackRow> = sqlx::query_as(
        "SELECT t.id, t.artist, coalesce(a.artist_name, '') AS album_artist, t.album_id, t.genres
         FROM music_tracks t LEFT JOIN music_albums a ON a.id = t.album_id
         WHERE t.missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let first = first_artist(row.artist.as_deref()).map(str::to_owned);
            let album_artist = Some(row.album_artist.trim())
                .filter(|a| !a.is_empty())
                .map(str::to_owned);
            let artist = album_artist
                .clone()
                .or_else(|| first.clone())
                .unwrap_or_else(|| "Unknown Artist".to_owned());
            DiscoverTrack {
                track_artist: first.unwrap_or_else(|| artist.clone()),
                album: row.album_id.map(|id| id.to_string()),
                track: LibraryTrack {
                    id: row.id.to_string(),
                    artist,
                    genres: serde_json::from_str(&row.genres).unwrap_or_default(),
                },
            }
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct MeasuredRow {
    id: i64,
    bpm: Option<f64>,
    bpm_stability: Option<f64>,
    segments: Option<String>,
    beats_per_bar: Option<i64>,
    mix_in_ms: Option<f64>,
    mix_out_ms: Option<f64>,
    key_camelot: Option<String>,
    sections: Option<String>,
    phrase_bars: Option<i64>,
    phrase_anchor_ms: Option<f64>,
    vocals: Option<String>,
}

/// What's measured about every present track, as the ranking and the planner read it.
/// Where a track's start and end parts are, in milliseconds, and its length.
type DeckInfo = (
    Option<automix::Region>,
    Option<automix::Region>,
    Option<f64>,
);

/// For each track named: the regions its band parts cover (from the separator's manifest, only
/// while the file is as it was split) and its length.
async fn decks(pool: &SqlitePool, ids: &[String]) -> sqlx::Result<HashMap<String, DeckInfo>> {
    let rows: Vec<(i64, Option<f64>, Option<String>)> = sqlx::query_as(
        "SELECT t.id, t.duration_seconds, s.manifest FROM music_tracks t
         LEFT JOIN music_stems s ON s.track_id = t.id AND s.state = 'done'
              AND s.size = t.size AND s.modified_at = t.modified_at
         WHERE t.id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(ids).unwrap_or_else(|_| "[]".into()))
    .fetch_all(pool)
    .await?;
    let mut out = HashMap::new();
    for (id, seconds, manifest) in rows {
        let manifest: Option<Value> = manifest.and_then(|m| serde_json::from_str(&m).ok());
        let region = |names: &[&str]| -> Option<automix::Region> {
            let m = manifest.as_ref()?;
            let rate = m["rate"].as_f64().filter(|r| *r > 0.0)?;
            let r = m["regions"]
                .as_array()?
                .iter()
                .find(|r| names.contains(&r["name"].as_str().unwrap_or_default()))?;
            Some(automix::Region {
                from_ms: r["start"].as_f64()? / rate * 1000.0,
                to_ms: r["end"].as_f64()? / rate * 1000.0,
            })
        };
        out.insert(
            id.to_string(),
            (
                region(&["start", "all"]),
                region(&["end", "all"]),
                seconds.map(|s| s * 1000.0),
            ),
        );
    }
    Ok(out)
}

async fn measured(pool: &SqlitePool) -> sqlx::Result<HashMap<String, Measured>> {
    let rows: Vec<MeasuredRow> = sqlx::query_as(
        "SELECT t.id, a.bpm, a.bpm_stability, g.segments, g.beats_per_bar, g.mix_in_ms,
             g.mix_out_ms, g.key_camelot, r.sections, r.phrase_bars, r.phrase_anchor_ms, r.vocals
         FROM music_tracks t
         LEFT JOIN music_audio_analysis a ON a.track_id = t.id AND a.error IS NULL
         LEFT JOIN music_beat_grids g ON g.track_id = t.id AND g.error IS NULL
         LEFT JOIN music_arrangements r ON r.track_id = t.id AND r.error IS NULL
         WHERE t.missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let segments: Option<Vec<Segment>> = row
                .segments
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());
            let sections: Option<Vec<Section>> = row
                .sections
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok());
            let measured = Measured {
                tempo: row.bpm.map(|bpm| (bpm, row.bpm_stability)),
                grid: segments.map(|segments| Grid {
                    segments,
                    beats_per_bar: row.beats_per_bar,
                    mix_in_ms: row.mix_in_ms,
                    mix_out_ms: row.mix_out_ms,
                }),
                key: row.key_camelot,
                arrangement: sections.map(|sections| Arrangement {
                    sections,
                    phrase_bars: row.phrase_bars,
                    phrase_anchor_ms: row.phrase_anchor_ms,
                    vocals: row
                        .vocals
                        .as_deref()
                        .and_then(|v| serde_json::from_str::<Vec<VocalSpan>>(v).ok()),
                }),
            };
            (row.id.to_string(), measured)
        })
        .collect())
}

#[derive(Deserialize)]
struct NextBody {
    seed: String,
    #[serde(default)]
    queue: Vec<String>,
    #[serde(default)]
    recent: Vec<String>,
    mode: Option<String>,
    limit: Option<i64>,
}

fn bad(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

/// The library ranked for what follows the seed: chosen by taste, kinship, similarity and
/// arc, and in Auto DJ mode put in the order the planner says they mix. `mixability` is
/// always written, and null outside Auto DJ or for a pair either side of which is unread.
async fn next_list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    bytes: Bytes,
) -> Result<Json<Value>, ApiError> {
    let shape = "send { seed, queue, recent, mode, limit }";
    let body: NextBody = serde_json::from_slice(&bytes).map_err(|_| bad(shape))?;
    if body.seed.trim().is_empty() {
        return Err(bad(shape));
    }
    let auto_dj = match body.mode.as_deref().map(str::to_lowercase).as_deref() {
        None | Some("" | "infinity") => false,
        Some("autodj") => true,
        Some(_) => return Err(bad("mode must be \"infinity\" or \"autodj\"")),
    };
    // Auto DJ is off unless the household turns it on; off, it's Infinity's order.
    let auto_dj = auto_dj && crate::settings::auto_dj(&pool).await?;
    let limit = body.limit.unwrap_or(20);
    if !(1..=200).contains(&limit) || body.queue.len() > 1000 || body.recent.len() > 40 {
        return Err(bad(
            "limit must be 1 to 200; queue at most 1000 ids; recent at most 40",
        ));
    }
    let library: Vec<LibraryTrack> = library(&pool).await?.into_iter().map(|t| t.track).collect();
    let seed = listening::normalise(&body.seed);
    if !library.iter().any(|t| t.id == seed) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "the seed isn't a track this user can see",
        ));
    }
    let history = listening::play_events(&pool, current.user.id).await?;
    let flags = listening::not_interested(&pool, current.user.id).await?;
    let measured = measured(&pool).await?;
    let request = next::Request {
        seed,
        queue: body
            .queue
            .iter()
            .map(|id| listening::normalise(id))
            .collect(),
        recent: body
            .recent
            .iter()
            .map(|id| listening::normalise(id))
            .collect(),
        auto_dj,
        limit: limit as usize,
    };
    let ranked = next::rank(
        &request,
        &library,
        &history,
        &flags,
        &measured,
        &PlannerMixability(&measured),
        now_ms(),
    );
    let mut candidates: Vec<Value> = ranked
        .candidates
        .iter()
        .map(|c| {
            let f = &c.factors;
            json!({
                "id": c.id,
                "score": c.score,
                "factors": {
                    "taste": f.taste, "freshness": f.freshness, "kinship": f.kinship,
                    "similarity": f.similarity, "mixability": f.mixability, "arc": f.arc,
                },
            })
        })
        .collect();
    // In Auto DJ, each record comes with how to mix into it from the one before.
    if auto_dj {
        let mut chain = vec![request.seed.clone()];
        chain.extend(ranked.candidates.iter().map(|c| c.id.clone()));
        let decks = decks(&pool, &chain).await?;
        let deck = |id: &str, end: bool| {
            let (start_region, end_region, length) = decks.get(id).copied().unwrap_or_default();
            automix::Deck {
                record: measured.get(id).and_then(next::mix_record),
                parts: if end { end_region } else { start_region },
                length_ms: length,
            }
        };
        for (i, candidate) in candidates.iter_mut().enumerate() {
            let plan = automix::mix(deck(&chain[i], true), deck(&chain[i + 1], false));
            candidate["mix"] = serde_json::to_value(plan).map_err(anyhow::Error::from)?;
        }
    }
    Ok(Json(json!({
        "candidates": candidates,
        "profile": { "events": ranked.profile_events, "since": ranked.profile_since },
    })))
}

#[derive(Deserialize)]
struct DiscoverQuery {
    limit: Option<String>,
}

/// The week's list: score (taste) descending, then the week's shuffle, then id, at most two
/// by one artist and three from one album. Only taste is measured; the other factors are
/// null for want of a seed, and freshness is 1 because nothing in the pool has been heard.
async fn discover_list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<DiscoverQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = match query.limit.as_deref() {
        None => 30,
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|l| (1..=100).contains(l))
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "limit is 1 to 100"))?,
    };
    let library = library(&pool).await?;
    let history = listening::play_events(&pool, current.user.id).await?;
    let flags = listening::not_interested(&pool, current.user.id).await?;
    let result = discover::rank(
        &current.user.id.to_string(),
        &library,
        &history,
        &flags,
        limit,
        now_ms(),
    );
    let candidates: Vec<Value> = result
        .candidates
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "score": c.score,
                "factors": {
                    "taste": c.taste, "kinship": null, "freshness": 1,
                    "similarity": null, "mixability": null, "arc": null,
                },
            })
        })
        .collect();
    Ok(Json(json!({
        "week": result.week,
        "candidates": candidates,
        "profile": { "events": result.profile_events, "since": result.profile_since },
    })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

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
    async fn discovers_from_the_library_and_the_sync_log() {
        let pool = crate::db::open_in_memory().await;
        // Two rock records heard through; 3 and 4 are rock, never heard; 5 is flagged; 6 is jazz.
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1), (2, 1, 'k2', 'Two', 'Birch', 'b', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'A1', 'Alder', '["Rock"]'),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'A2', 'Alder', '["Rock"]'),
                 (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'A3', 'Alder', '["Rock"]'),
                 (4, 1, 'b/1', 1, 1, 1, 1, 2, 1, 'B1', 'Birch', '["Rock"]'),
                 (5, 1, 'b/2', 1, 1, 1, 1, 2, 1, 'B2', 'Birch', '["Rock"]'),
                 (6, 1, 'b/3', 1, 1, 1, 1, 2, 1, 'B3', 'Birch', '["Jazz"]');"#,
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
        // Last week, so it counts for this week's list.
        let (_, week_start) = discover::week(now_ms());
        let before = week_start - 2 * 86_400_000;
        let ops = [
            (
                "e1",
                "play_events",
                format!(r#"{{"jellyfinId":"1","startedAt":{before},"completed":true}}"#),
            ),
            (
                "e2",
                "play_events",
                format!(r#"{{"jellyfinId":"2","startedAt":{before},"completed":true}}"#),
            ),
            (
                "f1",
                "track_flags",
                r#"{"jellyfinId":"5","notInterested":true}"#.to_owned(),
            ),
        ];
        for (i, (entity_id, entity, payload)) in ops.iter().enumerate() {
            sqlx::query(
                "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                     payload, created_at, received_at)
                 VALUES (?, ?, 'd', ?, ?, 'upsert', ?, 0, 0)",
            )
            .bind(format!("op{i}"))
            .bind(user.id)
            .bind(entity)
            .bind(entity_id)
            .bind(payload)
            .execute(&pool)
            .await
            .unwrap();
        }
        let app = routes(pool);

        let (status, body) = get(&app, &token, "/aoide/discover").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ids: Vec<&str> = body["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        // Heard through and flagged are out; the rock ones lead, by artist Alder (both plays)
        // first; the jazz one, sharing nothing, last.
        assert_eq!(ids, ["3", "4", "6"]);
        assert_eq!(body["candidates"][0]["score"], 0.75);
        assert_eq!(body["profile"]["events"], 2);
        assert_eq!(body["week"], discover::week(now_ms()).0);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        for (schema, value) in [
            ("AoideDiscoverResponse", &body),
            ("AoideCandidate", &body["candidates"][0]),
            ("AoideFactors", &body["candidates"][0]["factors"]),
            ("AoideProfile", &body["profile"]),
        ] {
            let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect();
            let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
            documented.sort();
            actual.sort();
            assert_eq!(actual, documented, "{schema}");
        }
        assert_eq!(
            get(&app, &token, "/aoide/discover?limit=1").await.1["candidates"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        for bad in ["0", "101", "x"] {
            assert_eq!(
                get(&app, &token, &format!("/aoide/discover?limit={bad}"))
                    .await
                    .0,
                StatusCode::BAD_REQUEST
            );
        }
    }

    async fn post_next(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post("/aoide/next")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
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
    async fn auto_dj_says_how_to_mix_into_each_record() {
        let pool = crate::db::open_in_memory().await;
        // Two house records at 128 that mix, the first split into parts; Auto DJ on.
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres, duration_seconds) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Seed', 'Alder', '["House"]', 240.0),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Next', 'Alder', '["House"]', 240.0);
             INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_AUTO_DJ', 'on', 0);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let grid = r#"[{"startMs":0.0,"endMs":240000.0,"anchorMs":0.0,"bpm":128.0,"residualMs":5.0,"beats":512}]"#;
        let sections = r#"[{"startMs":0.0,"endMs":30000.0,"kind":"intro","energy":0.2},{"startMs":30000.0,"endMs":210000.0,"kind":"drop","energy":0.9},{"startMs":210000.0,"endMs":240000.0,"kind":"outro","energy":0.2}]"#;
        for id in [1, 2] {
            sqlx::query(
                "INSERT INTO music_beat_grids (track_id, modified_at, analyzer, segments, beats_per_bar,
                     downbeat_index, mix_in_ms, mix_out_ms, key_camelot, key_confidence, measured_at)
                 VALUES (?, 1, 2, ?, 4, 0, 15000.0, 210000.0, '8A', 0.9, 0);
                 INSERT INTO music_arrangements (track_id, modified_at, analyzer, sections, phrase_bars,
                     phrase_anchor_ms, vocals, measured_at)
                 VALUES (?1, 1, 2, ?3, 16, 0.0, '[]', 0)",
            )
            .bind(id)
            .bind(grid)
            .bind(sections)
            .execute(&pool)
            .await
            .unwrap();
        }
        // Parts over the whole of both, so the mix has lanes.
        let manifest =
            r#"{"rate":1000,"regions":[{"name":"all","start":0,"end":240000,"parts":{}}]}"#;
        for id in [1, 2] {
            sqlx::query(
                "INSERT INTO music_stems (track_id, state, size, modified_at, folder, manifest, queued_at, finished_at)
                 VALUES (?, 'done', 1, 1, 'x', ?, 0, 1)",
            )
            .bind(id)
            .bind(manifest)
            .execute(&pool)
            .await
            .unwrap();
        }
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool.clone());
        let (status, body) =
            post_next(&app, &token, json!({ "seed": "1", "mode": "autodj" })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let mix = &body["candidates"][0]["mix"];
        assert_eq!(
            (&mix["kind"], &mix["style"]),
            (&json!("beatmatch"), &json!("blend")),
            "{mix}"
        );
        assert!(mix["lanes"].as_array().unwrap().len() >= 10, "{mix}");
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        for (schema, value) in [
            ("AoideAutoDjCandidate", &body["candidates"][0]),
            ("AoideMix", mix),
            ("AoideMixLane", &mix["lanes"][0]),
        ] {
            let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect();
            let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
            documented.sort();
            actual.sort();
            assert_eq!(actual, documented, "{schema}");
        }
        // Infinity's list carries no plans, nor Auto DJ's once it's turned off.
        let (_, infinity) = post_next(&app, &token, json!({ "seed": "1" })).await;
        assert!(infinity["candidates"][0].get("mix").is_none());
        sqlx::query("DELETE FROM settings WHERE name = 'MELETE_AUTO_DJ'")
            .execute(&pool)
            .await
            .unwrap();
        let (_, off) = post_next(&app, &token, json!({ "seed": "1", "mode": "autodj" })).await;
        assert!(off["candidates"][0].get("mix").is_none());
    }

    #[tokio::test]
    async fn plays_next_from_the_library_and_orders_auto_dj_by_the_planner() {
        let pool = crate::db::open_in_memory().await;
        // A 128 BPM seed and three candidates: 2 and 3 house and fully measured (2 mixes from
        // the seed, 3 clashes in key), 4 jazz and unmeasured; 5 is flagged.
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, artist, genres) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Seed', 'Alder', '["House"]'),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Mixes', 'Alder', '["House"]'),
                 (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'Clashes', 'Alder', '["House"]'),
                 (4, 1, 'a/4', 1, 1, 1, 1, 1, 1, 'Jazz', 'Alder', '["Jazz"]'),
                 (5, 1, 'a/5', 1, 1, 1, 1, 1, 1, 'Flagged', 'Alder', '["House"]');"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let grid = r#"[{"startMs":0.0,"endMs":240000.0,"anchorMs":0.0,"bpm":128.0,"residualMs":5.0,"beats":512}]"#;
        let sections = r#"[{"startMs":0.0,"endMs":30000.0,"kind":"intro","energy":0.2},{"startMs":30000.0,"endMs":210000.0,"kind":"drop","energy":0.9},{"startMs":210000.0,"endMs":240000.0,"kind":"outro","energy":0.2}]"#;
        for (id, key) in [(1, "8A"), (2, "9A"), (3, "3B")] {
            sqlx::query(
                "INSERT INTO music_beat_grids (track_id, modified_at, analyzer, segments, beats_per_bar,
                     downbeat_index, mix_in_ms, mix_out_ms, key_camelot, key_confidence, measured_at)
                 VALUES (?, 1, 2, ?, 4, 0, 15000.0, 210000.0, ?, 0.9, 0)",
            )
            .bind(id)
            .bind(grid)
            .bind(key)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO music_arrangements (track_id, modified_at, analyzer, sections, phrase_bars,
                     phrase_anchor_ms, vocals, measured_at)
                 VALUES (?, 1, 2, ?, 16, 0.0, '[]', 0)",
            )
            .bind(id)
            .bind(sections)
            .execute(&pool)
            .await
            .unwrap();
        }
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation, payload,
                 created_at, received_at)
             VALUES ('f', ?, 'd', 'track_flags', 'f', 'upsert', '{\"jellyfinId\":\"5\",\"notInterested\":true}', 0, 0)",
        )
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        let app = routes(pool.clone());
        let ids = |body: &Value| -> Vec<String> {
            body["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap().to_owned())
                .collect()
        };

        // Chosen by kin: the two house records lead (2 nearer the seed's key), jazz last.
        let (status, body) = post_next(&app, &token, json!({ "seed": "1" })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(ids(&body), ["2", "3", "4"]);
        assert!(
            body["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["factors"]["mixability"].is_null())
        );
        assert_eq!(body["candidates"][0]["factors"]["kinship"], 1.0);
        let spec: Value = serde_json::from_str(super::super::OPENAPI).unwrap();
        for (schema, value) in [
            ("AoideNextResponse", &body),
            ("AoideFactors", &body["candidates"][0]["factors"]),
        ] {
            let mut documented: Vec<&String> = spec["components"]["schemas"][schema]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .collect();
            let mut actual: Vec<&String> = value.as_object().unwrap().keys().collect();
            documented.sort();
            actual.sort();
            assert_eq!(actual, documented, "{schema}");
        }

        // Auto DJ: the planner's real score from the seed for 2; 3 clashes but is still read,
        // and comes before the unread jazz record, which counts only the crossfade figure.
        // Auto DJ is off unless the household turns it on.
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_AUTO_DJ', 'on', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (_, body) = post_next(&app, &token, json!({ "seed": "1", "mode": "autodj" })).await;
        assert_eq!(ids(&body), ["2", "3", "4"]);
        let mixability: Vec<Option<f64>> = body["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["factors"]["mixability"].as_f64())
            .collect();
        assert!(
            mixability[0].unwrap() > mixability[1].unwrap(),
            "{mixability:?}"
        );
        assert_eq!(mixability[2], None, "an unread record is null, not 0");

        // The queue and the recent list are never returned.
        let (_, body) = post_next(
            &app,
            &token,
            json!({ "seed": "1", "queue": ["2"], "recent": ["3"] }),
        )
        .await;
        assert_eq!(ids(&body), ["4"]);

        assert_eq!(
            post_next(&app, &token, json!({ "seed": "99" })).await.0,
            StatusCode::NOT_FOUND
        );
        for bad in [
            json!({}),
            json!({ "seed": "1", "mode": "party" }),
            json!({ "seed": "1", "limit": 0 }),
        ] {
            assert_eq!(
                post_next(&app, &token, bad).await.0,
                StatusCode::BAD_REQUEST
            );
        }
    }
}
