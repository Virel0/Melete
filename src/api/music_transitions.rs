//! Auto DJ's transitions as they were played, kept beside the play history so the rules
//! that plan them can be tuned from how they went. The app that played one sends
//! it once the mix is over or the listener skipped during it: which records, the plan as played
//! (its kind, length and style, whether part by part, the key shifts and the planner's score)
//! and whether it was skipped during the mix, with the version of the rules that planned it.
//! Each user sends and reads only their own; admins read a summary of the household's, in
//! counts only.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin, require_user};
use super::music::{bad_request, paging};
use crate::db::now_ms;

/// The longest id, style or device name an app may give.
const MAX_TEXT: usize = 128;
/// How far ahead of the server's clock a transition may say it started.
const CLOCK_SLACK_MS: i64 = 86_400_000;
/// The longest mix kept, in seconds.
const LONGEST: f64 = 600.0;
/// The widest key shift, in semitones, either way.
const WIDEST_SHIFT: i64 = 12;
const KINDS: [&str; 4] = ["beatmatch", "ramp", "throw", "crossfade"];
/// The summary's period, in days: by default, and at most.
const SUMMARY_DAYS: i64 = 30;
const MOST_DAYS: i64 = 365;

/// Each user's own transitions, for everyone signed in; the household's summary, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    let own = Router::new()
        .route("/api/v1/music/transitions", get(list).post(add))
        .route_layer(middleware::from_fn_with_state(
            auth.pool.clone(),
            super::access::require_all_music,
        ))
        .route_layer(middleware::from_fn_with_state(auth.clone(), require_user));
    let household = Router::new()
        .route("/api/v1/music/transitions/summary", get(summary))
        .route_layer(middleware::from_fn_with_state(auth, require_admin));
    own.merge(household).with_state(pool)
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    device: String,
    from_track_id: i64,
    to_track_id: i64,
    play_event_id: Option<String>,
    started_at: i64,
    kind: String,
    seconds: f64,
    style: Option<String>,
    by_parts: bool,
    band_semitones: i64,
    voice_semitones: i64,
    score: Option<f64>,
    skipped: bool,
    skipped_at_seconds: Option<f64>,
    rules: Option<i64>,
    /// The listen it led into, when the app named it and it's in the history.
    listen_id: Option<i64>,
    listen_completed: Option<bool>,
    listen_skipped: Option<bool>,
    listen_position_ms: Option<i64>,
}

/// Each transition with the listen it led into, by the app's id for that listen.
const SELECT: &str = "SELECT t.id, t.device, t.from_track_id, t.to_track_id, t.play_event_id,
        t.started_at, t.kind, t.seconds, t.style, t.by_parts, t.band_semitones,
        t.voice_semitones, t.score, t.skipped, t.skipped_at_seconds, t.rules,
        p.id AS listen_id, p.completed AS listen_completed, p.skipped AS listen_skipped,
        p.position_ms AS listen_position_ms
    FROM music_transitions t
    LEFT JOIN music_plays p ON p.id = (
        SELECT max(id) FROM music_plays
        WHERE user_id = t.user_id AND event_id = t.play_event_id)";

fn to_json(row: &Row) -> Value {
    let listen = row.listen_id.map(|id| {
        json!({
            "id": id,
            "completed": row.listen_completed.unwrap_or(false),
            "skipped": row.listen_skipped.unwrap_or(false),
            "position_seconds": row.listen_position_ms.map(|p| p as f64 / 1000.0),
        })
    });
    json!({
        "id": row.id,
        "from_track_id": row.from_track_id,
        "to_track_id": row.to_track_id,
        "started_at": row.started_at.div_euclid(1000),
        "kind": row.kind,
        "seconds": row.seconds,
        "style": row.style,
        "by_parts": row.by_parts,
        "band_semitones": row.band_semitones,
        "voice_semitones": row.voice_semitones,
        "score": row.score,
        "skipped": row.skipped,
        "skipped_at_seconds": row.skipped_at_seconds,
        "rules": row.rules,
        "play_id": row.play_event_id,
        "listen": listen,
        "device": row.device,
    })
}

