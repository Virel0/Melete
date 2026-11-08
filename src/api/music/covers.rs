//! Album covers and motion covers: from disk, the cache or Cover Art Archive, served with their
//! image type.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct CoverRow {
    pub(super) root: String,
    pub(super) cover_file: Option<String>,
    pub(super) motion_file: Option<String>,
    pub(super) release_group: Option<String>,
    pub(super) archive_cover: bool,
}

pub(super) async fn cover_row(pool: &SqlitePool, id: i64) -> Result<CoverRow, ApiError> {
    let row: Option<CoverRow> = sqlx::query_as(
        "SELECT l.path AS root, a.cover_file, a.motion_file,
             a.match_release_group_id AS release_group, coalesce(r.cover_front, 0) AS archive_cover
         FROM music_albums a JOIN libraries l ON l.id = a.library_id
         LEFT JOIN mb_releases r ON r.id = a.match_release_id
         WHERE a.id = ? AND a.missing_since IS NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "no such album"))
}

pub(super) fn no_cover() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "this album has no cover")
}

/// A file from the library, with byte ranges (for video), kept by clients a day.
pub(super) async fn serve(path: PathBuf, request: Request) -> Result<Response, ApiError> {
    let mut response = ServeFile::new(path)
        .try_call(request)
        .await?
        .map(axum::body::Body::new);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(COVER_CACHE));
    Ok(response)
}

/// What kind of image some bytes are, by their first bytes.
pub(super) fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub(super) fn image(bytes: Vec<u8>) -> Result<Response, ApiError> {
    let content_type = image_type(&bytes).ok_or_else(no_cover)?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, COVER_CACHE),
        ],
        bytes,
    )
        .into_response())
}

/// The sizes a cover can be asked for at, in pixels wide (what apps draw rows, grids, players
/// and page headers at), each with the name its files are kept under.
const COVER_SIZES: [(u32, &str); 4] = [(160, "160"), (400, "400"), (600, "600"), (800, "800")];
/// Covers being made at a size at once: a grid opening asks for many.
static RESIZING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Deserialize)]
pub(super) struct CoverQuery {
    /// One of [`COVER_SIZES`]' widths; leave out for the cover as it is.
    pub(super) size: Option<u32>,
}

/// The size asked for as one of the known ones, so nothing the caller sent names a file.
pub(super) fn cover_size(asked: Option<u32>) -> Result<Option<(u32, &'static str)>, ApiError> {
    let Some(asked) = asked else {
        return Ok(None);
    };
    COVER_SIZES
        .into_iter()
        .find(|(width, _)| *width == asked)
        .map(Some)
        .ok_or_else(|| bad_request("size is 160, 400, 600 or 800"))
}

/// An album's cover as found: a file in its folder, or bytes from a track's tags or the Cover
/// Art Archive (cached). Each with a name for what it is, which changes when the cover does.
enum Found {
    File { path: PathBuf, name: String },
    Bytes { bytes: Vec<u8>, name: String },
}

pub(super) async fn cover(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    Query(query): Query<CoverQuery>,
    request: Request,
) -> Result<Response, ApiError> {
    let size = cover_size(query.size)?;
    if !current.access.sees_album(&state.pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such album"));
    }
    let found = find_cover(&state, id).await?;
    let Some(size) = size else {
        return match found {
            Found::File { path, .. } => serve(path, request).await,
            Found::Bytes { bytes, .. } => image(bytes),
        };
    };
    sized(&state, found, size).await
}

/// The cover at `size` pixels wide at most (never larger than it is), as a JPEG made once and
/// kept beside the other cached covers.
async fn sized(
    state: &MusicState,
    found: Found,
    (width, label): (u32, &'static str),
) -> Result<Response, ApiError> {
    let name = match &found {
        Found::File { name, .. } | Found::Bytes { name, .. } => name,
    };
    let key = Sha256::digest(name.as_bytes());
    let hex: String = key[..12].iter().map(|b| format!("{b:02x}")).collect();
    let dir = state.covers.join("sized");
    let cached = dir.join(format!("{hex}-{label}.jpg"));
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        return image(bytes);
    }
    let bytes = match found {
        Found::File { path, .. } => tokio::fs::read(&path).await.map_err(|_| no_cover())?,
        Found::Bytes { bytes, .. } => bytes,
    };
    let jpeg = resized(state, &bytes, width).await?;
    keep(&dir, &cached, &jpeg).await;
    image(jpeg)
}

