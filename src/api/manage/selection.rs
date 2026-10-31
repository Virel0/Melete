//! Changing a managed list's selected titles in one request (admins only):
//! `POST /api/v1/manage/{service}/selection` monitors them or stops, sets their profile, takes
//! them off the wanted list, or puts those only in the library on it. At most 100 titles a
//! request; a larger selection is sent in batches.

use axum::Extension;
use axum::extract::Path;
use axum::routing::post;

use super::*;
use crate::api::auth::CurrentUser;
use crate::manage::selection::{self, Action, Lookups, Outcome, Service};
use crate::{musicbrainz, tmdb};

#[derive(Clone)]
struct SelectionState {
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
}

/// The selection route, for admins only. TMDB and MusicBrainz say which title a folder only in
/// the library is, as an add does.
pub fn selection_routes(
    pool: SqlitePool,
    tmdb: Option<Arc<tmdb::Client>>,
    musicbrainz: Option<Arc<musicbrainz::Client>>,
) -> Router {
    let auth = AppState {
        pool: pool.clone(),
        limiter: Arc::default(),
    };
    Router::new()
        .route("/api/v1/manage/{service}/selection", post(change))
        .route_layer(middleware::from_fn_with_state(auth, require_admin))
        .with_state(SelectionState {
            pool,
            tmdb,
            musicbrainz,
        })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    /// monitor, unmonitor, profile or remove.
    action: String,
    /// Titles on the wanted list.
    #[serde(default)]
    wanted_ids: Vec<i64>,
    /// Titles only in the library, by folder: monitor only.
    #[serde(default)]
    folders: Vec<i64>,
    /// For profile, the one to set; for monitor, the one folders are wanted with.
    profile_id: Option<i64>,
}

fn bad(why: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, why.into())
}

async fn change(
    State(state): State<SelectionState>,
    Extension(current): Extension<CurrentUser>,
    Path(service): Path<String>,
    Json(asked): Json<Selection>,
) -> Result<Json<Outcome>, ApiError> {
    let Some(service) = Service::parse(&service) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "the services are movies, series, anime and artists",
        ));
    };
    let count = asked.wanted_ids.len() + asked.folders.len();
    if count == 0 || count > selection::MOST {
        return Err(bad("send 1 to 100 titles at a time"));
    }
    let action = match (asked.action.as_str(), asked.profile_id) {
        ("monitor", profile) => Action::Monitor(profile),
        ("unmonitor", None) => Action::Unmonitor,
        ("profile", Some(profile)) => Action::Profile(profile),
        ("profile", None) => return Err(bad("profile needs a profile_id")),
        ("remove", None) => Action::Remove(current.user.id),
        ("unmonitor" | "remove", Some(_)) => {
            return Err(bad("profile_id goes with monitor or profile"));
        }
        _ => return Err(bad("action is monitor, unmonitor, profile or remove")),
    };
    if !asked.folders.is_empty() && !matches!(action, Action::Monitor(_)) {
        return Err(bad(
            "titles only in the library can only be monitored, which puts them on the wanted list",
        ));
    }
    if let Some(profile) = asked.profile_id
        && let Some(why) = selection::unfit_profile(&state.pool, service, profile).await?
    {
        return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, why));
    }
    let lookups = Lookups {
        tmdb: state.tmdb.as_deref(),
        musicbrainz: state.musicbrainz.as_deref(),
    };
    let outcome = selection::change(
        &state.pool,
        lookups,
        service,
        action,
        &asked.wanted_ids,
        &asked.folders,
    )
    .await?;
    Ok(Json(outcome))
}

#[cfg(test)]
mod tests;
