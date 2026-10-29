//! Radarr's movies into the wanted list, with their ids for folders already on disk.

use super::*;

/// What a Radarr import did, for the admin who ran it.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct RadarrReport {
    /// Movies Radarr listed with a TMDB id.
    pub movies: usize,
    /// Of those, the ones Radarr is looking for.
    pub monitored: usize,
    /// Movies whose folder Melete already adopted.
    pub on_disk: usize,
    /// Of those, folders to be matched again with Radarr's id: unmatched ones, and ones whose
    /// match disagreed (matches made by hand are left alone).
    pub rematching: usize,
    /// Movies whose root folder matched no movies library.
    pub without_library: usize,
    /// Wanted movies from Radarr that Radarr no longer lists, removed.
    pub removed: u64,
}

/// Imports Radarr's movies into the wanted list, in one transaction. Running it again updates
/// the same entries. Adopted folders get Radarr's TMDB id, and their match is redone with it
/// unless it already agrees or was decided by hand.
pub async fn import_radarr(
    pool: &SqlitePool,
    radarr: &ArrClient,
    root_folders: &HashMap<String, String>,
) -> anyhow::Result<RadarrReport> {
    radarr.expect_app("Radarr").await?;
    let profiles: HashMap<i64, String> = radarr
        .quality_profiles()
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let movies = radarr.radarr_movies().await?;
    let libraries = libraries(pool, "movies").await?;
    let now = unix_now();

    let mut report = RadarrReport::default();
    let mut tx = pool.begin().await?;
    for movie in movies.iter().filter(|m| m.tmdb_id > 0) {
        report.movies += 1;
        report.monitored += usize::from(movie.monitored);
        // Taken off the list in Melete: an import leaves it off.
        if wanted_edit::removed_here(&mut tx, Kind::Movie, &movie.tmdb_id.to_string()).await? {
            continue;
        }
        let library_id = library_for(movie.root_folder_path.as_deref(), &libraries, root_folders);
        if library_id.is_none() {
            report.without_library += 1;
        }

        let adopted_id = adopt_movie(&mut tx, movie, library_id, &mut report).await?;
        upsert_wanted_movie(&mut tx, movie, &profiles, library_id, adopted_id, now).await?;
    }
    let listed: Vec<i64> = movies
        .iter()
        .filter(|m| m.tmdb_id > 0)
        .map(|m| m.tmdb_id)
        .collect();
    report.removed = remove_unlisted(&mut tx, "radarr", &listed).await?;
    tx.commit().await?;
    Ok(report)
}

/// Finds the movie's folder already on disk, gives it Radarr's TMDB id, and clears its match
/// for redoing unless it agrees or was made by hand; returns the adopted movie's id.
async fn adopt_movie(
    tx: &mut sqlx::SqliteConnection,
    movie: &crate::arr::RadarrMovie,
    library_id: Option<i64>,
    report: &mut RadarrReport,
) -> anyhow::Result<Option<i64>> {
    let folder = movie.path.as_deref().and_then(last_component);
    // Only in the mapped library; without one, only a folder name no other library has.
    let adopted: Option<(i64, Option<i64>, Option<String>)> = match folder {
        Some(folder) => {
            let found: Vec<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
                "SELECT m.id, m.match_tmdb_id, m.match_method FROM movies m
                     JOIN libraries l ON l.id = m.library_id
                     WHERE l.kind = 'movies' AND m.rel_path = ?1 AND m.missing_since IS NULL
                       AND (?2 IS NULL OR m.library_id = ?2)
                     LIMIT 2",
            )
            .bind(folder)
            .bind(library_id)
            .fetch_all(&mut *tx)
            .await?;
            if found.len() == 1 {
                found.into_iter().next()
            } else {
                None
            }
        }
        None => None,
    };
    let adopted_id = adopted.as_ref().map(|(id, _, _)| *id);
    if let Some((movie_id, matched, method)) = adopted {
        report.on_disk += 1;
        sqlx::query("UPDATE movies SET arr_tmdb_id = ? WHERE id = ?")
            .bind(movie.tmdb_id)
            .bind(movie_id)
            .execute(&mut *tx)
            .await?;
        if method.as_deref() != Some("manual") && matched != Some(movie.tmdb_id) {
            sqlx::query(
                "UPDATE movies SET match_status = NULL, match_tmdb_id = NULL,
                         match_method = NULL, match_candidates = NULL
                     WHERE id = ?",
            )
            .bind(movie_id)
            .execute(&mut *tx)
            .await?;
            report.rematching += 1;
        }
    }
    Ok(adopted_id)
}

/// Adds the movie to the wanted list, or updates its entry when that came from Radarr.
async fn upsert_wanted_movie(
    tx: &mut sqlx::SqliteConnection,
    movie: &crate::arr::RadarrMovie,
    profiles: &HashMap<i64, String>,
    library_id: Option<i64>,
    adopted_id: Option<i64>,
    now: i64,
) -> anyhow::Result<()> {
    let profile = movie.quality_profile_id.and_then(|id| profiles.get(&id));
    sqlx::query(
        "INSERT INTO wanted_movies (tmdb_id, imdb_id, title, year, monitored, quality_profile,
                 minimum_availability, library_id, movie_id, source, source_id, added_at,
                 original_language)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'radarr', ?, ?, ?)
             ON CONFLICT (tmdb_id) DO UPDATE SET
                 imdb_id = excluded.imdb_id, title = excluded.title, year = excluded.year,
                 monitored = CASE wanted_movies.monitored_here WHEN 1 THEN wanted_movies.monitored
                                  ELSE excluded.monitored END,
                 quality_profile = excluded.quality_profile,
                 minimum_availability = excluded.minimum_availability,
                                  library_id = excluded.library_id, movie_id = excluded.movie_id,
                 source_id = excluded.source_id, original_language = excluded.original_language
             WHERE wanted_movies.source = 'radarr'",
    )
    .bind(movie.tmdb_id)
    .bind(movie.imdb_id.as_deref().filter(|i| !i.is_empty()))
    .bind(&movie.title)
    .bind(movie.year.filter(|y| *y > 0))
    .bind(movie.monitored)
    .bind(profile)
    .bind(&movie.minimum_availability)
    .bind(library_id)
    .bind(adopted_id)
    .bind(movie.id)
    .bind(now)
    .bind(original_language(movie.original_language.as_ref()))
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// The *arr's original language as Melete's code; None when unknown.
pub(super) fn original_language(language: Option<&crate::arr::IdName>) -> Option<&'static str> {
    crate::profiles::import::language_code(&language?.name)
}
