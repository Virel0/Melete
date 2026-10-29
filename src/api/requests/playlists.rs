//! Playlist imports: starting one, listing them, one with its tracks, removing one, and approving
//! every track still waiting.

use super::*;

/// The largest playlist file taken: a few thousand tracks with every column Exportify writes.
pub(super) const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewImport {
    /// The playlist's name, as shown.
    pub(super) name: String,
    /// The playlist as CSV, Exportify's columns or the like.
    pub(super) csv: String,
}

/// Starts importing a playlist: each track will be matched and asked for as a song.
pub(super) async fn import(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Json(new): Json<NewImport>,
) -> Result<(StatusCode, Json<ImportSummary>), ApiError> {
    let bad = |message: String| ApiError::new(StatusCode::BAD_REQUEST, message);
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(bad("name must be 1 to 200 characters".into()));
    }
    if state.musicbrainz.is_none() {
        return Err(refused(Refused::NoMusicBrainz));
    }
    let tracks = playlist_import::parse(&new.csv).map_err(bad)?;
    let made = playlist_import::create(&state.pool, &current.user, name, &tracks).await?;
    Ok((StatusCode::ACCEPTED, Json(made)))
}

#[derive(Deserialize)]
pub(super) struct ImportsQuery {
    /// Everyone's, for an admin.
    #[serde(default)]
    pub(super) all: bool,
}

pub(super) async fn imports(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ImportsQuery>,
) -> Result<Json<Vec<ImportSummary>>, ApiError> {
    if query.all && !current.user.is_admin {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "everyone's imports are for admins",
        ));
    }
    let user = (!query.all).then_some(current.user.id);
    Ok(Json(playlist_import::list(&state.pool, user).await?))
}

/// An import with each of its tracks; someone else's is as good as none, but to an admin.
pub(super) async fn one_import(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<ImportView>, ApiError> {
    match playlist_import::get(&state.pool, id).await? {
        Some(view) if view.summary.user_id == current.user.id || current.user.is_admin => {
            Ok(Json(view))
        }
        _ => Err(ApiError::new(StatusCode::NOT_FOUND, "no such import")),
    }
}

#[derive(Serialize)]
pub(super) struct ImportRemoved {
    /// Its requests still waiting for an admin, taken back with it.
    pub(super) taken_back: u64,
}

/// Removes an import: its own or, for an admin, anyone's.
pub(super) async fn remove_import(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<ImportRemoved>, ApiError> {
    match playlist_import::remove(&state.pool, id, &current.user).await? {
        Some(taken_back) => Ok(Json(ImportRemoved { taken_back })),
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "no such import")),
    }
}

/// Approves every track of an import still waiting; they're fetched one after another.
pub(super) async fn approve_import(
    State(state): State<RequestState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if playlist_import::summary(&state.pool, id).await?.is_none() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such import"));
    }
    let (approved, not) = playlist_import::approve_all(
        &state.pool,
        state.musicbrainz.as_deref(),
        &state.sources,
        id,
        &current.user,
    )
    .await?;
    Ok(Json(json!({
        "approved": approved,
        "refused": not.iter().map(|(request_id, why)| json!({"request_id": request_id, "why": why})).collect::<Vec<_>>(),
    })))
}
