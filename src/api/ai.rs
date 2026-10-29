//! AI features (`crate::ai`), for everyone signed in, off unless OpenRouter is set up.
//! The first: finding movies or series in the library by describing them. Only the words
//! typed go to OpenRouter; the model answers with titles it knows, and Melete looks for those
//! in the library itself, so nothing about the library leaves the server.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::media::{add_list_fields, artwork_url};
use crate::ai::{self, Client, Failure, Suggestion, TitleKind};
use crate::search::comparable;

/// Each search costs money on the household's account, so each user gets this many an hour.
const SEARCHES_PER_HOUR: usize = 30;

/// When each user's searches in the last hour were made.
pub type Searches = Mutex<HashMap<i64, VecDeque<Instant>>>;

#[derive(Clone)]
struct AiState {
    pool: SqlitePool,
    client: Option<Arc<Client>>,
    recent: Arc<Searches>,
}

/// The AI routes with a count of their own, for tests.
#[cfg(test)]
pub fn routes(pool: SqlitePool, client: Option<Arc<Client>>) -> Router {
    routes_with(pool, client, Arc::default())
}

/// The AI routes, counting searches in `recent`, which outlasts the API being set up afresh.
pub fn routes_with(pool: SqlitePool, client: Option<Arc<Client>>, recent: Arc<Searches>) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/ai", get(about))
        .route("/api/v1/ai/search", post(search))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(AiState {
            pool,
            client,
            recent,
        })
}

/// Whether the apps should offer AI features.
async fn about(State(state): State<AiState>) -> Json<Value> {
    Json(json!({ "enabled": state.client.is_some() }))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Movies,
    Series,
}

#[derive(Deserialize)]
struct Search {
    kind: Kind,
    /// What someone is in the mood for, in their own words.
    query: String,
}

/// Whether `user` may search now; counts the search when they may.
fn allowed(recent: &Searches, user: i64, now: Instant) -> bool {
    let hour = Duration::from_secs(3600);
    let mut recent = recent.lock().expect("no holder panics");
    recent.retain(|_, times| times.back().is_some_and(|t| now.duration_since(*t) < hour));
    let times = recent.entry(user).or_default();
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) >= hour)
    {
        times.pop_front();
    }
    if times.len() >= SEARCHES_PER_HOUR {
        return false;
    }
    times.push_back(now);
    true
}

/// Movies or series in the library that fit a description: the titles the model suggests
/// that are here, best fit first, and every title it suggested, with its id here or null.
async fn search(
    State(state): State<AiState>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Search>,
) -> Result<Json<Value>, ApiError> {
    let Some(client) = &state.client else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "AI features aren't set up; an admin can add an OpenRouter key under Settings",
        ));
    };
    let query = body.query.trim();
    if query.is_empty() || query.chars().count() > ai::MAX_DESCRIPTION {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "describe what you're after in 1 to {} characters",
                ai::MAX_DESCRIPTION
            ),
        ));
    }
    if !allowed(&state.recent, current.user.id, Instant::now()) {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("that's {SEARCHES_PER_HOUR} searches this hour; try again later"),
        ));
    }
    let kind = match body.kind {
        Kind::Movies => TitleKind::Movie,
        Kind::Series => TitleKind::Series,
    };
    let suggestions = client.suggest(kind, query).await.map_err(|failure| {
        let status = match failure {
            Failure::Busy => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::BAD_GATEWAY,
        };
        ApiError::new(status, failure.to_string())
    })?;
    let library = present(&state.pool, &current.access, kind).await?;
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut suggested = Vec::new();
    for suggestion in &suggestions {
        let here: Vec<&Present> = library.iter().filter(|p| fits(p, suggestion)).collect();
        suggested.push(json!({
            "title": suggestion.title,
            "year": suggestion.year,
            "id": here.first().map(|p| p.id),
        }));
        for p in here {
            if seen.insert(p.id) {
                results.push(json!({
                    "id": p.id,
                    "title": p.title,
                    "year": p.year,
                    "poster": artwork_url("w342", p.poster_path.clone()),
                    "matched": p.match_status.as_deref() == Some("matched"),
                }));
            }
        }
    }
    add_list_fields(
        &state.pool,
        current.user.id,
        matches!(kind, TitleKind::Series),
        &mut results,
    )
    .await?;
    Ok(Json(json!({ "results": results, "suggested": suggested })))
}

#[derive(sqlx::FromRow)]
struct Present {
    id: i64,
    title: String,
    original_title: Option<String>,
    year: Option<i64>,
    poster_path: Option<String>,
    match_status: Option<String>,
}

/// Every title of the kind in the library now that the user may see, with what it's called.
async fn present(
    pool: &SqlitePool,
    access: &crate::access::Access,
    kind: TitleKind,
) -> sqlx::Result<Vec<Present>> {
    let sql = match kind {
        TitleKind::Movie => format!(
            "SELECT x.id, COALESCE(t.title, x.parsed_title) AS title, t.original_title,
                 COALESCE(t.year, x.parsed_year) AS year, t.poster_path, x.match_status
             FROM movies x LEFT JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.missing_since IS NULL AND {} ORDER BY x.id",
            access.movie_sql("x")
        ),
        TitleKind::Series => format!(
            "SELECT x.id, COALESCE(t.name, x.parsed_title) AS title,
                 t.original_name AS original_title, COALESCE(t.year, x.parsed_year) AS year,
                 t.poster_path, x.match_status
             FROM series x LEFT JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.missing_since IS NULL AND {} ORDER BY x.id",
            access.series_sql("x")
        ),
    };
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await
}

