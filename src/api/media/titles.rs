//! A movie, a series and a season with their files and episodes.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct FileRow {
    pub(super) id: i64,
    pub(super) rel_path: String,
    pub(super) size: i64,
    pub(super) modified_at: i64,
}

impl FileRow {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "path": self.rel_path,
            "size": self.size,
            "modified_at": self.modified_at,
        })
    }
}

#[derive(sqlx::FromRow)]
pub(super) struct MovieRow {
    pub(super) id: i64,
    pub(super) library: String,
    pub(super) title: String,
    pub(super) original_title: Option<String>,
    pub(super) year: Option<i64>,
    pub(super) overview: Option<String>,
    pub(super) runtime_minutes: Option<i64>,
    pub(super) genres: Option<String>,
    pub(super) tmdb_id: Option<i64>,
    pub(super) imdb_id: Option<String>,
    pub(super) poster_path: Option<String>,
    pub(super) backdrop_path: Option<String>,
    pub(super) match_status: Option<String>,
}

pub(super) async fn movie(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if !current.access.sees_movie(&state.pool, id).await? {
        return Err(not_found("movie"));
    }
    let row: Option<MovieRow> = sqlx::query_as(
        "SELECT m.id, l.name AS library, COALESCE(t.title, m.parsed_title) AS title,
             t.original_title, COALESCE(t.year, m.parsed_year) AS year, t.overview,
             t.runtime_minutes, t.genres, t.tmdb_id, COALESCE(t.imdb_id, m.imdb_id) AS imdb_id,
             COALESCE(m.local_poster_key, t.poster_path) AS poster_path,
             COALESCE(m.local_backdrop_key, t.backdrop_path) AS backdrop_path, m.match_status
         FROM movies m
         JOIN libraries l ON l.id = m.library_id
         LEFT JOIN tmdb_movies t ON t.tmdb_id = m.match_tmdb_id
         WHERE m.id = ? AND m.missing_since IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(m) = row else {
        return Err(not_found("movie"));
    };
    let files: Vec<FileRow> = sqlx::query_as(
        "SELECT id, rel_path, size, modified_at FROM media_files
         WHERE movie_id = ? AND missing_since IS NULL ORDER BY rel_path",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut body = json!({
        "id": m.id,
        "library": m.library,
        "title": m.title,
        "original_title": m.original_title,
        "year": m.year,
        "overview": m.overview,
        "runtime_minutes": m.runtime_minutes,
        "genres": genres(m.genres)?,
        "tmdb_id": m.tmdb_id,
        "imdb_id": m.imdb_id,
        "poster": artwork_url("w500", m.poster_path),
        "backdrop": artwork_url("w1280", m.backdrop_path),
        "match_status": m.match_status,
        "files": files.iter().map(FileRow::to_json).collect::<Vec<_>>(),
    });
    add_extras(&state, false, id, &mut body).await?;
    Ok(Json(body))
}

/// The textless backdrop, logo and each backdrop's focus, as `/artwork` gives them.
pub(super) async fn add_extras(
    state: &MediaState,
    series: bool,
    id: i64,
    body: &mut Value,
) -> Result<(), ApiError> {
    let found = extras(&state.title_artwork, series, id).await?;
    for field in [
        "textless_backdrop",
        "logo",
        "backdrop_focus",
        "textless_backdrop_focus",
    ] {
        body[field] = found
            .as_ref()
            .map_or(Value::Null, |found| found[field].clone());
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
pub(super) struct SeriesRow {
    pub(super) id: i64,
    pub(super) library: String,
    pub(super) title: String,
    pub(super) original_title: Option<String>,
    pub(super) year: Option<i64>,
    pub(super) overview: Option<String>,
    pub(super) status: Option<String>,
    pub(super) genres: Option<String>,
    pub(super) tmdb_id: Option<i64>,
    pub(super) tvdb_id: Option<i64>,
    pub(super) imdb_id: Option<String>,
    pub(super) poster_path: Option<String>,
    pub(super) backdrop_path: Option<String>,
    pub(super) match_status: Option<String>,
}

pub(super) async fn series(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    if !current.access.sees_series(&state.pool, id).await? {
        return Err(not_found("series"));
    }
    let row: Option<SeriesRow> = sqlx::query_as(
        "SELECT s.id, l.name AS library, COALESCE(t.name, s.parsed_title) AS title,
             t.original_name AS original_title, COALESCE(t.year, s.parsed_year) AS year,
             t.overview, t.status, t.genres, t.tmdb_id, COALESCE(t.tvdb_id, s.tvdb_id) AS tvdb_id,
             COALESCE(t.imdb_id, s.imdb_id) AS imdb_id,
             COALESCE(s.local_poster_key, t.poster_path) AS poster_path,
             COALESCE(s.local_backdrop_key, t.backdrop_path) AS backdrop_path,
             s.match_status
         FROM series s
         JOIN libraries l ON l.id = s.library_id
         LEFT JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
         WHERE s.id = ? AND s.missing_since IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(s) = row else {
        return Err(not_found("series"));
    };

    let known: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT season, count(*) FROM tmdb_episodes WHERE series_tmdb_id = ? GROUP BY season",
    )
    .bind(s.tmdb_id)
    .fetch_all(&state.pool)
    .await?;
    let numbered: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT season, episode, episode_end FROM media_files
         WHERE series_id = ? AND season IS NOT NULL AND missing_since IS NULL",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    let mut available: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for (season, first, last) in numbered {
        available
            .entry(season)
            .or_default()
            .extend(episode_range(first, last));
    }
    let known: BTreeMap<i64, i64> = known.into_iter().collect();
    let numbers: BTreeSet<i64> = known.keys().chain(available.keys()).copied().collect();
    let seasons: Vec<Value> = numbers
        .into_iter()
        .map(|n| {
            json!({
                "season": n,
                "episodes": known.get(&n).copied().unwrap_or(0),
                "available": available.get(&n).map_or(0, BTreeSet::len),
            })
        })
        .collect();

    let unparsed: Vec<FileRow> = sqlx::query_as(
        "SELECT id, rel_path, size, modified_at FROM media_files
         WHERE series_id = ? AND season IS NULL AND missing_since IS NULL ORDER BY rel_path",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let mut body = json!({
        "id": s.id,
        "library": s.library,
        "title": s.title,
        "original_title": s.original_title,
        "year": s.year,
        "overview": s.overview,
        "status": s.status,
        "genres": genres(s.genres)?,
        "tmdb_id": s.tmdb_id,
        "tvdb_id": s.tvdb_id,
        "imdb_id": s.imdb_id,
        "poster": artwork_url("w500", s.poster_path),
        "backdrop": artwork_url("w1280", s.backdrop_path),
        "match_status": s.match_status,
        "seasons": seasons,
        // Files whose names gave no season and episode, so apps can still offer them.
        "unparsed_files": unparsed.iter().map(FileRow::to_json).collect::<Vec<_>>(),
    });
    add_extras(&state, true, id, &mut body).await?;
    Ok(Json(body))
}