#[derive(Deserialize)]
struct ListQuery {
    /// Only transitions started at or after this, in Unix seconds.
    since: Option<i64>,
    kind: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// The caller's transitions, newest first.
async fn list(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let (limit, offset) = paging(query.limit, query.offset)?;
    if query.kind.as_deref().is_some_and(|k| !KINDS.contains(&k)) {
        return Err(bad_request("kind is beatmatch, ramp, throw or crossfade"));
    }
    // Listens synced by Aoide's devices are read in, so each transition finds its listen.
    crate::plays::catch_up(&pool).await?;
    let since = query.since.map(|s| s.saturating_mul(1000));
    let filter = "WHERE t.user_id = ?1 AND (?2 IS NULL OR t.started_at >= ?2)
         AND (?3 IS NULL OR t.kind = ?3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM music_transitions t {filter}"
    )))
    .bind(current.user.id)
    .bind(since)
    .bind(&query.kind)
    .fetch_one(&pool)
    .await?;
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} {filter} ORDER BY t.started_at DESC, t.id DESC LIMIT ?4 OFFSET ?5"
    )))
    .bind(current.user.id)
    .bind(since)
    .bind(&query.kind)
    .bind(limit)
    .bind(offset)
    .fetch_all(&pool)
    .await?;
    Ok(Json(
        json!({ "total": total, "items": rows.iter().map(to_json).collect::<Vec<_>>() }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewTransition {
    /// The app's own id for it: sent again, it's changed, not added twice.
    id: Option<String>,
    from_track_id: i64,
    to_track_id: i64,
    /// Unix seconds, when the mix began.
    started_at: i64,
    /// The plan's `kind`, `seconds` and `style`, as `/aoide/next` gave them.
    kind: String,
    seconds: f64,
    style: Option<String>,
    /// Whether it was played part by part (the plan had lanes and the deck used them).
    #[serde(default)]
    by_parts: bool,
    #[serde(default)]
    band_semitones: i64,
    #[serde(default)]
    voice_semitones: i64,
    /// The incoming record's `factors.mixability` from `/aoide/next`.
    score: Option<f64>,
    /// Whether the listener skipped while it played, and how far into it.
    #[serde(default)]
    skipped: bool,
    skipped_at_seconds: Option<f64>,
    /// The app's id for the listen it led into, as sent with that listen.
    play_id: Option<String>,
    device: Option<String>,
    /// The plan's `rules`, the version of the rules that made it; apps that don't send it
    /// yet are still heard.
    rules: Option<i64>,
}

/// A trimmed text of 1 to [`MAX_TEXT`] characters, none when left out.
fn text(value: Option<String>, name: &str) -> Result<Option<String>, ApiError> {
    match value.map(|v| v.trim().to_owned()) {
        Some(v) if !v.is_empty() && v.chars().count() <= MAX_TEXT => Ok(Some(v)),
        Some(_) => Err(bad_request(&format!(
            "{name} must be 1 to {MAX_TEXT} characters"
        ))),
        None => Ok(None),
    }
}

/// Keeps a transition the caller's app played.
async fn add(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<NewTransition>,
) -> Result<Json<Value>, ApiError> {
    let (started_at, skipped_at) = checked(&body)?;
    let event_id = match text(body.id, "id")? {
        Some(id) => id,
        None => crate::auth::random_token("server-")?,
    };
    let style = text(body.style, "style")?;
    let play_id = text(body.play_id, "play_id")?;
    tracks_known(&pool, body.from_track_id, body.to_track_id).await?;
    let device = device_name(&pool, body.device, current.token_id).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO music_transitions (user_id, event_id, device, from_track_id, to_track_id,
             play_event_id, started_at, kind, seconds, style, by_parts, band_semitones,
             voice_semitones, score, skipped, skipped_at_seconds, rules)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, event_id) DO UPDATE SET
             device = excluded.device, from_track_id = excluded.from_track_id,
             to_track_id = excluded.to_track_id, play_event_id = excluded.play_event_id,
             started_at = excluded.started_at, kind = excluded.kind,
             seconds = excluded.seconds, style = excluded.style, by_parts = excluded.by_parts,
             band_semitones = excluded.band_semitones,
             voice_semitones = excluded.voice_semitones, score = excluded.score,
             skipped = excluded.skipped, skipped_at_seconds = excluded.skipped_at_seconds,
             rules = excluded.rules
         RETURNING id",
    )
    .bind(current.user.id)
    .bind(&event_id)
    .bind(&device)
    .bind(body.from_track_id)
    .bind(body.to_track_id)
    .bind(&play_id)
    .bind(started_at)
    .bind(&body.kind)
    .bind(body.seconds)
    .bind(&style)
    .bind(body.by_parts)
    .bind(body.band_semitones)
    .bind(body.voice_semitones)
    .bind(body.score)
    .bind(body.skipped)
    .bind(skipped_at)
    .bind(body.rules)
    .fetch_one(&pool)
    .await?;
    crate::plays::catch_up(&pool).await?;
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE t.id = ?")))
        .bind(id)
        .fetch_one(&pool)
        .await?;
    Ok(Json(to_json(&row)))
}

/// When the mix began, in milliseconds, and how far into it the listener skipped, once every
/// figure of the plan is checked.
fn checked(body: &NewTransition) -> Result<(i64, Option<f64>), ApiError> {
    let started_at = body.started_at.saturating_mul(1000);
    if started_at <= 0 || started_at > now_ms() + CLOCK_SLACK_MS {
        return Err(bad_request(
            "started_at must be a time in Unix seconds, not in the future",
        ));
    }
    if !KINDS.contains(&body.kind.as_str()) {
        return Err(bad_request("kind is beatmatch, ramp, throw or crossfade"));
    }
    if !(body.seconds.is_finite() && body.seconds > 0.0 && body.seconds <= LONGEST) {
        return Err(bad_request("seconds must be more than 0 and at most 600"));
    }
    if body.band_semitones.abs() > WIDEST_SHIFT || body.voice_semitones.abs() > WIDEST_SHIFT {
        return Err(bad_request(
            "a key shift is at most 12 semitones either way",
        ));
    }
    if body
        .score
        .is_some_and(|s| !(s.is_finite() && (0.0..=1.0).contains(&s)))
    {
        return Err(bad_request("score is from 0 to 1"));
    }
    if body.rules.is_some_and(|r| r < 1) {
        return Err(bad_request("rules is a version, 1 or more"));
    }
    let skipped_at = match body.skipped_at_seconds {
        Some(_) if !body.skipped => {
            return Err(bad_request("skipped_at_seconds is only for a skipped one"));
        }
        Some(s) if !(s.is_finite() && s >= 0.0) => {
            return Err(bad_request("skipped_at_seconds must be 0 or more"));
        }
        other => other,
    };
    Ok((started_at, skipped_at))
}

/// Refuses a transition from or to a track that isn't in the library.
async fn tracks_known(
    pool: &SqlitePool,
    from_track_id: i64,
    to_track_id: i64,
) -> Result<(), ApiError> {
    let known: i64 = sqlx::query_scalar("SELECT count(*) FROM music_tracks WHERE id IN (?, ?)")
        .bind(from_track_id)
        .bind(to_track_id)
        .fetch_one(pool)
        .await?;
    let wanted = if from_track_id == to_track_id { 1 } else { 2 };
    if known < wanted {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such track"));
    }
    Ok(())
}

/// The name of the device that played the transition.
async fn device_name(
    pool: &SqlitePool,
    given: Option<String>,
    token_id: i64,
) -> Result<String, ApiError> {
    // As for listens: the device's name as the app gives it, else the session's.
    Ok(match given.map(|d| d.trim().to_owned()) {
        Some(d) if !d.is_empty() => d.chars().take(MAX_TEXT).collect(),
        _ => sqlx::query_scalar("SELECT name FROM tokens WHERE id = ?")
            .bind(token_id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_else(|| "unknown device".to_owned()),
    })
}

#[derive(Deserialize)]
struct SummaryQuery {
    days: Option<i64>,
}

/// The household's transitions in the last `days`, grouped by `column`: each value with how
/// many there were and how many were skipped during the mix, most first.
async fn groups(pool: &SqlitePool, column: &str, since: i64) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<(Value, i64, i64)> =
        sqlx::query_as::<_, (Option<String>, i64, i64)>(sqlx::AssertSqlSafe(format!(
            "SELECT CAST({column} AS TEXT), count(*), sum(skipped) FROM music_transitions
             WHERE started_at >= ? GROUP BY {column} ORDER BY count(*) DESC, {column}"
        )))
        .bind(since)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(value, count, skipped)| {
            // Versions are numbers; kinds and styles are text.
            let value = match (column, value) {
                ("rules", Some(v)) => v.parse::<i64>().map_or(Value::Null, Value::from),
                (_, v) => v.map_or(Value::Null, Value::from),
            };
            (value, count, skipped)
        })
        .collect();
    Ok(rows
        .into_iter()
        .map(|(value, count, skipped)| {
            json!({
                "value": value,
                "count": count,
                "skipped": skipped,
                "skip_rate": skipped as f64 / count as f64,
            })
        })
        .collect())
}

