//! A user's audio and subtitle languages, the tracks they choose by hand in a series, and the
//! tracks Melete picks to play a file with, so every app picks the same.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Extension, Json, Router, middleware};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::auth::{AppState, CurrentUser, require_playback, require_user};
use super::playback::resolve;
use super::{ApiError, bad_request};
use crate::languages::Language;
use crate::probe::MediaInfo;
use crate::track_picks::{self, Pick, Preferences, SeriesChoice, SubtitleChoice, SubtitleMode};

/// Most languages kept in each list.
const MAX_LANGUAGES: usize = 10;

/// The routes, for everyone signed in.
pub fn routes(pool: SqlitePool) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    // A choice is reported while playing, so a key for playback may send it.
    let playing = Router::new()
        .route("/api/v1/files/{id}/track-choice", put(choose))
        .route_layer(middleware::from_fn_with_state(
            auth.clone(),
            require_playback,
        ));
    Router::new()
        .route("/api/v1/me/languages", get(show).put(replace))
        .route_layer(middleware::from_fn_with_state(auth, require_user))
        .merge(playing)
        .with_state(pool)
}

fn preferences_json(p: &Preferences) -> Value {
    let codes = |l: &[Language]| l.iter().map(Language::code).collect::<Vec<_>>();
    json!({
        "audio_languages": codes(&p.audio),
        "subtitle_languages": codes(&p.subtitles),
        "subtitle_mode": p.mode.name(),
    })
}

async fn show(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Value>, ApiError> {
    let preferences = track_picks::preferences(&pool, current.user.id).await?;
    Ok(Json(preferences_json(&preferences)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguagesBody {
    audio_languages: Vec<String>,
    subtitle_languages: Vec<String>,
    subtitle_mode: String,
}

/// Language codes as a list, in order and each once; a 400 naming any that isn't one.
fn language_list(codes: &[String], what: &str) -> Result<Vec<Language>, ApiError> {
    let mut list: Vec<Language> = Vec::new();
    for code in codes {
        let language = Language::parse(code).ok_or_else(|| {
            bad_request(&format!(
                "“{code}” isn't a language code; use ISO 639 codes such as “en”, “jpn” or “pt-BR”"
            ))
        })?;
        if !list.contains(&language) {
            list.push(language);
        }
    }
    if list.len() > MAX_LANGUAGES {
        return Err(bad_request(&format!(
            "list at most {MAX_LANGUAGES} {what} languages"
        )));
    }
    Ok(list)
}

async fn replace(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Json(body): Json<LanguagesBody>,
) -> Result<Json<Value>, ApiError> {
    let mode = SubtitleMode::parse(&body.subtitle_mode).ok_or_else(|| {
        bad_request(&format!(
            "subtitle_mode is one of {}",
            SubtitleMode::NAMES.join(", ")
        ))
    })?;
    let preferences = Preferences {
        audio: language_list(&body.audio_languages, "audio")?,
        subtitles: language_list(&body.subtitle_languages, "subtitle")?,
        mode,
    };
    track_picks::set_preferences(&pool, current.user.id, &preferences).await?;
    Ok(Json(preferences_json(&preferences)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubtitleBody {
    language: Option<String>,
    #[serde(default)]
    forced: bool,
    #[serde(default)]
    hearing_impaired: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceBody {
    audio_language: Option<String>,
    subtitle: Option<SubtitleBody>,
}

/// A track's language as a choice keeps it: none for an untagged track, a 400 for a code that
/// isn't one.
fn chosen_language(code: Option<&str>) -> Result<Option<Language>, ApiError> {
    match code {
        None => Ok(None),
        Some(code) => Language::parse(code)
            .map(Some)
            .ok_or_else(|| bad_request(&format!("“{code}” isn't a language code"))),
    }
}

/// Keeps the tracks the user chose by hand while playing an episode, for its series.
async fn choose(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Path(file_id): Path<i64>,
    Json(body): Json<ChoiceBody>,
) -> Result<StatusCode, ApiError> {
    super::playback::require_file(&pool, &current.access, file_id).await?;
    let series: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT series_id FROM media_files WHERE id = ? AND missing_since IS NULL",
    )
    .bind(file_id)
    .fetch_optional(&pool)
    .await?;
    let Some(series) = series else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such file"));
    };
    let Some(series_id) = series else {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "choices are kept per series, and this file isn't an episode",
        ));
    };
    let subtitles = match body.subtitle {
        None => Some(SubtitleChoice::Off),
        Some(s) => chosen_language(s.language.as_deref())?.map(|language| SubtitleChoice::On {
            language,
            forced: s.forced,
            hearing_impaired: s.hearing_impaired,
        }),
    };
    let choice = SeriesChoice {
        audio: chosen_language(body.audio_language.as_deref())?,
        subtitles,
    };
    track_picks::set_series_choice(&pool, current.user.id, series_id, &choice).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The tracks to play a file with for a user, from its stored probe; None until it's been
/// probed as it is now (asking for the file's info probes it), so playing never waits on it.
pub(super) async fn picked(
    pool: &SqlitePool,
    user_id: i64,
    file_id: i64,
) -> Result<Option<Pick>, ApiError> {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT l.path, f.rel_path, p.info FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         JOIN media_probes p ON p.file_id = f.id
         WHERE f.id = ? AND p.size = f.size AND p.modified_at = f.modified_at",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let Some((root, rel_path, info)) = row else {
        return Ok(None);
    };
    let media: Option<MediaInfo> = info.as_deref().map(serde_json::from_str).transpose()?;
    // Without the folder, the subtitles beside the file are left out rather than the pick.
    let sidecars = match resolve(&root, &rel_path).await {
        Ok(video) => super::subtitles::sidecars_of(video)
            .await
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let pick = track_picks::for_file(pool, user_id, file_id, media.as_ref(), &sidecars).await?;
    Ok(Some(pick))
}

#[cfg(test)]
mod tests;
