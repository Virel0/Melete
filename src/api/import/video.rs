//! Every wanted series, with its seasons and unmonitored episodes, and every wanted movie.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct WantedSeriesRow {
    pub(super) id: i64,
    pub(super) tvdb_id: i64,
    pub(super) tmdb_id: Option<i64>,
    pub(super) imdb_id: Option<String>,
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) monitored: bool,
    pub(super) series_type: String,
    pub(super) quality_profile: Option<String>,
    pub(super) profile_id: Option<i64>,
    pub(super) library: Option<String>,
    pub(super) series_id: Option<i64>,
    pub(super) source: String,
}

/// Every wanted series with its seasons and the episodes left unmonitored in monitored seasons.
pub(super) async fn wanted_series(
    State(state): State<ImportState>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<WantedSeriesRow> = sqlx::query_as(
        "SELECT w.id, w.tvdb_id, w.tmdb_id, w.imdb_id, w.title, w.year, w.monitored,
             w.series_type, w.quality_profile, w.profile_id, l.name AS library, w.series_id, w.source
         FROM wanted_series w LEFT JOIN libraries l ON l.id = w.library_id
         ORDER BY w.title COLLATE NOCASE, w.id",
    )
    .fetch_all(&state.pool)
    .await?;
    let seasons: Vec<(i64, i64, bool)> = sqlx::query_as(
        "SELECT wanted_series_id, season, monitored FROM wanted_seasons ORDER BY season",
    )
    .fetch_all(&state.pool)
    .await?;
    let overrides: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT wanted_series_id, season, episode FROM wanted_episode_overrides
         WHERE monitored = 0 ORDER BY season, episode",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut seasons_of: HashMap<i64, Vec<Value>> = HashMap::new();
    for (id, season, monitored) in seasons {
        seasons_of
            .entry(id)
            .or_default()
            .push(json!({ "season": season, "monitored": monitored }));
    }
    let mut overrides_of: HashMap<i64, Vec<Value>> = HashMap::new();
    for (id, season, episode) in overrides {
        overrides_of
            .entry(id)
            .or_default()
            .push(json!({ "season": season, "episode": episode }));
    }
    Ok(Json(
        rows.into_iter()
            .map(|w| {
                json!({
                    "id": w.id,
                    "tvdb_id": w.tvdb_id,
                    "tmdb_id": w.tmdb_id,
                    "imdb_id": w.imdb_id,
                    "title": w.title,
                    "year": w.year,
                    "monitored": w.monitored,
                    "series_type": w.series_type,
                    "quality_profile": w.quality_profile,
                    "profile_id": w.profile_id,
                    "library": w.library,
                    "series_id": w.series_id,
                    "source": w.source,
                    "seasons": seasons_of.remove(&w.id).unwrap_or_default(),
                    "unmonitored_episodes": overrides_of.remove(&w.id).unwrap_or_default(),
                })
            })
            .collect(),
    ))
}

#[derive(sqlx::FromRow)]
pub(super) struct WantedMovieRow {
    pub(super) id: i64,
    pub(super) tmdb_id: i64,
    pub(super) imdb_id: Option<String>,
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) monitored: bool,
    pub(super) quality_profile: Option<String>,
    pub(super) profile_id: Option<i64>,
    pub(super) minimum_availability: Option<String>,
    pub(super) library: Option<String>,
    pub(super) movie_id: Option<i64>,
    pub(super) source: String,
}

pub(super) async fn wanted_movies(
    State(state): State<ImportState>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<WantedMovieRow> = sqlx::query_as(
        "SELECT w.id, w.tmdb_id, w.imdb_id, w.title, w.year, w.monitored, w.quality_profile,
             w.profile_id, w.minimum_availability, l.name AS library, w.movie_id, w.source
         FROM wanted_movies w LEFT JOIN libraries l ON l.id = w.library_id
         ORDER BY w.title COLLATE NOCASE, w.id",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|w| {
                json!({
                    "id": w.id,
                    "tmdb_id": w.tmdb_id,
                    "imdb_id": w.imdb_id,
                    "title": w.title,
                    "year": w.year,
                    "monitored": w.monitored,
                    "quality_profile": w.quality_profile,
                    "profile_id": w.profile_id,
                    "minimum_availability": w.minimum_availability,
                    "library": w.library,
                    "movie_id": w.movie_id,
                    "source": w.source,
                })
            })
            .collect(),
    ))
}
