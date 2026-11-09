//! Covers of music not in the library, for music search results: a release group's front
//! cover from the Cover Art Archive, served only on [`crate::search_covers`]' terms, fetched
//! once and kept under its id, at the sizes album covers come in.

use super::*;

fn no_such_cover() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "no such cover")
}

pub(super) async fn release_group_cover(
    State(state): State<MusicState>,
    Extension(current): Extension<CurrentUser>,
    Path(group): Path<String>,
    Query(query): Query<CoverQuery>,
) -> Result<Response, ApiError> {
    let size = cover_size(query.size)?;
    let now = crate::db::unix_now();
    // The archive is there only with MusicBrainz on; the id names a folder, and only a real
    // MusicBrainz id (hex and dashes) is servable. Served only to those a search listed it for:
    // an album in a library hidden from someone is, to them, like any album not in the library,
    // and nobody learns from it what someone else searched for.
    let Some(coverart) = &state.coverart else {
        return Err(no_such_cover());
    };
    if !crate::search_covers::servable(&state.pool, &group, current.user.id, now).await? {
        return Err(no_such_cover());
    }
    let dir = crate::search_covers::folder(&state.covers, &group);
    let sized = size.map(|(width, label)| (width, dir.join(format!("{label}.jpg"))));
    if let Some((_, path)) = &sized
        && let Ok(bytes) = tokio::fs::read(path).await
    {
        return image(bytes);
    }
    let original = dir.join("cover.img");
    let bytes = match tokio::fs::read(&original).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = fetch(&state.pool, coverart, &group, now).await?;
            keep(&dir, &original, &bytes).await;
            bytes
        }
    };
    let Some((width, path)) = sized else {
        return image(bytes);
    };
    let jpeg = resized(&state, &bytes, width).await?;
    keep(&dir, &path, &jpeg).await;
    image(jpeg)
}

/// The archive's front cover for `group`. When it has none, or fails, that's noted, so it isn't
/// offered or asked for again for a while. Why a fetch failed is logged, never sent.
async fn fetch(
    pool: &SqlitePool,
    coverart: &coverart::Client,
    group: &str,
    now: i64,
) -> Result<Vec<u8>, ApiError> {
    match coverart.front(group).await {
        Ok(coverart::Fetched::Image { bytes, .. }) if image_type(&bytes).is_some() => Ok(bytes),
        Ok(_) => {
            crate::search_covers::missed(pool, group, now).await?;
            Err(no_such_cover())
        }
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't fetch a search result's cover"
            );
            crate::search_covers::missed(pool, group, now).await?;
            Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "the Cover Art Archive didn't give the cover",
            ))
        }
    }
}
