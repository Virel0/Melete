//! System status (admins only): free disk space where Melete keeps things, the background
//! tasks with running one now, and the database's backups.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use serde::Serialize;
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, require_admin};
use crate::tasks::{self, NotRun, TaskView};
use crate::{backup, disk};

#[derive(Clone)]
struct SystemState {
    pool: SqlitePool,
    data_dir: PathBuf,
    transcode_dir: PathBuf,
}

/// The system status routes, for admins only.
pub fn routes(pool: SqlitePool, data_dir: PathBuf, transcode_dir: PathBuf) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/admin/system/disk", get(disk_space))
        .route("/api/v1/admin/system/tasks", get(list_tasks))
        .route("/api/v1/admin/system/tasks/{name}/run", post(run_task))
        .route("/api/v1/admin/system/backups", get(backups))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(SystemState {
            pool,
            data_dir,
            transcode_dir,
        })
}

/// One place Melete keeps things, and the room on its file system.
#[derive(Serialize)]
struct Place {
    /// library, data or transcode.
    kind: &'static str,
    /// The library's name; "data" or "transcode" for the others.
    name: String,
    /// The folder, as Melete sees it.
    path: String,
    /// The file system's size in bytes; null when it can't be read.
    total: Option<u64>,
    /// What's free on it.
    free: Option<u64>,
    /// Whether less than 5% is free.
    nearly_full: bool,
    /// Why it couldn't be read, when it couldn't.
    error: Option<String>,
}

fn place(kind: &'static str, name: String, path: PathBuf) -> Place {
    let measured = disk::space(&path);
    Place {
        kind,
        name,
        path: path.display().to_string(),
        total: measured.as_ref().ok().map(|s| s.total),
        free: measured.as_ref().ok().map(|s| s.free),
        nearly_full: measured.as_ref().is_ok_and(|s| s.nearly_full()),
        error: measured.err().map(|err| err.to_string()),
    }
}

async fn disk_space(State(state): State<SystemState>) -> Result<Json<Vec<Place>>, ApiError> {
    let libraries: Vec<(String, String)> =
        sqlx::query_as("SELECT name, path FROM libraries ORDER BY name")
            .fetch_all(&state.pool)
            .await?;
    let mut places: Vec<Place> = libraries
        .into_iter()
        .map(|(name, path)| place("library", name, path.into()))
        .collect();
    places.push(place("data", "data".into(), state.data_dir.clone()));
    places.push(place(
        "transcode",
        "transcode".into(),
        state.transcode_dir.clone(),
    ));
    Ok(Json(places))
}

async fn list_tasks() -> Json<Vec<TaskView>> {
    Json(tasks::list())
}

async fn run_task(Path(name): Path<String>) -> Result<StatusCode, ApiError> {
    match tasks::run_now(&name) {
        Ok(()) => Ok(StatusCode::ACCEPTED),
        Err(NotRun::NotFound) => Err(ApiError::new(StatusCode::NOT_FOUND, "no such task")),
        Err(NotRun::NotByHand) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "this task isn't run from here; see what it says for how",
        )),
        Err(NotRun::Inactive) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "this task doesn't run in this server: its service is off",
        )),
        Err(NotRun::Running) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "it's running now; it isn't started twice at once",
        )),
    }
}

/// The database's copies and how the latest tries went.
#[derive(Serialize)]
struct Backups {
    /// The copies in the backups folder, newest first.
    copies: Vec<backup::Listed>,
    /// The latest try of each kind: when, and whether it made a copy. Why one failed stays in
    /// the server's log and the admin health, never here.
    latest: Vec<Attempt>,
    /// What's kept of the copies Melete makes on its own.
    kept: Kept,
}

#[derive(Serialize, sqlx::FromRow)]
struct Attempt {
    kind: String,
    made_at: i64,
    ok: bool,
}

#[derive(Serialize)]
struct Kept {
    daily: usize,
    weekly: usize,
    before_migrating: usize,
}

async fn backups(State(state): State<SystemState>) -> Result<Json<Backups>, ApiError> {
    let latest = sqlx::query_as(
        "SELECT kind, made_at, error IS NULL AS ok FROM backups AS b
         WHERE id = (SELECT id FROM backups WHERE kind = b.kind ORDER BY made_at DESC, id DESC
                     LIMIT 1)
         ORDER BY made_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(Backups {
        copies: backup::list(&state.data_dir)?,
        latest,
        kept: Kept {
            daily: backup::DAILY_KEPT,
            weekly: backup::WEEKLY_KEPT,
            before_migrating: backup::BEFORE_MIGRATING_KEPT,
        },
    }))
}

#[cfg(test)]
mod tests;
