//! Artists' portraits for music search results: the picture a search found on Wikimedia
//! Commons, served only on [`crate::artist_portraits`]' terms, fetched once from Commons'
//! picture hosts and kept, at the sizes album covers come in.

use super::*;

fn no_portrait() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such portrait")
}

pub(super) async fn artist_portrait(
    State(state): State<MusicState>,
    Path(artist): Path<String>,
    Query(query): Query<CoverQuery>,
) -> Result<Response, ApiError> {
    let size = cover_size(query.size)?;
    let now = crate::db::unix_now();
    // Commons is there only with MusicBrainz on; the id names a folder, and only a real
    // MusicBrainz id (hex and dashes) is servable. Served alike to everyone signed in: an
    // artist only in libraries hidden from someone is, to them, like any other.
    let Some(wikimedia) = &state.wikimedia else {
        return Err(no_portrait());
    };
    let picture = crate::artist_portraits::servable(&state.pool, &artist, now).await?;
    let Some(picture) = picture else {
        return Err(no_portrait());
    };
    let original = crate::artist_portraits::cached(&state.covers, &artist, &picture);
    let sized = size.map(|(width, label)| (width, original.with_extension(format!("{label}.jpg"))));
    if let Some((_, path)) = &sized
        && let Ok(bytes) = tokio::fs::read(path).await
    {
        return image(bytes);
    }
    let bytes = match tokio::fs::read(&original).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = fetch(&state.pool, wikimedia, &artist, &picture, now).await?;
            // An earlier picture of theirs, if the portrait changed, goes.
            let dir = crate::artist_portraits::folder(&state.covers, &artist);
            let _ = tokio::fs::remove_dir_all(&dir).await;
            keep(&dir, &original, &bytes).await;
            bytes
        }
    };
    let Some((width, path)) = sized else {
        return image(bytes);
    };
    let jpeg = resized(&state, &bytes, width).await?;
    keep(path.parent().unwrap_or(&state.covers), &path, &jpeg).await;
    image(jpeg)
}

/// The picture from Commons' picture hosts. When it fails, or isn't an image, that's noted, so it
/// isn't offered or asked for again for a while. Why a fetch failed is logged, never sent.
async fn fetch(
    pool: &SqlitePool,
    wikimedia: &crate::wikimedia::Client,
    artist: &str,
    picture: &str,
    now: i64,
) -> Result<Vec<u8>, ApiError> {
    match wikimedia.download(picture).await {
        Ok((bytes, _)) if image_type(&bytes).is_some() => Ok(bytes),
        Ok(_) => {
            crate::artist_portraits::failed(pool, artist, now).await?;
            Err(no_portrait())
        }
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't fetch an artist's portrait"
            );
            crate::artist_portraits::failed(pool, artist, now).await?;
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "Wikimedia Commons didn't give the portrait",
            ))
        }
    }
}
