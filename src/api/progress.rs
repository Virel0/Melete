//! How wanted titles are getting on (admins only): what's on the move, one wanted item's
//! step, and the same list as a live event stream (Server-Sent Events) that sends it again
//! whenever it changes. Each user follows their own requests the same way.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use futures_util::Stream;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin};
use crate::progress::{self, Progress};

/// How often the stream looks for changes.
const EVERY: Duration = Duration::from_secs(2);
/// A stream ends after this long; the browser reconnects at once, signing in afresh.
const LIFETIME: Duration = Duration::from_secs(10 * 60);

/// The progress routes, for admins only.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/progress", get(list))
        .route("/api/v1/progress/events", get(events))
        .route("/api/v1/progress/{kind}/{id}", get(one))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(pool)
}

async fn list(State(pool): State<SqlitePool>) -> Result<Json<Vec<Progress>>, ApiError> {
    Ok(Json(progress::list(&pool).await?))
}

async fn one(
    State(pool): State<SqlitePool>,
    Path((kind, id)): Path<(String, i64)>,
) -> Result<Json<Progress>, ApiError> {
    let Some(kind) = progress::kind(&kind) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "the kind is movies, series, albums or songs",
        ));
    };
    progress::of(&pool, kind, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "it isn't wanted"))
}

/// Whether the token a stream was opened with still signs someone in (an admin, for `admin`).
async fn still_allowed(pool: &SqlitePool, token_id: i64, admin: bool) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tokens t JOIN users u ON u.id = t.user_id
                        WHERE t.id = ? AND (? = 0 OR u.is_admin = 1))",
    )
    .bind(token_id)
    .bind(admin)
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

struct Watch<F> {
    pool: SqlitePool,
    token_id: i64,
    admin: bool,
    load: F,
    /// What was last sent of each kind of event.
    sent: std::collections::HashMap<&'static str, String>,
    /// Changes found at one look, sent one after another.
    pending: std::collections::VecDeque<Event>,
    until: tokio::time::Instant,
    first: bool,
}

/// What `load` gives, as `name` events: once at the start, then whenever it changes. It ends
/// when the session does (or stops being an admin's, for `admin`), or after [`LIFETIME`].
pub(super) fn stream<F, Fut, T>(
    pool: SqlitePool,
    current: &CurrentUser,
    admin: bool,
    name: &'static str,
    load: F,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>> + use<F, Fut, T>>
where
    F: Fn(SqlitePool) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<T>> + Send,
    T: serde::Serialize,
{
    let load = std::sync::Arc::new(load);
    stream_of(pool, current, admin, move |pool| {
        let load = load.clone();
        async move { Ok(vec![(name, serde_json::to_string(&load(pool).await?)?)]) }
    })
}

/// Several kinds of event in one stream: `load` gives each kind's name and its data as JSON,
/// and a kind is sent at the start and then whenever its data changes. A kind `load` leaves
/// out is simply not sent. Ends as [`stream`] does.
pub(super) fn stream_of<F, Fut>(
    pool: SqlitePool,
    current: &CurrentUser,
    admin: bool,
    load: F,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>> + use<F, Fut>>
where
    F: Fn(SqlitePool) -> Fut + Send + 'static,
    Fut: Future<Output = anyhow::Result<Vec<(&'static str, String)>>> + Send,
{
    let watch = Watch {
        pool,
        token_id: current.token_id,
        admin,
        load,
        sent: std::collections::HashMap::new(),
        pending: std::collections::VecDeque::new(),
        until: tokio::time::Instant::now() + LIFETIME,
        first: true,
    };
    let stream = futures_util::stream::unfold(watch, move |mut watch| async move {
        loop {
            if let Some(event) = watch.pending.pop_front() {
                return Some((Ok(event), watch));
            }
            if !watch.first {
                tokio::time::sleep(EVERY).await;
            }
            watch.first = false;
            if tokio::time::Instant::now() >= watch.until
                || !still_allowed(&watch.pool, watch.token_id, watch.admin).await
            {
                return None;
            }
            let Ok(kinds) = (watch.load)(watch.pool.clone()).await else {
                continue;
            };
            for (name, json) in kinds {
                if watch.sent.get(name) != Some(&json) {
                    watch
                        .pending
                        .push_back(Event::default().event(name).data(json.clone()));
                    watch.sent.insert(name, json);
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The activity list as `progress` events.
async fn events(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    stream(pool, &current, true, "progress", |pool| async move {
        progress::list(&pool).await
    })
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
        let user = auth::create_user(pool, name, "a-long-password".into(), admin)
            .await
            .unwrap()
            .unwrap();
        auth::issue_token(pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap()
            .1
    }

    fn get(uri: &str, token: &str) -> Request<Body> {
        Request::get(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn lists_progress_and_one_items_step_for_admins() {
        let pool = db::open_in_memory().await;
        let admin = token(&pool, "admin", true).await;
        let someone = token(&pool, "someone", false).await;
        sqlx::raw_sql(
            "INSERT INTO wanted_songs (id, title, artist, source, added_at) VALUES (2, 'Airbag', 'Radiohead', 'manual', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let app = routes(pool);
        let send = |request: Request<Body>| async {
            let response = app.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        };
        assert_eq!(
            send(get("/api/v1/progress", &admin)).await,
            (StatusCode::OK, serde_json::json!([]))
        );
        let (status, song) = send(get("/api/v1/progress/songs/2", &admin)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            (&song["step"], &song["message"]),
            (
                &serde_json::json!("wanted"),
                &serde_json::json!("waiting for a search")
            )
        );
        assert_eq!(
            send(get("/api/v1/progress/songs/9", &admin)).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(get("/api/v1/progress/artists/1", &admin)).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(get("/api/v1/progress", &someone)).await.0,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn the_stream_sends_the_list_and_ends_with_the_session() {
        let pool = db::open_in_memory().await;
        let admin = token(&pool, "admin", true).await;
        let response = routes(pool.clone())
            .oneshot(get("/api/v1/progress/events", &admin))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let mut body = response.into_body();
        let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
        assert_eq!(
            std::str::from_utf8(&first).unwrap(),
            "event: progress\ndata: []\n\n"
        );

        // Signed out: the token is gone, and the stream stops at its next look.
        sqlx::query("DELETE FROM tokens")
            .execute(&pool)
            .await
            .unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = body.frame().await {
                frame.unwrap();
            }
        })
        .await;
        assert!(ended.is_ok(), "the stream outlived the session");
    }
}
