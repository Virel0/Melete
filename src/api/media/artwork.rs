//! Artwork from TMDB, fetched once and cached under names that never change meaning, and
//! titles' own images from their folders, resized once and cached the same way.

use super::*;

/// Serves one TMDB image, downloading it into the cache on first use, or a title's own image.
///
/// Only images some cached title or episode refers to are fetched, so this can't be used to
/// pull arbitrary files through the server.
pub(super) async fn artwork(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Path((size, file)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let size_known = [POSTER_SIZES, BACKDROP_SIZES, STILL_SIZES, LOGO_SIZES]
        .iter()
        .any(|sizes| sizes.contains(&size.as_str()))
        || size == super::super::credits::PROFILE_SIZE;
    if !size_known {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "unknown size; use the artwork URLs the API returns",
        ));
    }
    let Some(content_type) = artwork_type(&file) else {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "not an artwork file",
        ));
    };
    if !current.access.is_everything() && !shown_to(&state.pool, &current, &file).await? {
        return Err(not_found("artwork"));
    }
    let path = state.artwork_dir.join(&size).join(&file);
    let bytes = if crate::api::local_artwork::is_local(&file) {
        crate::api::local_artwork::image(
            &state.pool,
            &state.artwork_dir,
            state.title_artwork.ffmpeg(),
            &size,
            &file,
        )
        .await?
        .ok_or_else(|| not_found("artwork"))?
    } else {
        match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == ErrorKind::NotFound => {
                from_tmdb(&state, &size, &file, &path).await?
            }
            Err(err) => return Err(err.into()),
        }
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        bytes,
    )
        .into_response())
}

/// Downloads a TMDB image some cached title or episode refers to, at a size the API hands out
/// for its kind, into the cache.
async fn from_tmdb(
    state: &MediaState,
    size: &str,
    file: &str,
    path: &std::path::Path,
) -> Result<Vec<u8>, ApiError> {
    {
        {
            let (poster, backdrop, still, logo, profile): (bool, bool, bool, bool, bool) =
                sqlx::query_as(
                    "SELECT
                     EXISTS (SELECT 1 FROM tmdb_movies WHERE poster_path = ?1
                             UNION ALL SELECT 1 FROM tmdb_series WHERE poster_path = ?1
                             UNION ALL SELECT 1 FROM tmdb_collections WHERE poster_path = ?1
                             UNION ALL SELECT 1 FROM requests WHERE poster = ?1
                                 AND kind IN ('movie', 'series')),
                     EXISTS (SELECT 1 FROM tmdb_movies
                                 WHERE backdrop_path = ?1 OR textless_backdrop_path = ?1
                             UNION ALL SELECT 1 FROM tmdb_series
                                 WHERE backdrop_path = ?1 OR textless_backdrop_path = ?1
                             UNION ALL SELECT 1 FROM tmdb_collections WHERE backdrop_path = ?1),
                     EXISTS (SELECT 1 FROM tmdb_episodes WHERE still_path = ?1),
                     EXISTS (SELECT 1 FROM tmdb_movies WHERE logo_path = ?1
                             UNION ALL SELECT 1 FROM tmdb_series WHERE logo_path = ?1),
                     EXISTS (SELECT 1 FROM tmdb_people WHERE profile_path = ?1)",
                )
                .bind(format!("/{file}"))
                .fetch_one(&state.pool)
                .await?;
            let allowed = (poster && POSTER_SIZES.contains(&size))
                || (backdrop && BACKDROP_SIZES.contains(&size))
                || (still && STILL_SIZES.contains(&size))
                || (logo && LOGO_SIZES.contains(&size))
                || (profile && size == super::super::credits::PROFILE_SIZE)
                // A poster a discovery answer listed, when an admin allows those.
                || (size == crate::discover_posters::SIZE
                    && crate::discover_posters::servable(&state.pool, &format!("/{file}"), unix_now())
                        .await?);
            if !allowed {
                return Err(not_found("artwork"));
            }
            let key = format!("{size}/{file}");
            let missed_lately = state
                .misses
                .lock()
                .expect("no holder panics")
                .get(&key)
                .is_some_and(|when| when.elapsed() < MISS_FOR);
            if missed_lately {
                return Err(not_found("artwork"));
            }
            let fetched = state.images.fetch(size, file).await.map_err(|err| {
                tracing::warn!(error = format!("{err:#}"), "artwork download failed");
                ApiError::new(StatusCode::BAD_GATEWAY, "could not fetch artwork from TMDB")
            })?;
            let Some(bytes) = fetched else {
                let mut misses = state.misses.lock().expect("no holder panics");
                misses.retain(|_, when| when.elapsed() < MISS_FOR);
                misses.insert(key, std::time::Instant::now());
                return Err(not_found("artwork"));
            };
            if let Err(err) = save_atomically(path, &bytes).await {
                // Still serve it; the next request just downloads it again.
                tracing::warn!(error = format!("{err:#}"), "could not cache artwork");
            }
            Ok(bytes)
        }
    }
}

