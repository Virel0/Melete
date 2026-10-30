//! A user's playback preferences, kept per user so they follow them to every app: whether the
//! next episode starts by itself, and how long the countdown before it lasts.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::auth::{AppState, CurrentUser, require_user};
use super::{ApiError, bad_request};

/// The shortest and longest countdown before the next episode, in seconds.
const COUNTDOWN: std::ops::RangeInclusive<i64> = 5..=30;

/// A user's playback preferences, as the API gives them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Playback {
    /// The next episode starts by itself when the countdown reaches nothing.
    pub autoplay_next: bool,
    /// How long the countdown lasts.
    pub countdown_seconds: i64,
}

impl Default for Playback {
    fn default() -> Self {
        Playback {
            autoplay_next: true,
            countdown_seconds: 10,
        }
    }
}

/// The routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/me/playback", get(show).put(change))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

/// A user's preferences; the defaults for one who hasn't set any.
pub async fn playback(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Playback> {
    let row: Option<(bool, i64)> = sqlx::query_as(
        "SELECT autoplay_next, countdown_seconds FROM user_playback WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        None => Playback::default(),
        Some((autoplay_next, countdown_seconds)) => Playback {
            autoplay_next,
            countdown_seconds,
        },
    })
}

async fn set_playback(pool: &SqlitePool, user_id: i64, p: &Playback) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO user_playback (user_id, autoplay_next, countdown_seconds, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET autoplay_next = excluded.autoplay_next,
             countdown_seconds = excluded.countdown_seconds, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(p.autoplay_next)
    .bind(p.countdown_seconds)
    .bind(crate::db::unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

async fn show(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Playback>, ApiError> {
    Ok(Json(playback(&pool, current.user.id).await?))
}

/// What a change sends: each field left out keeps its value, so an app that doesn't know a
/// newer preference never resets it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    autoplay_next: Option<bool>,
    countdown_seconds: Option<i64>,
}

async fn change(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Change>,
) -> Result<Json<Playback>, ApiError> {
    let mut kept = playback(&pool, current.user.id).await?;
    if let Some(on) = body.autoplay_next {
        kept.autoplay_next = on;
    }
    if let Some(seconds) = body.countdown_seconds {
        if !COUNTDOWN.contains(&seconds) {
            return Err(bad_request(&format!(
                "countdown_seconds is from {} to {}",
                COUNTDOWN.start(),
                COUNTDOWN.end()
            )));
        }
        kept.countdown_seconds = seconds;
    }
    set_playback(&pool, current.user.id, &kept).await?;
    Ok(Json(kept))
}

#[cfg(test)]
mod tests;
