//! For You as the API gives it (`crate::for_you`): each listener's own list, songs to play as
//! the library's other track lists give them and songs to request with what asking for one
//! takes, each with the seed it came from. Only what the listener may see is there, and only
//! their own list is served.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::ApiError;
use super::auth::{AppState, CurrentUser, require_user};
use super::music::tracks_by_id;
use crate::access::Access;
use crate::for_you::{self, Item, Song, State as ListState};
use crate::music_requests::{self, Asked};

/// The For You route, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/music/for-you", get(for_you))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .with_state(pool)
}

#[derive(Deserialize)]
struct ForYouQuery {
    limit: Option<String>,
}

async fn for_you(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ForYouQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = match query.limit.as_deref() {
        None => for_you::ITEMS,
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|l| (1..=100).contains(l))
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "limit is 1 to 100"))?,
    };
    let me = current.user.id;
    let now = crate::db::unix_now();
    let served = for_you::for_listener(&pool, me, &current.access, now).await?;
    let state = match served.state {
        ListState::Off => "off",
        ListState::NoHistory => "no_history",
        ListState::Waiting => "waiting",
        ListState::Ready => "ready",
    };
    let items = items(&pool, me, &current.access, served.items, limit, now).await?;
    Ok(Json(json!({
        "state": state,
        "made_at": served.made_at,
        "items": items,
    })))
}

/// The list's first `limit` songs as the answer gives them: one to play with its track as the
/// library's track lists give it, one to request with its state and request, as discovery
/// marks titles.
async fn items(
    pool: &SqlitePool,
    me: i64,
    access: &Access,
    items: Vec<Item>,
    limit: usize,
    now: i64,
) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<i64> = items.iter().filter_map(|i| i.track_id).collect();
    let tracks = tracks_by_id(pool, access, me, &ids).await?;
    let mut out = Vec::new();
    for item in items {
        if out.len() == limit {
            break;
        }
        let because = serde_json::to_value(&item.because).map_err(anyhow::Error::from)?;
        match (item.track_id, item.song) {
            (Some(id), _) => {
                let Some(track) = tracks.get(&id) else {
                    continue;
                };
                out.push(json!({
                    "state": "playable", "track": track, "song": null, "request_id": null,
                    "because": because,
                }));
            }
            (None, Some(song)) => {
                let (state, request_id) = asked(pool, me, &song).await?;
                let group = song.release_group.as_deref();
                let poster = crate::search_covers::offer(pool, group, me, now).await?;
                out.push(json!({
                    "state": state, "track": null, "request_id": request_id, "because": because,
                    "song": {
                        "kind": "song", "mb_recording_id": song.mb_recording_id,
                        "title": song.title, "artist": song.artist,
                        "mb_artist_ids": song.mb_artist_ids, "release": song.release,
                        "poster": poster,
                    },
                }));
            }
            (None, None) => {}
        }
    }
    Ok(out)
}

/// Whether a song to request is asked for already, as request search says: `requested` when
/// it's wanted, or `me` asked for it and it wasn't declined (with that request), else
/// `requestable`.
async fn asked(
    pool: &SqlitePool,
    me: i64,
    song: &Song,
) -> Result<(&'static str, Option<i64>), ApiError> {
    let wanted = music_requests::wanted_already(
        pool,
        &Asked {
            kind: "song".into(),
            title: song.title.clone(),
            artist: Some(song.artist.clone()),
            mb_artist_id: song.mb_artist_ids.first().cloned(),
            mb_release_group_id: None,
            mb_recording_id: Some(song.mb_recording_id.clone()),
        },
    )
    .await?
    .is_some();
    let request_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM requests WHERE user_id = ? AND kind = 'song' AND state != 'declined'
           AND mb_recording_id = ?
         ORDER BY id DESC LIMIT 1",
    )
    .bind(me)
    .bind(&song.mb_recording_id)
    .fetch_optional(pool)
    .await?;
    let state = if wanted || request_id.is_some() {
        "requested"
    } else {
        "requestable"
    };
    Ok((state, request_id))
}

#[cfg(test)]
mod tests;
