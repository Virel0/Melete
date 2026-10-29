//! The movies list: every wanted movie, and every movie in the library no wanted movie claims.

use std::collections::HashMap;

use super::*;
use crate::profiles::App;

/// A movie's own fields in the list.
#[derive(Debug, Clone, Serialize)]
pub struct MovieDetail {
    /// Its folder in the library, when it's on disk.
    pub movie_id: Option<i64>,
    /// TMDB's id, when known.
    pub tmdb_id: Option<i64>,
    /// IMDb's id, when known.
    pub imdb_id: Option<String>,
    /// The quality of its largest file, as decisions read it; null when nothing's on disk or
    /// it can't be told yet.
    pub quality: Option<String>,
    /// The first day it's out (in cinemas, digitally or on disc), when TMDB gives one.
    pub release_date: Option<String>,
    /// Its largest file's id and path, which `quality` is read from.
    #[serde(skip)]
    pub(crate) largest: Option<(i64, String)>,
}

impl super::Folder for MovieDetail {
    fn folder(&self) -> Option<i64> {
        self.movie_id
    }
}

#[derive(sqlx::FromRow)]
struct WantedRow {
    id: i64,
    tmdb_id: i64,
    imdb_id: Option<String>,
    title: String,
    year: Option<i64>,
    monitored: bool,
    added_at: i64,
    movie_id: Option<i64>,
    profile_id: Option<i64>,
    profile: Option<String>,
    /// The profile's name as the arr gave it.
    arr_profile: Option<String>,
    original_language: Option<String>,
    library: Option<String>,
    category: Option<String>,
    poster_path: Option<String>,
    /// Whether TMDB's entry is here, so its dates (or their absence) can be trusted.
    known: bool,
    release_date: Option<String>,
    /// Whether that day has come.
    released: bool,
}

#[derive(sqlx::FromRow)]
struct LibraryRow {
    id: i64,
    match_tmdb_id: Option<i64>,
    arr_tmdb_id: Option<i64>,
    tmdb_id: Option<i64>,
    imdb_id: Option<String>,
    title: String,
    year: Option<i64>,
    poster_path: Option<String>,
    library: String,
    category: Option<String>,
}

/// A movie's present files, gathered: their bytes, the first found, the largest, and each
/// one's id and path.
#[derive(Default)]
struct Files {
    size: i64,
    first: Option<i64>,
    largest: Option<(i64, i64, String)>,
    all: Vec<(i64, String)>,
}

impl Files {
    /// The largest file's id and path.
    fn largest(&self) -> Option<(i64, String)> {
        self.largest
            .as_ref()
            .map(|(id, _, path)| (*id, path.clone()))
    }
}

/// The first of a TMDB movie's days out; '~' sorts after every date, standing for none.
const FIRST_DAY: &str = "nullif(min(coalesce(nullif(t.release_date, ''), '~'),
                                 coalesce(nullif(t.digital_release, ''), '~'),
                                 coalesce(nullif(t.physical_release, ''), '~')), '~')";

pub(super) static WANTED: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "SELECT w.id, w.tmdb_id, w.imdb_id, coalesce(t.title, w.title) AS title,
             coalesce(t.year, w.year) AS year, w.monitored, w.added_at, w.movie_id, w.profile_id,
             coalesce(p.name, w.quality_profile) AS profile, w.quality_profile AS arr_profile,
             w.original_language, l.name AS library, l.category,
             t.poster_path, t.tmdb_id IS NOT NULL AS known, {FIRST_DAY} AS release_date,
             coalesce({FIRST_DAY} <= date('now'), 0) AS released
         FROM wanted_movies w
         LEFT JOIN libraries l ON l.id = w.library_id
         LEFT JOIN quality_profiles p ON p.id = w.profile_id
         LEFT JOIN tmdb_movies t ON t.tmdb_id = w.tmdb_id"
    )
});

pub(super) const LIBRARY: &str = "
    SELECT m.id, m.imdb_id, coalesce(t.title, m.parsed_title) AS title,
           coalesce(t.year, m.parsed_year) AS year, t.poster_path, l.name AS library, l.category,
           m.match_tmdb_id, m.arr_tmdb_id, m.tmdb_id
    FROM movies m
    JOIN libraries l ON l.id = m.library_id
    LEFT JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
    WHERE m.missing_since IS NULL";

