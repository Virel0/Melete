//! A smart playlist's songs, worked out here for whoever asks (`aoide::smart`): the rules its
//! row carries, run against the library and the listener's own likes, flags and listens, so
//! the web and the Aoide apps get the same songs.

use axum::extract::Query;
use serde::Deserialize;

use super::*;
use crate::aoide::playlist_edits::is_smart;
use crate::aoide::smart::{self, EvaluateError, Listener, Rules};
use crate::db::now_ms;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SongsQuery {
    /// The listener's offset from UTC in minutes, for spans of months and years.
    offset_minutes: Option<i64>,
}

/// The rule set a smart playlist's row carries, or why there's none to run.
pub(super) fn rules_of(row: &playlists::Row) -> Result<Rules, ApiError> {
    let conflict = |message: String| ApiError::new(StatusCode::CONFLICT, message);
    if !is_smart(row) {
        return Err(conflict(
            "it isn't a smart playlist; a hand-made playlist's songs are its items".into(),
        ));
    }
    match row.get("smartRules") {
        None | Some(Value::Null) => Err(conflict("the smart playlist has no rules".into())),
        Some(Value::String(text)) if text.trim().is_empty() => {
            Err(conflict("the smart playlist has no rules".into()))
        }
        Some(rules) => Rules::from_json(rules)
            .map_err(|err| conflict(format!("its rules can't be worked out: {err}"))),
    }
}

/// The songs `rules` give the signed-in listener now, as the music API shows them, in order.
pub(super) async fn songs_for(
    pool: &SqlitePool,
    current: &CurrentUser,
    rules: &Rules,
    offset_minutes: i64,
) -> Result<Vec<Value>, ApiError> {
    let listener = Listener {
        user_id: current.user.id,
        offset_minutes,
    };
    let ids = smart::evaluate(pool, rules, now_ms(), listener)
        .await
        .map_err(|err| match err {
            EvaluateError::Rules(err) => ApiError::new(
                StatusCode::CONFLICT,
                format!("its rules can't be worked out: {err}"),
            ),
            EvaluateError::Database(err) => err.into(),
        })?;
    let tracks =
        super::super::music::tracks_by_id(pool, &current.access, current.user.id, &ids).await?;
    Ok(ids
        .iter()
        .filter_map(|id| tracks.get(id).cloned())
        .collect())
}

/// An offset from UTC in minutes, as given: none is UTC.
pub(super) fn offset(given: Option<i64>) -> Result<i64, ApiError> {
    let offset = given.unwrap_or(0);
    if !(-840..=840).contains(&offset) {
        return Err(super::super::bad_request(
            "offset_minutes must be from -840 to 840 (minutes from UTC)",
        ));
    }
    Ok(offset)
}

/// `GET /playlists/{id}/songs`: a smart playlist's songs for the listener, with its rules.
pub(super) async fn songs(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    Query(query): Query<SongsQuery>,
) -> Result<Json<Value>, ApiError> {
    let offset = offset(query.offset_minutes)?;
    let Some(playlist) = playlists::visible(&pool, current.user.id)
        .await?
        .into_iter()
        .find(|p| p.id == id)
    else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such playlist"));
    };
    let rules = rules_of(&playlist.row)?;
    let tracks = songs_for(&pool, &current, &rules, offset).await?;
    Ok(Json(json!({
        "id": playlist.id,
        "rules": rules.to_json(),
        "tracks": tracks,
    })))
}

/// Rules as a request gives them, checked as the apps check them; a sort on genre, which
/// can't be run, is refused here too.
pub(super) fn rules_given(given: &Value) -> Result<Rules, ApiError> {
    let rules = Rules::from_json(given)
        .map_err(|err| super::super::bad_request(&format!("rules: {err}")))?;
    if rules.sort.is_some_and(|s| s.field == smart::Field::Genre) {
        return Err(super::super::bad_request(
            "rules: genre has no single value to sort by",
        ));
    }
    Ok(rules)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Trial {
    rules: Value,
    offset_minutes: Option<i64>,
}

/// How many songs a preview lists; its count is of them all.
const PREVIEW: usize = 100;

/// `POST /smart-playlists/songs`: what rules give the listener now, before they're kept: how
/// many songs, and the first of them in order.
pub(super) async fn preview(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<Trial>,
) -> Result<Json<Value>, ApiError> {
    let offset = offset(body.offset_minutes)?;
    let rules = rules_given(&body.rules)?;
    let mut tracks = songs_for(&pool, &current, &rules, offset).await?;
    let count = tracks.len();
    tracks.truncate(PREVIEW);
    Ok(Json(json!({
        "rules": rules.to_json(),
        "count": count,
        "tracks": tracks,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewRules {
    rules: Value,
}

/// `PUT /playlists/{id}/rules`: new rules for one's own smart playlist, written into the sync
/// log as a device of the owner's writes them.
pub(super) async fn set_rules(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<String>,
    Json(body): Json<NewRules>,
) -> Result<Json<Value>, ApiError> {
    let rules = rules_given(&body.rules)?;
    let device = crate::aoide::writing::session_device(current.token_id);
    let editor = crate::aoide::playlist_edits::Editor {
        user_id: current.user.id,
        device: &device,
        now: now_ms(),
    };
    crate::aoide::playlist_edits::set_rules(&pool, &editor, &id, &rules)
        .await
        .map_err(super::edits::failed)?;
    Ok(Json(
        detail(&pool, &current.access, current.user.id, &id).await?,
    ))
}

#[cfg(test)]
mod tests;