/// Whether a title here is the one suggested: the same title or original title (case,
/// accents, punctuation and a leading article aside), and a year at most one off when both
/// are known.
fn fits(present: &Present, suggestion: &Suggestion) -> bool {
    let wanted = comparable(&suggestion.title);
    let named = comparable(&present.title) == wanted
        || present
            .original_title
            .as_deref()
            .is_some_and(|o| comparable(o) == wanted);
    let year = match (present.year, suggestion.year) {
        (Some(a), Some(b)) => (a - b).abs() <= 1,
        _ => true,
    };
    named && year
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::ai::fake;

    async fn setup(client: Option<Arc<Client>>) -> (SqlitePool, Router, String) {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "alice", "alice-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (_, token) =
            crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
                .await
                .unwrap();
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
               INSERT INTO tmdb_movies (tmdb_id, title, original_title, year, genres, fetched_at,
                   poster_path) VALUES
                   (949, 'Heat', 'Heat', 1995, '[]', 0, '/heat.jpg'),
                   (1, 'Heat', 'Heat', 1986, '[]', 0, NULL),
                   (2, 'Amélie', 'Le Fabuleux Destin d''Amélie Poulain', 2001, '[]', 0, NULL);
               INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year,
                   seen_generation, match_status, match_tmdb_id, missing_since) VALUES
                   (1, 1, 'Heat (1995)', 'Heat', 1995, 1, 'matched', 949, NULL),
                   (2, 1, 'Heat (1986)', 'Heat', 1986, 1, 'matched', 1, NULL),
                   (3, 1, 'Amelie', 'Amelie', 2001, 1, 'matched', 2, NULL),
                   (4, 1, 'Ronin', 'Ronin', 1998, 1, 'unmatched', NULL, NULL),
                   (5, 1, 'Collateral', 'Collateral', 2004, 1, 'unmatched', NULL, 100);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool.clone(), client);
        (pool, app, token)
    }

    async fn post(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::post("/api/v1/ai/search")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
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
    async fn finds_what_the_model_suggests_in_the_library() {
        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() = r#"{"titles": [
            {"title": "Heat", "year": 1995},
            {"title": "The Fabulous Destiny of Amélie Poulain", "year": 2001},
            {"title": "Le Fabuleux Destin d'Amelie Poulain", "year": 2001},
            {"title": "Ronin", "year": 1998},
            {"title": "Collateral", "year": 2004},
            {"title": "Thief", "year": 1981}
        ]}"#
        .into();
        let (_, app, token) = setup(Some(Arc::new(fake::client(&api)))).await;
        let (status, body) = post(
            &app,
            &token,
            json!({ "kind": "movies", "query": "slow-burn crime films" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let ids: Vec<i64> = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect();
        // Heat (1995) not 1986's; Amélie by its original title; Ronin by its folder; not a
        // title gone from the library.
        assert_eq!(ids, [1, 3, 4]);
        assert_eq!(
            body["results"][0],
            json!({ "id": 1, "title": "Heat", "year": 1995,
                    "poster": "/api/v1/artwork/w342/heat.jpg", "matched": true,
                    "added_at": null, "unwatched_episodes": null })
        );
        let suggested: Vec<Value> = body["suggested"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].clone())
            .collect();
        assert_eq!(
            suggested,
            [
                json!(1),
                Value::Null,
                json!(3),
                json!(4),
                Value::Null,
                Value::Null
            ]
        );
        let sent = fake.seen.lock().unwrap()[0].0.to_string();
        assert!(
            !sent.contains("Ronin") && !sent.contains("alice"),
            "nothing about the library or who asked leaves: {sent}"
        );
    }

    #[tokio::test]
    async fn off_without_a_key_and_limited_with_one() {
        let (_, app, token) = setup(None).await;
        let (status, _) = post(&app, &token, json!({ "kind": "series", "query": "x" })).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        let (fake, api) = fake::start().await;
        *fake.content.lock().unwrap() = r#"{"titles": []}"#.into();
        let (_, app, token) = setup(Some(Arc::new(fake::client(&api)))).await;
        for query in ["", "   ", &"x".repeat(301)] {
            let (status, _) = post(&app, &token, json!({ "kind": "movies", "query": query })).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        for _ in 0..SEARCHES_PER_HOUR {
            let (status, _) = post(&app, &token, json!({ "kind": "series", "query": "x" })).await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, _) = post(&app, &token, json!({ "kind": "series", "query": "x" })).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        *fake.status.lock().unwrap() = 402;
        let (_, app, token) = setup(Some(Arc::new(fake::client(&api)))).await;
        let (status, body) = post(&app, &token, json!({ "kind": "movies", "query": "x" })).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body["error"], "the OpenRouter account is out of credit");
    }

    #[test]
    fn an_hour_frees_a_users_searches_and_users_dont_share() {
        let recent = Mutex::default();
        let t0 = Instant::now();
        for _ in 0..SEARCHES_PER_HOUR {
            assert!(allowed(&recent, 1, t0));
        }
        assert!(!allowed(&recent, 1, t0));
        assert!(allowed(&recent, 2, t0));
        assert!(allowed(&recent, 1, t0 + Duration::from_secs(3600)));
    }
}
