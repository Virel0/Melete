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

pub(super) async fn cover(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
    request: Request,
) -> Result<Response, ApiError> {
    if !current.access.sees_album(&state.pool, id).await? {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "no such album"));
    }
    let row = cover_row(&state.pool, id).await?;
    if let Some(file) = &row.cover_file {
        return serve(resolve(&row.root, file).await?, request).await;
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
        let cached = state.covers.join(format!("album-{id}-{hex}.img"));
        if let Ok(bytes) = tokio::fs::read(&cached).await {
            return image(bytes);
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
        return image(bytes);
    }
    // The Cover Art Archive's, for a matched release group that has one. The id names cache
    // files, so only a real MusicBrainz id (hex and dashes) is used.
    let group = row.release_group.as_deref();
    let group = group.filter(|g| crate::library::tags::is_mbid(g));
    let (Some(coverart), Some(group), true) = (&state.coverart, group, row.archive_cover) else {
        return Err(no_cover());
    };
    let cached = state.covers.join(format!("rg-{group}.img"));
    if let Ok(bytes) = tokio::fs::read(&cached).await {
        return image(bytes);
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
            image(bytes)
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
    // The album's own first, always.
    if let Some(file) = &row.motion_file {
        return serve(resolve(&row.root, file).await?, request).await;
    }
    if crate::motion::shown_to(&state.pool, current.user.id).await?
        && let Some(file) = crate::motion::current_file(&state.pool, id).await?
    {
        return serve(crate::motion::video_path(&state.motion, &file), request).await;
    }
    Err(ApiError::new(
        StatusCode::NOT_FOUND,
        "this album has no motion cover",
    ))
}
