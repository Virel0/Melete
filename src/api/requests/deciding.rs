//! An admin's side: approving a request, declining it, and undoing a decline.

use super::*;

/// Undoes a decline: the request waits for a decision again.
pub(super) async fn reopen(
    State(state): State<RequestState>,
    Path(id): Path<i64>,
) -> Result<Json<RequestView>, ApiError> {
    requests::reopen(&state.pool, id)
        .await?
        .map(Json)
        .map_err(refused)
}

pub(super) async fn approve(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<RequestView>, ApiError> {
    // Choosing where it goes is optional: no body means the defaults.
    let choice: Choice = if body.iter().all(u8::is_ascii_whitespace) {
        Choice::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| ApiError::new(StatusCode::BAD_REQUEST, format!("the body: {err}")))?
    };
    let mut view = requests::approve(
        &state.pool,
        state.sources.tmdb.as_deref(),
        state.musicbrainz.as_deref(),
        id,
        &current.user,
        &choice,
    )
    .await?
    .map_err(refused)?;
    if choice.searches_now() {
        fulfil::start(&state.pool, &state.sources, view.id);
        view.search_started = true;
    }
    Ok(Json(view))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Decline {
    pub(super) reason: String,
}

pub(super) async fn decline(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Json(body): Json<Decline>,
) -> Result<Json<RequestView>, ApiError> {
    requests::decline(&state.pool, id, &current.user, &body.reason)
        .await?
        .map(Json)
        .map_err(refused)
}
