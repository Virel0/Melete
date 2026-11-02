//! Playlist imports: starting one, listing them, one with its tracks, removing one, and approving
//! every track still waiting.

use super::*;
use crate::aoide::playlist_edits::{self, EditError, Editor};
use crate::aoide::writing::session_device;
use crate::db::now_ms;

/// The largest playlist file taken: a few thousand tracks with every column Exportify writes.
pub(super) const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NewImport {
    /// The playlist's name, as shown.
    pub(super) name: String,
    /// The playlist as CSV, Exportify's columns or the like.
    pub(super) csv: String,
    /// Whether to make an Aoide playlist of it, with every track in it.
    #[serde(default)]
    pub(super) make_playlist: bool,
}

/// Starts importing a playlist: each track will be matched and asked for as a song, and with
/// `make_playlist`, put in an Aoide playlist of the same name.
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
    let playlist = if new.make_playlist {
        Some(make_playlist(&state.pool, &current, name).await?)
    } else {
        None
    };
    let made = playlist_import::create(
        &state.pool,
        &current.user,
        name,
        &tracks,
        playlist.as_deref(),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(made)))
}

/// A new, empty Aoide playlist named `name` for the import to fill, made as this session, as
/// one made from the web is. Refused to a user who may see only some music libraries, to whom
/// Aoide's sync log is closed.
async fn make_playlist(
    pool: &SqlitePool,
    current: &CurrentUser,
    name: &str,
) -> Result<String, ApiError> {
    if !current.access.sees_all_music(pool).await? {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "playlists are open only to users who may see every music library",
        ));
    }
    let device = session_device(current.token_id);
    let editor = Editor {
        user_id: current.user.id,
        device: &device,
        now: now_ms(),
    };
    playlist_edits::create(pool, &editor, name, None)
        .await
        .map_err(|err| match err {
            EditError::Write(err) => super::super::music_likes::write_failed(err),
            // Making one fails only in writing it.
            other => anyhow::anyhow!("making a playlist: {other:?}").into(),
        })
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
