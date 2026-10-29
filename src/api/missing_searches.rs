//! Searching for everything missing in an area at once (admins only): starting a run,
//! watching it, and stopping it.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_admin};
use crate::search_missing::{self, Already, Area, Goal, MissingSearch, NotStopped, Sources};

#[derive(Clone)]
struct RunState {
    pool: SqlitePool,
    sources: Sources,
}

/// Starting, watching and stopping searches for what's missing, for admins only.
pub fn routes(pool: SqlitePool, sources: Sources) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/missing-searches", get(latest).post(missing))
        .route("/api/v1/missing-searches/{id}", get(one))
        .route("/api/v1/missing-searches/{id}/stop", post(stop))
        .route("/api/v1/cutoff-searches", post(below_cutoff))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(RunState { pool, sources })
}

#[derive(Deserialize)]
struct Start {
    area: String,
}

const AREAS: &str = "area is movies, series or albums";

/// Starts a run for what's missing in an area.
async fn missing(
    state: State<RunState>,
    current: Extension<CurrentUser>,
    Json(body): Json<Start>,
) -> Result<(StatusCode, Json<MissingSearch>), ApiError> {
    let Some(area) = Area::parse(&body.area) else {
        return Err(ApiError::new(StatusCode::BAD_REQUEST, AREAS));
    };
    start(state, current, area, Goal::Missing).await
}

/// Starts a run for upgrades of what's below its cutoff in an area; music isn't upgraded.
async fn below_cutoff(
    state: State<RunState>,
    current: Extension<CurrentUser>,
    Json(body): Json<Start>,
) -> Result<(StatusCode, Json<MissingSearch>), ApiError> {
    let area = match Area::parse(&body.area) {
        Some(Area::Albums) => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "music isn't upgraded, so no album is below a cutoff",
            ));
        }
        Some(area) => area,
        None => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "area is movies or series",
            ));
        }
    };
    start(state, current, area, Goal::CutoffUnmet).await
}

async fn start(
    State(state): State<RunState>,
    Extension(current): Extension<CurrentUser>,
    area: Area,
    goal: Goal,
) -> Result<(StatusCode, Json<MissingSearch>), ApiError> {
    let (pool, sources) = (&state.pool, state.sources.clone());
    match search_missing::start_for(pool, sources, area, goal, current.user.id).await? {
        Ok(run) => Ok((StatusCode::ACCEPTED, Json(run))),
        Err(Already(id)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "a search in this area is under way already (missing search {id}); stop it or wait for it to finish"
            ),
        )),
    }
}

async fn latest(State(state): State<RunState>) -> Result<Json<Vec<MissingSearch>>, ApiError> {
    Ok(Json(search_missing::latest(&state.pool).await?))
}

async fn one(
    State(state): State<RunState>,
    Path(id): Path<i64>,
) -> Result<Json<MissingSearch>, ApiError> {
    search_missing::get(&state.pool, id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such missing search"))
}

async fn stop(
    State(state): State<RunState>,
    Path(id): Path<i64>,
) -> Result<Json<MissingSearch>, ApiError> {
    match search_missing::stop(&state.pool, id).await? {
        Ok(run) => Ok(Json(run)),
        Err(NotStopped::NotFound) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no such missing search",
        )),
        Err(NotStopped::Finished) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "it has finished already",
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, header};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{self, TokenKind};
    use crate::db;

    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
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

    #[tokio::test]
    async fn admins_start_watch_and_stop_a_search_for_everything_missing() {
        let pool = db::open_in_memory().await;
        let mut tokens = Vec::new();
        for (name, admin) in [("admin", true), ("viewer", false)] {
            let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
                .await
                .unwrap()
                .unwrap();
            tokens.push(
                auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                    .await
                    .unwrap()
                    .1,
            );
        }
        let (admin, viewer) = (tokens[0].clone(), tokens[1].clone());
        // A monitored movie with no profile: searched for, and noted as not searchable.
        sqlx::query(
            "INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at)
             VALUES (1, 'Dune', 1, 'manual', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let sources = Sources {
            tmdb: None,
            slskd: None,
            pipeline: None,
            pace: Duration::ZERO,
        };
        let app = routes(pool, sources);

        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/missing-searches",
            &admin,
            Some(json!({ "area": "songs" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, run) = send(
            &app,
            "POST",
            "/api/v1/missing-searches",
            &admin,
            Some(json!({ "area": "movies" })),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        assert_eq!(run["started_by"], "admin");
        let id = run["id"].as_i64().unwrap();
        let mut run = run;
        for _ in 0..500 {
            if run["state"] != "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            run = send(
                &app,
                "GET",
                &format!("/api/v1/missing-searches/{id}"),
                &admin,
                None,
            )
            .await
            .1;
        }
        assert_eq!(run["state"], "done", "{run}");
        assert_eq!(run["total"], 1);
        assert_eq!(run["searched"], 0);
        assert!(
            run["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n.as_str().unwrap().starts_with("Dune: ")),
            "{run}"
        );
        let (_, latest) = send(&app, "GET", "/api/v1/missing-searches", &admin, None).await;
        assert_eq!(latest.as_array().unwrap().len(), 1);
        let (status, _) = send(
            &app,
            "POST",
            &format!("/api/v1/missing-searches/{id}/stop"),
            &admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "finished already");
        let (status, _) = send(
            &app,
            "POST",
            "/api/v1/missing-searches/99/stop",
            &admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        for (method, uri) in [
            ("GET", "/api/v1/missing-searches"),
            ("POST", "/api/v1/missing-searches"),
            ("POST", "/api/v1/missing-searches/1/stop"),
        ] {
            let (status, _) = send(
                &app,
                method,
                uri,
                &viewer,
                Some(json!({ "area": "movies" })),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        }
    }

    #[tokio::test]
    async fn a_run_for_upgrades_names_its_goal_and_music_has_none() {
        let pool = db::open_in_memory().await;
        let user = auth::create_user(&pool, "admin", "admin-password".into(), true)
            .await
            .unwrap()
            .unwrap();
        let (_, admin) = auth::issue_token(&pool, user.id, TokenKind::Session, "t")
            .await
            .unwrap();
        let sources = Sources {
            tmdb: None,
            slskd: None,
            pipeline: None,
            pace: Duration::ZERO,
        };
        let app = routes(pool, sources);
        let start = |area: &str| {
            let body = json!({ "area": area });
            send(&app, "POST", "/api/v1/cutoff-searches", &admin, Some(body))
        };
        let (status, run) = start("series").await;
        assert_eq!(status, StatusCode::ACCEPTED, "{run}");
        assert_eq!(run["goal"], "cutoff_unmet");
        for refused in ["albums", "songs"] {
            let (status, body) = start(refused).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {body}");
        }
        let (_, latest) = send(&app, "GET", "/api/v1/missing-searches", &admin, None).await;
        assert_eq!(latest[0]["goal"], "cutoff_unmet");
    }
}