/// A cover as a JPEG `width` pixels wide at most (never larger than it is), a few at a time.
pub(super) async fn resized(
    state: &MusicState,
    bytes: &[u8],
    width: u32,
) -> Result<Vec<u8>, ApiError> {
    let _turn = RESIZING
        .acquire()
        .await
        .map_err(|err| anyhow::anyhow!(err))?;
    crate::api::local_artwork::resize(&state.ffmpeg, bytes, width)
        .await
        .map_err(|err| {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't make a cover at a size"
            );
            no_cover()
        })
}

/// The album's cover: the image in its folder, else a picture in a track's tags, else the Cover
/// Art Archive's for its release group (fetched once, with MusicBrainz matching on).
async fn find_cover(state: &MusicState, id: i64) -> Result<Found, ApiError> {
    let row = cover_row(&state.pool, id).await?;
    if let Some(file) = &row.cover_file {
        let path = resolve(&row.root, file).await?;
        let stamp = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| Some((m.len(), m.modified().ok()?)))
            .map(|(len, at)| format!("{len}:{at:?}"))
            .unwrap_or_default();
        let name = format!("file:{id}:{file}:{stamp}");
        return Ok(Found::File { path, name });
    }
    // A picture in a track's tags, pulled out once per version of that file.
    let tagged: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT rel_path, size, modified_at FROM music_tracks
         WHERE album_id = ? AND missing_since IS NULL AND has_picture = 1
         ORDER BY coalesce(disc_number, 1), track_number, rel_path LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((rel_path, size, modified)) = tagged {
        let key = Sha256::digest(format!("{rel_path}\0{size}\0{modified}"));
        let hex: String = key[..12].iter().map(|b| format!("{b:02x}")).collect();
        let name = format!("tag:{id}:{hex}");
        let cached = state.covers.join(format!("album-{id}-{hex}.img"));
        if let Ok(bytes) = tokio::fs::read(&cached).await {
            return Ok(Found::Bytes { bytes, name });
        }
        let path = resolve(&row.root, &rel_path).await?;
        let picture = tokio::task::spawn_blocking(move || crate::library::tags::picture(&path))
            .await
            .map_err(|err| anyhow::anyhow!(err))?
            .map_err(|_| no_cover())?;
        let Some((bytes, _)) = picture else {
            return Err(no_cover());
        };
        keep(&state.covers, &cached, &bytes).await;
        return Ok(Found::Bytes { bytes, name });
    }
    // The Cover Art Archive's, for a matched release group that has one. The id names cache
    // files, so only a real MusicBrainz id (hex and dashes) is used.
    let group = row.release_group.as_deref();
    let group = group.filter(|g| crate::library::tags::is_mbid(g));
    let (Some(coverart), Some(group), true) = (&state.coverart, group, row.archive_cover) else {
        return Err(no_cover());
    };
    let name = format!("rg:{group}");
    let cached = state.covers.join(format!("rg-{group}.img"));
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        return Ok(Found::Bytes { bytes, name });
    }
    let miss = state.covers.join(format!("rg-{group}.none"));
    let recent_miss = tokio::fs::metadata(&miss)
        .await
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < MISS_FOR);
    if recent_miss {
        return Err(no_cover());
    }
    match coverart.front(group).await {
        Ok(coverart::Fetched::Image { bytes, .. }) if image_type(&bytes).is_some() => {
            keep(&state.covers, &cached, &bytes).await;
            // Its colours can be worked out now it's here.
            crate::jobs::COVERS.notify_one();
            Ok(Found::Bytes { bytes, name })
        }
        Ok(_) => {
            keep(&state.covers, &miss, b"").await;
            Err(no_cover())
        }
        Err(err) => Err(ApiError::new(StatusCode::BAD_GATEWAY, format!("{err:#}"))),
    }
}

/// Writes a cache file through a temporary name, so a half-written one is never served.
/// Failing to cache only costs a fetch next time.
pub(super) async fn keep(dir: &FsPath, path: &FsPath, bytes: &[u8]) {
    let temporary = path.with_extension("partial");
    let written = async {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(&temporary, bytes).await?;
        tokio::fs::rename(&temporary, path).await
    };
    if let Err(err) = written.await {
        tracing::warn!(error = %err, "couldn't cache an album cover");
    }
}

pub(super) async fn motion(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    request: Request,
) -> Result<Response, ApiError> {
    if !current.access.sees_album(&state.pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such album"));
    }
    let row = cover_row(&state.pool, id).await?;
    if let Some(file) = &row.motion_file {
        return serve(resolve(&row.root, file).await?, request).await;
    }
    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        "this album has no motion cover",
    ))
}
