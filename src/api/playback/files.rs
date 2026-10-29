//! A file with its probe, probed on the spot when the background job hasn't reached it.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct FileRow {
    pub(super) id: i64,
    pub(super) root: String,
    pub(super) rel_path: String,
    pub(super) size: i64,
    pub(super) modified_at: i64,
    pub(super) info: Option<String>,
    pub(super) probe_error: Option<String>,
    pub(super) probed_size: Option<i64>,
    pub(super) probed_modified_at: Option<i64>,
}

/// A present file with what ffprobe found in it.
pub(in super::super) struct LoadedFile {
    /// The file's row id.
    pub id: i64,
    /// The library folder it's in.
    pub root: String,
    /// Its path within that folder.
    pub rel_path: String,
    /// Bytes.
    pub size: i64,
    /// When it was last changed on disk, in seconds since the Unix epoch.
    pub modified_at: i64,
    /// `None` when ffprobe rejected the file.
    pub media: Option<probe::MediaInfo>,
}

/// Loads a file (or an extra) with its probe, probing on the spot if the background job hasn't
/// reached it or the file changed since. 404 for unknown or missing files; 503 when it can't be
/// read now.
pub(in super::super) async fn load_file(
    pool: &SqlitePool,
    ffprobe: &std::path::Path,
    id: i64,
) -> Result<LoadedFile, ApiError> {
    let query = if extras::is_extra_id(id) {
        "SELECT e.id, l.path AS root, e.rel_path, e.size, e.modified_at, e.probe_info AS info,
             e.probe_error, e.probed_size, e.probed_modified_at
         FROM media_extras e
         JOIN libraries l ON l.id = e.library_id
         WHERE e.id = ? AND e.missing_since IS NULL"
    } else {
        "SELECT f.id, l.path AS root, f.rel_path, f.size, f.modified_at, p.info,
             p.error AS probe_error, p.size AS probed_size, p.modified_at AS probed_modified_at
         FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         LEFT JOIN media_probes p ON p.file_id = f.id
         WHERE f.id = ? AND f.missing_since IS NULL"
    };
    let row: Option<FileRow> = sqlx::query_as(query).bind(id).fetch_optional(pool).await?;
    let Some(f) = row else {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such file"));
    };
    let current = f.probed_size == Some(f.size) && f.probed_modified_at == Some(f.modified_at);
    let media = match (current, &f.info, &f.probe_error) {
        (true, Some(info), _) => Some(serde_json::from_str(info)?),
        (true, None, Some(_)) => None,
        _ => {
            let path = resolve(&f.root, &f.rel_path).await?;
            let result = {
                let _slot = ON_DEMAND_PROBES.acquire().await?;
                probe::probe_with_chapters(ffprobe, &path).await
            };
            probe::store_probed(pool, f.id, f.size, f.modified_at, &result).await?;
            match result {
                Ok(probed) => Some(probed.info),
                Err(probe::ProbeError::Failed(_)) => None,
                Err(err) => {
                    tracing::warn!(rel_path = f.rel_path, error = %err, "probing on request failed");
                    return Err(ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "the file can't be read right now; try again later",
                    ));
                }
            }
        }
    };
    Ok(LoadedFile {
        id: f.id,
        root: f.root,
        rel_path: f.rel_path,
        size: f.size,
        modified_at: f.modified_at,
        media,
    })
}

/// A file with what's in it: container, duration, video, audio and subtitle streams. Probes
/// on the spot if the background job hasn't reached the file yet.
pub(super) async fn file_info(
    State(state): State<PlaybackState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    limit: Option<Extension<crate::ways_in::VideoLimit>>,
) -> Result<Json<Value>, ApiError> {
    require_playable(&state.pool, &current.access, id).await?;
    let f = load_file(&state.pool, &state.ffprobe, id).await?;
    Ok(Json(json!({
        "id": f.id,
        "path": f.rel_path,
        "size": f.size,
        "modified_at": f.modified_at,
        "media": f.media,
        "max_bitrate": way_limit(limit),
        "segments": super::super::segments::of_file(&state.pool, id).await?,
        "chapters": probe::chapters_of(&state.pool, id).await?,
        "trickplay": crate::trickplay::made(&state.pool, id).await?,
        "offline": crate::settings::offline_copies(&state.pool).await?,
        "subtitle_search": crate::settings::value(&state.pool, "MELETE_OPENSUBTITLES_KEY")
            .await?
            .is_some(),
    })))
}

/// A 404 unless a present library file or extra has this id and the user may see it.
pub(in super::super) async fn require_playable(
    pool: &SqlitePool,
    access: &Access,
    id: i64,
) -> Result<(), ApiError> {
    if !extras::is_extra_id(id) {
        return require_file(pool, access, id).await;
    }
    let found: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT x.id FROM media_extras x WHERE x.id = ? AND x.missing_since IS NULL AND {}",
        access.extra_sql("x")
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match found {
        Some(_) => Ok(()),
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "no such file")),
    }
}

/// A 404 unless the file is present and the user may see it. Extras aren't files here: what's
/// kept per file (progress, watched, tracks chosen) isn't kept for them.
pub(in super::super) async fn require_file(
    pool: &SqlitePool,
    access: &Access,
    id: i64,
) -> Result<(), ApiError> {
    let found: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT x.id FROM media_files x WHERE x.id = ? AND x.missing_since IS NULL AND {}",
        access.file_sql("x")
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match found {
        Some(_) => Ok(()),
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "no such file")),
    }
}

/// Whether whoever was given a stream link may still see its file: what a user may see can
/// change after a link was made, and the link then stops working.
pub(in super::super) async fn holder_sees(
    pool: &SqlitePool,
    token_id: i64,
    file_id: i64,
) -> Result<bool, ApiError> {
    Ok(match holder_access(pool, token_id).await? {
        Some(access) if extras::is_extra_id(file_id) => access.sees_extra(pool, file_id).await?,
        Some(access) => access.sees_file(pool, file_id).await?,
        None => false,
    })
}

/// What the user a link was given to may see now; None once the session or key is gone.
pub(in super::super) async fn holder_access(
    pool: &SqlitePool,
    token_id: i64,
) -> Result<Option<Access>, ApiError> {
    let user: Option<(i64, bool)> = sqlx::query_as(
        "SELECT u.id, u.is_admin FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.id = ?",
    )
    .bind(token_id)
    .fetch_optional(pool)
    .await?;
    let Some((user_id, is_admin)) = user else {
        return Ok(None);
    };
    Ok(Some(Access::of(pool, user_id, is_admin).await?))
}