/// Every present movie file: its movie, id, path, bytes and when it was found.
pub(super) const FILES: &str = "
    SELECT movie_id, id, rel_path, size, coalesce(added_at, modified_at) FROM media_files
    WHERE movie_id IS NOT NULL AND missing_since IS NULL";

/// Every managed movie, unsorted. A wanted movie claims the folder it was adopted with, and
/// any folder in the library matched to the same TMDB movie; a folder no wanted movie claims
/// is a row of its own, when it holds a file.
pub async fn movies(pool: &SqlitePool) -> anyhow::Result<Vec<Title<MovieDetail>>> {
    let wanted: Vec<WantedRow> = sqlx::query_as(sqlx::AssertSqlSafe(WANTED.as_str()))
        .fetch_all(pool)
        .await?;
    let library: Vec<LibraryRow> = sqlx::query_as(LIBRARY).fetch_all(pool).await?;
    let files = present_files(pool).await?;
    let downloads = Downloads::load(pool, "wanted_movie_id").await?;

    let by_tmdb: HashMap<i64, usize> = wanted
        .iter()
        .enumerate()
        .map(|(i, w)| (w.tmdb_id, i))
        .collect();
    let by_folder: HashMap<i64, usize> = wanted
        .iter()
        .enumerate()
        .filter_map(|(i, w)| Some((w.movie_id?, i)))
        .collect();
    // Each wanted movie's folders, and the folders nobody wants.
    let mut claimed: Vec<Vec<&LibraryRow>> = vec![Vec::new(); wanted.len()];
    let mut unclaimed = Vec::new();
    for movie in &library {
        let owner = by_folder.get(&movie.id).copied().or_else(|| {
            [movie.match_tmdb_id, movie.arr_tmdb_id, movie.tmdb_id]
                .into_iter()
                .flatten()
                .find_map(|id| by_tmdb.get(&id).copied())
        });
        match owner {
            Some(i) => claimed[i].push(movie),
            None => unclaimed.push(movie),
        }
    }

    let mut profiles = cutoff::Profiles::new(pool, App::Radarr).await?;
    let mut titles = Vec::with_capacity(wanted.len() + unclaimed.len());
    for (w, folders) in wanted.into_iter().zip(claimed) {
        let unmet = movie_cutoff(pool, &mut profiles, &w, &files).await?;
        titles.push(wanted_title(w, &folders, &files, &downloads, unmet));
    }
    for movie in unclaimed {
        if let Some(present) = files.get(&movie.id) {
            titles.push(library_title(movie, present));
        }
    }
    Ok(titles)
}

/// Every present movie file, gathered by movie.
async fn present_files(pool: &SqlitePool) -> anyhow::Result<HashMap<i64, Files>> {
    let mut files: HashMap<i64, Files> = HashMap::new();
    let rows: Vec<(i64, i64, String, i64, i64)> = sqlx::query_as(FILES).fetch_all(pool).await?;
    for (movie, id, path, size, added) in rows {
        let entry = files.entry(movie).or_default();
        entry.size += size;
        entry.first = Some(entry.first.map_or(added, |f| f.min(added)));
        if entry.largest.as_ref().is_none_or(|(_, s, _)| size > *s) {
            entry.largest = Some((id, size, path.clone()));
        }
        entry.all.push((id, path));
    }
    Ok(files)
}

/// Whether a monitored wanted movie's file is below its profile's cutoff, judged on the folder
/// linked to it, as an upgrade search judges it.
async fn movie_cutoff(
    pool: &SqlitePool,
    profiles: &mut cutoff::Profiles,
    w: &WantedRow,
    files: &HashMap<i64, Files>,
) -> anyhow::Result<Option<bool>> {
    let Some(present) = w.movie_id.and_then(|id| files.get(&id)) else {
        return Ok(None);
    };
    if !w.monitored {
        return Ok(None);
    }
    let Some(profile) = profiles
        .get(pool, w.profile_id, w.arr_profile.as_deref())
        .await?
    else {
        return Ok(None);
    };
    let mut read = Vec::with_capacity(present.all.len());
    for (id, path) in &present.all {
        read.push(cutoff::read(pool, *id, path).await?);
    }
    let original = w.original_language.as_deref();
    Ok(cutoff::unmet(
        profile,
        read.iter().map(Option::as_ref),
        original,
    ))
}