/// The household's Auto DJ transitions over a period, for admins tuning the rules: counts by
/// kind, style and rules version, and how often each was skipped during the mix. Counts only:
/// nothing says who played what.
async fn summary(
    State(pool): State<SqlitePool>,
    Query(query): Query<SummaryQuery>,
) -> Result<Json<Value>, ApiError> {
    let days = query.days.unwrap_or(SUMMARY_DAYS);
    if !(1..=MOST_DAYS).contains(&days) {
        return Err(bad_request("days is 1 to 365"));
    }
    let since = now_ms() - days * 86_400_000;
    let (total, skipped): (i64, Option<i64>) = sqlx::query_as(
        "SELECT count(*), sum(skipped) FROM music_transitions WHERE started_at >= ?",
    )
    .bind(since)
    .fetch_one(&pool)
    .await?;
    let skipped = skipped.unwrap_or(0);
    Ok(Json(json!({
        "days": days,
        "since": since.div_euclid(1000),
        "total": total,
        "skipped": skipped,
        "skip_rate": (total > 0).then(|| skipped as f64 / total as f64),
        "by_kind": groups(&pool, "kind", since).await?,
        "by_style": groups(&pool, "style", since).await?,
        "by_rules": groups(&pool, "rules", since).await?,
    })))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};

    async fn user(pool: &SqlitePool, name: &str) -> (i64, String) {
        let user = auth::create_user(pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let token = auth::issue_token(pool, user.id, TokenKind::Session, "Living room iPad")
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
                        (8, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Second', 'Alice', '[]'),
                        (9, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'Third', 'Alice', '[]');"#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn keeps_each_transition_with_how_it_went_and_the_listen_it_led_into() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let (alice, token) = user(&pool, "alice").await;
        let (_, bob_token) = user(&pool, "bob").await;
        let app = routes(pool.clone()).merge(super::super::music_plays::routes(pool.clone()));
        // Alice's phone synced the listen the mix led into: skipped 20 seconds in.
        sqlx::query(
            "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                 payload, created_at, received_at)
             VALUES ('o1', ?, 'phone', 'play_events', 'listen-8', 'upsert',
                 '{\"jellyfinId\":\"8\",\"startedAt\":2000000,\"skipped\":true,\"positionMs\":20000}',
                 1, 1)",
        )
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();

        let mix = json!({ "id": "mix-1", "from_track_id": 7, "to_track_id": 8,
                          "started_at": 1990, "kind": "beatmatch", "seconds": 30.5,
                          "style": "blend", "by_parts": true, "band_semitones": -1,
                          "score": 0.82, "play_id": "listen-8", "device": "Phone" });
        let (status, kept) = call(
            &app,
            &token,
            "POST",
            "/api/v1/music/transitions",
            Some(mix.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{kept}");
        assert_eq!(
            (
                &kept["kind"],
                &kept["seconds"],
                &kept["by_parts"],
                &kept["band_semitones"],
                &kept["voice_semitones"],
                &kept["skipped"],
                &kept["play_id"],
                &kept["listen"]["skipped"],
                &kept["listen"]["position_seconds"],
            ),
            (
                &json!("beatmatch"),
                &json!(30.5),
                &json!(true),
                &json!(-1),
                &json!(0),
                &json!(false),
                &json!("listen-8"),
                &json!(true),
                &json!(20.0),
            )
        );
        // Sent again, skipped during the mix after all: the same transition, changed.
        let mut skipped = mix.clone();
        skipped["skipped"] = json!(true);
        skipped["skipped_at_seconds"] = json!(12.0);
        let (_, again) = call(
            &app,
            &token,
            "POST",
            "/api/v1/music/transitions",
            Some(skipped),
        )
        .await;
        assert_eq!(again["id"], kept["id"]);
        assert_eq!(again["skipped_at_seconds"], json!(12.0));
        // A crossfade without parts, into a listen not sent yet.
        let (status, _) = call(
            &app,
            &token,
            "POST",
            "/api/v1/music/transitions",
            Some(
                json!({ "from_track_id": 8, "to_track_id": 9, "started_at": 2100,
                         "kind": "crossfade", "seconds": 8 }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (_, page) = call(&app, &token, "GET", "/api/v1/music/transitions", None).await;
        assert_eq!(page["total"], 2);
        assert_eq!(page["items"][0]["kind"], "crossfade", "newest first");
        assert_eq!(page["items"][0]["listen"], Value::Null);
        assert_eq!(page["items"][0]["device"], "Living room iPad");
        let (_, beatmatches) = call(
            &app,
            &token,
            "GET",
            "/api/v1/music/transitions?kind=beatmatch",
            None,
        )
        .await;
        assert_eq!(beatmatches["total"], 1);
        // Each user's own.
        let (_, bobs) = call(&app, &bob_token, "GET", "/api/v1/music/transitions", None).await;
        assert_eq!(bobs["total"], 0);

        // Deleting the listen it led into deletes it; a range takes the rest.
        let (_, plays) = call(&app, &token, "GET", "/api/v1/music/plays", None).await;
        let listen = plays["items"][0]["id"].as_i64().unwrap();
        let (status, _) = call(
            &app,
            &token,
            "DELETE",
            &format!("/api/v1/music/plays/{listen}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, page) = call(&app, &token, "GET", "/api/v1/music/transitions", None).await;
        assert_eq!(page["total"], 1);
        let (_, _) = call(
            &app,
            &token,
            "DELETE",
            "/api/v1/music/plays?from=2000&to=2200",
            None,
        )
        .await;
        let (_, page) = call(&app, &token, "GET", "/api/v1/music/transitions", None).await;
        assert_eq!(page["total"], 0);
    }

    #[tokio::test]
    async fn a_transition_that_cant_be_one_is_refused() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let (_, token) = user(&pool, "alice").await;
        let app = routes(pool);
        let good = json!({ "from_track_id": 7, "to_track_id": 8, "started_at": 10,
                           "kind": "ramp", "seconds": 20 });
        let with = |field: &str, value: Value| {
            let mut body = good.clone();
            body[field] = value;
            body
        };
        for (body, want) in [
            (with("to_track_id", json!(99)), StatusCode::NOT_FOUND),
            (with("kind", json!("scratch")), StatusCode::BAD_REQUEST),
            (with("seconds", json!(0)), StatusCode::BAD_REQUEST),
            (with("seconds", json!(601)), StatusCode::BAD_REQUEST),
            (with("started_at", json!(0)), StatusCode::BAD_REQUEST),
            (
                with("started_at", json!(4_000_000_000_i64)),
                StatusCode::BAD_REQUEST,
            ),
            (with("score", json!(1.5)), StatusCode::BAD_REQUEST),
            (with("band_semitones", json!(13)), StatusCode::BAD_REQUEST),
            (
                with("skipped_at_seconds", json!(3)),
                StatusCode::BAD_REQUEST,
            ),
            (with("id", json!(" ")), StatusCode::BAD_REQUEST),
            (with("lanes", json!([])), StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let (status, answer) = call(
                &app,
                &token,
                "POST",
                "/api/v1/music/transitions",
                Some(body.clone()),
            )
            .await;
            assert_eq!(status, want, "{body} → {answer}");
        }
        let (status, _) = call(
            &app,
            &token,
            "GET",
            "/api/v1/music/transitions?kind=scratch",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(
            &app,
            &token,
            "POST",
            "/api/v1/music/transitions",
            Some(good),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn admins_see_the_households_transitions_summed_up() {
        let pool = crate::db::open_in_memory().await;
        library(&pool).await;
        let (_, alice) = user(&pool, "alice").await;
        let (_, bob) = user(&pool, "bob").await;
        let admin = auth::create_user(&pool, "admin", "a-long-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, admin) = auth::issue_token(&pool, admin.id, TokenKind::Session, "Laptop")
            .await
            .unwrap();
        let app = routes(pool.clone());
        let now = now_ms() / 1000;
        // (who, kind, style, rules, skipped, how long ago in days)
        let played = [
            (&alice, "beatmatch", Some("blend"), Some(1), false, 1),
            (&alice, "beatmatch", Some("blend"), Some(1), true, 2),
            (&bob, "beatmatch", Some("cut"), Some(2), false, 3),
            (&bob, "crossfade", None, None, true, 4),
            // Outside the last 30 days.
            (&alice, "throw", None, Some(1), true, 40),
        ];
        for (token, kind, style, rules, skipped, ago) in played {
            let (status, kept) = call(
                &app,
                token,
                "POST",
                "/api/v1/music/transitions",
                Some(json!({ "from_track_id": 7, "to_track_id": 8,
                             "started_at": now - ago * 86_400, "kind": kind, "seconds": 16,
                             "style": style, "rules": rules, "skipped": skipped })),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{kept}");
            assert_eq!(kept["rules"], json!(rules), "kept as sent, or null");
        }

        let (status, _) = call(
            &app,
            &alice,
            "GET",
            "/api/v1/music/transitions/summary",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "admins only");
        let (status, summary) = call(
            &app,
            &admin,
            "GET",
            "/api/v1/music/transitions/summary",
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{summary}");
        assert_eq!(
            (
                &summary["days"],
                &summary["total"],
                &summary["skipped"],
                &summary["skip_rate"]
            ),
            (&json!(30), &json!(4), &json!(2), &json!(0.5))
        );
        assert_eq!(
            summary["by_kind"],
            json!([
                { "value": "beatmatch", "count": 3, "skipped": 1, "skip_rate": 1.0 / 3.0 },
                { "value": "crossfade", "count": 1, "skipped": 1, "skip_rate": 1.0 },
            ])
        );
        assert_eq!(
            summary["by_style"],
            json!([
                { "value": "blend", "count": 2, "skipped": 1, "skip_rate": 0.5 },
                { "value": null, "count": 1, "skipped": 1, "skip_rate": 1.0 },
                { "value": "cut", "count": 1, "skipped": 0, "skip_rate": 0.0 },
            ])
        );
        assert_eq!(
            summary["by_rules"],
            json!([
                { "value": 1, "count": 2, "skipped": 1, "skip_rate": 0.5 },
                { "value": null, "count": 1, "skipped": 1, "skip_rate": 1.0 },
                { "value": 2, "count": 1, "skipped": 0, "skip_rate": 0.0 },
            ])
        );
        // No per-user details: counts only.
        assert!(!summary.to_string().contains("alice"));
        let (_, year) = call(
            &app,
            &admin,
            "GET",
            "/api/v1/music/transitions/summary?days=365",
            None,
        )
        .await;
        assert_eq!(year["total"], 5);
        for bad in ["0", "366", "x"] {
            let (status, _) = call(
                &app,
                &admin,
                "GET",
                &format!("/api/v1/music/transitions/summary?days={bad}"),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "days={bad}");
        }
        let (status, _) = call(
            &app,
            &alice,
            "POST",
            "/api/v1/music/transitions",
            Some(
                json!({ "from_track_id": 7, "to_track_id": 8, "started_at": now,
                         "kind": "ramp", "seconds": 16, "rules": 0 }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a version is 1 or more");
    }
}