/// Whether an image belongs to something a restricted user may see: a movie's, series' or
/// episode's they may see (TMDB's or its own from its folder), the picture of someone credited
/// on one, the poster of a collection with one, a request of theirs, or a poster a discovery
/// answer listed that no title in the library has (with a rating limit, only one listed for a
/// title rated within it). Cached or not, nothing else is served to them.
async fn shown_to(pool: &SqlitePool, current: &CurrentUser, file: &str) -> Result<bool, ApiError> {
    let access = &current.access;
    let images = "?1 IN (t.poster_path, t.backdrop_path, t.textless_backdrop_path, t.logo_path)";
    let path = format!("/{file}");
    let (titles, in_library): (bool, bool) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT
             EXISTS (SELECT 1 FROM movies x JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
                     WHERE x.missing_since IS NULL AND {movie} AND {images})
             OR EXISTS (SELECT 1 FROM series x JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
                     WHERE x.missing_since IS NULL AND {series} AND {images})
             OR EXISTS (SELECT 1 FROM series x
                     JOIN tmdb_episodes e ON e.series_tmdb_id = x.match_tmdb_id
                     WHERE x.missing_since IS NULL AND {series} AND e.still_path = ?1)
             OR EXISTS (SELECT 1 FROM requests WHERE user_id = ?2 AND poster = ?1)
             OR EXISTS (SELECT 1 FROM movies x WHERE x.missing_since IS NULL AND {movie}
                     AND ?1 IN (x.local_poster_key, x.local_backdrop_key))
             OR EXISTS (SELECT 1 FROM series x WHERE x.missing_since IS NULL AND {series}
                     AND ?1 IN (x.local_poster_key, x.local_backdrop_key))
             OR EXISTS (SELECT 1 FROM tmdb_people p
                     JOIN tmdb_credits c ON c.person_id = p.id
                     JOIN movies x ON x.match_tmdb_id = c.movie_tmdb_id
                     WHERE p.profile_path = ?1 AND x.missing_since IS NULL AND {movie})
             OR EXISTS (SELECT 1 FROM tmdb_people p
                     JOIN tmdb_credits c ON c.person_id = p.id
                     JOIN series x ON x.match_tmdb_id = c.series_tmdb_id
                     WHERE p.profile_path = ?1 AND x.missing_since IS NULL AND {series})
             OR EXISTS (SELECT 1 FROM tmdb_collections k
                     JOIN tmdb_movies t ON t.collection_id = k.id
                     JOIN movies x ON x.match_tmdb_id = t.tmdb_id
                     WHERE ?1 IN (k.poster_path, k.backdrop_path) AND x.missing_since IS NULL
                         AND {movie}),
             EXISTS (SELECT 1 FROM movies x JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
                     WHERE {images})
             OR EXISTS (SELECT 1 FROM series x JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
                     WHERE {images})",
        movie = access.movie_sql("x"),
        series = access.series_sql("x"),
    )))
    .bind(&path)
    .bind(current.user.id)
    .fetch_one(pool)
    .await?;
    if titles {
        return Ok(true);
    }
    Ok(
        !in_library
            && crate::discover_posters::servable_to(pool, access, &path, unix_now()).await?,
    )
}

/// The content type for a TMDB image file name, or `None` if the name is not one. The name
/// becomes part of a file path and a URL, so only plain names pass.
pub(in super::super) fn artwork_type(file: &str) -> Option<&'static str> {
    let (stem, extension) = file.rsplit_once('.')?;
    let plain = !stem.is_empty()
        && stem.len() <= 64
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    match extension {
        "jpg" if plain => Some("image/jpeg"),
        "png" if plain => Some("image/png"),
        _ => None,
    }
}

/// Writes to a temporary file beside `path`, then renames, so readers never see half a file.
pub(in super::super) async fn save_atomically(
    path: &std::path::Path,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no folder to write the file into"))?;
    tokio::fs::create_dir_all(dir).await?;
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    let temp = dir.join(format!(".{}.part", u64::from_le_bytes(suffix)));
    let written = match tokio::fs::write(&temp, bytes).await {
        Ok(()) => tokio::fs::rename(&temp, path).await,
        Err(err) => Err(err),
    };
    if let Err(err) = written {
        // A half-written temp file (disk full) must not pile up.
        let _ = tokio::fs::remove_file(&temp).await;
        return Err(err.into());
    }
    Ok(())
}