pub(super) fn episode_range(first: i64, last: Option<i64>) -> std::ops::RangeInclusive<i64> {
    first..=last.unwrap_or(first).min(first + MAX_EPISODES_PER_FILE - 1)
}

#[derive(sqlx::FromRow)]
pub(super) struct EpisodeRow {
    pub(super) episode: i64,
    pub(super) name: Option<String>,
    pub(super) overview: Option<String>,
    pub(super) air_date: Option<String>,
    pub(super) runtime_minutes: Option<i64>,
    pub(super) still_path: Option<String>,
}

#[derive(sqlx::FromRow)]
pub(super) struct EpisodeFileRow {
    #[sqlx(flatten)]
    pub(super) file: FileRow,
    pub(super) episode: i64,
    pub(super) episode_end: Option<i64>,
}

/// A season's episodes: TMDB's list joined with the files on disk. Episodes TMDB doesn't
/// know still appear when there is a file for them.
pub(super) async fn season(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Path((id, season)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    if !current.access.sees_series(&state.pool, id).await? {
        return Err(not_found("series"));
    }
    let tmdb_id: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT match_tmdb_id FROM series WHERE id = ? AND missing_since IS NULL",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(tmdb_id) = tmdb_id else {
        return Err(not_found("series"));
    };
    let episodes: Vec<EpisodeRow> = sqlx::query_as(
        "SELECT episode, name, overview, air_date, runtime_minutes, still_path FROM tmdb_episodes
         WHERE series_tmdb_id = ? AND season = ? ORDER BY episode",
    )
    .bind(tmdb_id)
    .bind(season)
    .fetch_all(&state.pool)
    .await?;
    let files: Vec<EpisodeFileRow> = sqlx::query_as(
        "SELECT id, rel_path, size, modified_at, episode, episode_end FROM media_files
         WHERE series_id = ? AND season = ? AND missing_since IS NULL ORDER BY episode, rel_path",
    )
    .bind(id)
    .bind(season)
    .fetch_all(&state.pool)
    .await?;
    if episodes.is_empty() && files.is_empty() {
        return Err(not_found("season"));
    }

    let mut out: BTreeMap<i64, Value> = episodes
        .into_iter()
        .map(|e| {
            let episode = json!({
                "episode": e.episode,
                "name": e.name,
                "overview": e.overview,
                "air_date": e.air_date,
                "runtime_minutes": e.runtime_minutes,
                "still": artwork_url("w300", e.still_path),
                "files": [],
            });
            (e.episode, episode)
        })
        .collect();
    for row in &files {
        for n in episode_range(row.episode, row.episode_end) {
            let entry = out.entry(n).or_insert_with(|| {
                json!({
                    "episode": n,
                    "name": null,
                    "overview": null,
                    "air_date": null,
                    "runtime_minutes": null,
                    "still": null,
                    "files": [],
                })
            });
            entry["files"]
                .as_array_mut()
                .expect("every entry is made above with a files array")
                .push(row.file.to_json());
        }
    }
    Ok(Json(json!({
        "season": season,
        "episodes": out.into_values().collect::<Vec<_>>(),
    })))
}