/// A wanted movie's row, with the folders it claims. One that claims no folder and whose
/// newest download was imported counts as on disk: it waits for a scan to find it.
fn wanted_title(
    w: WantedRow,
    folders: &[&LibraryRow],
    files: &HashMap<i64, Files>,
    downloads: &Downloads,
    cutoff_unmet: Option<bool>,
) -> Title<MovieDetail> {
    let present: Vec<&Files> = folders.iter().filter_map(|m| files.get(&m.id)).collect();
    let on_disk = !present.is_empty() || (folders.is_empty() && downloads.imported(w.id));
    // A movie TMDB's entry isn't here for counts as out: it can't be told.
    let released = !w.known || w.released;
    let facts = Facts {
        on_disk,
        missing: i64::from(w.monitored && !on_disk && released),
        under_way: downloads.under_way(w.id).is_some(),
        monitored: w.monitored,
    };
    let shown = folders.iter().find(|m| files.contains_key(&m.id));
    Title {
        wanted_id: Some(w.id),
        title: w.title,
        year: w.year,
        poster: poster(w.poster_path),
        state: facts.state(),
        monitored: w.monitored,
        wanted: true,
        profile_id: w.profile_id,
        profile: w.profile,
        library: shown.map(|m| m.library.clone()).or(w.library),
        category: shown.map_or(w.category, |m| m.category.clone()),
        size: present.iter().map(|f| f.size).sum(),
        added_at: Some(w.added_at),
        download: downloads.under_way(w.id),
        cutoff_unmet,
        detail: MovieDetail {
            movie_id: shown.or(folders.first()).map(|m| m.id).or(w.movie_id),
            tmdb_id: Some(w.tmdb_id),
            imdb_id: w.imdb_id.or_else(|| shown.and_then(|m| m.imdb_id.clone())),
            quality: None,
            release_date: w.release_date,
            largest: shown.and_then(|m| files[&m.id].largest()),
        },
    }
}

/// A movie in the library that isn't wanted.
fn library_title(movie: &LibraryRow, present: &Files) -> Title<MovieDetail> {
    Title {
        wanted_id: None,
        title: movie.title.clone(),
        year: movie.year,
        poster: poster(movie.poster_path.clone()),
        state: State::InLibrary,
        monitored: false,
        wanted: false,
        profile_id: None,
        profile: None,
        library: Some(movie.library.clone()),
        category: movie.category.clone(),
        size: present.size,
        added_at: present.first,
        download: None,
        cutoff_unmet: None,
        detail: MovieDetail {
            movie_id: Some(movie.id),
            tmdb_id: movie.match_tmdb_id.or(movie.arr_tmdb_id).or(movie.tmdb_id),
            imdb_id: movie.imdb_id.clone(),
            quality: None,
            release_date: None,
            largest: present.largest(),
        },
    }
}

/// Fills in the quality of each movie's largest file, for the rows on a page only: reading
/// every file's name in a big library takes long enough to be felt.
pub async fn qualities(pool: &SqlitePool, titles: &mut [Title<MovieDetail>]) -> anyhow::Result<()> {
    for title in titles {
        if let Some((id, path)) = &title.detail.largest {
            title.detail.quality = quality(pool, *id, path).await?;
        }
    }
    Ok(())
}

/// The quality of a file, from its name, or from its probe when the name doesn't say; None
/// when neither does.
async fn quality(pool: &SqlitePool, id: i64, path: &str) -> anyhow::Result<Option<String>> {
    let name = crate::search::file_name(path);
    if let Some((_, judged)) = crate::search::judge_file(&name, None) {
        return Ok(Some(judged.quality()));
    }
    let probe: Option<String> = sqlx::query_scalar(
        "SELECT p.info FROM media_probes p JOIN media_files f ON f.id = p.file_id
         WHERE p.file_id = ? AND p.size = f.size AND p.modified_at = f.modified_at",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(crate::search::judge_file(&name, probe.as_deref()).map(|(_, judged)| judged.quality()))
}
