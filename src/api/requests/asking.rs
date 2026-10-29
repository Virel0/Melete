//! A requester's side: asking for something, looking at a request, withdrawing it, and changing a
//! series' season.

use super::*;

pub(super) async fn create(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Json(new): Json<NewRequest>,
) -> Result<(StatusCode, Json<RequestView>), ApiError> {
    let searches_now = new.choice().searches_now();
    let mut view = requests::create(
        &state.pool,
        state.sources.tmdb.as_deref(),
        state.musicbrainz.as_deref(),
        &current.user,
        new,
    )
    .await?
    .map_err(refused)?;
    // Approved at once by the requester's standing, or a season of a series wanted already
    // (now monitored): fetched now, as an admin's approval is, unless an admin said to wait.
    if view.state == "approved"
        && (view.decided_by.is_some() || view.season.is_some())
        && searches_now
    {
        fulfil::start(&state.pool, &state.sources, view.id);
        view.search_started = true;
    }
    Ok((StatusCode::CREATED, Json(view)))
}

pub(super) async fn one(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<RequestView>, ApiError> {
    match requests::get(&state.pool, id).await? {
        Some(view) if view.user_id == current.user.id || current.user.is_admin => Ok(Json(view)),
        _ => Err(refused(Refused::NotFound)),
    }
}

pub(super) async fn withdraw(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    requests::withdraw(&state.pool, id, &current.user)
        .await?
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(refused)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestChange {
    /// For a series: one season, or null for all of it.
    pub(super) season: Option<i64>,
}

/// Changes what a request still waiting asks for: a series' season.
pub(super) async fn change(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<RequestChange>,
) -> Result<Json<RequestView>, ApiError> {
    requests::change_season(&state.pool, id, &current.user, body.season)
        .await?
        .map(Json)
        .map_err(refused)
}
