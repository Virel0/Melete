//! What a search is for: a wanted movie, season or episode, loaded with what's known of it, and
//! every monitored one.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct WantedRow {
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) imdb_id: Option<String>,
    pub(super) tmdb_id: Option<i64>,
    pub(super) tvdb_id: Option<i64>,
    pub(super) original_language: Option<String>,
    pub(super) quality_profile: Option<String>,
    pub(super) profile_id: Option<i64>,
    pub(super) movie_id: Option<i64>,
    pub(super) series_id: Option<i64>,
    /// TMDB's original title, when the title is in its cache.
    pub(super) original_title: Option<String>,
}

pub(crate) async fn load_wanted(
    pool: &SqlitePool,
    target: Target,
) -> anyhow::Result<Option<Wanted>> {
    let (query, id) = match target {
        Target::Movie(id) => (
            "SELECT w.title, w.year, w.imdb_id, w.tmdb_id, NULL AS tvdb_id, w.original_language,
                    w.quality_profile, w.profile_id, w.movie_id, NULL AS series_id, t.original_title
             FROM wanted_movies w LEFT JOIN tmdb_movies t ON t.tmdb_id = w.tmdb_id
             WHERE w.id = ?",
            id,
        ),
        Target::Series { id, .. } => (
            "SELECT w.title, w.year, NULL AS imdb_id, w.tmdb_id, w.tvdb_id, w.original_language,
                    w.quality_profile, w.profile_id, NULL AS movie_id, w.series_id, t.original_name AS original_title
             FROM wanted_series w LEFT JOIN tmdb_series t ON t.tmdb_id = w.tmdb_id
             WHERE w.id = ?",
            id,
        ),
    };
    let row: Option<WantedRow> = sqlx::query_as(query).bind(id).fetch_optional(pool).await?;
    Ok(row.map(wanted_from))
}

pub(super) fn wanted_from(row: WantedRow) -> Wanted {
    Wanted {
        titles: std::iter::once(row.title)
            .chain(row.original_title)
            .collect(),
        year: row.year,
        imdb_id: row.imdb_id,
        tmdb_id: row.tmdb_id,
        tvdb_id: row.tvdb_id,
        original_language: row.original_language,
        profile: row.quality_profile,
        profile_id: row.profile_id,
        movie_id: row.movie_id,
        series_id: row.series_id,
    }
}

/// A wanted movie or series, by its id in the wanted list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Title {
    Movie(i64),
    Series(i64),
}

#[derive(sqlx::FromRow)]
pub(super) struct WantedIdRow {
    pub(super) id: i64,
    #[sqlx(flatten)]
    pub(super) row: WantedRow,
}

/// Every monitored wanted title, for checking releases nobody searched for.
pub(crate) async fn monitored_wanted(pool: &SqlitePool) -> anyhow::Result<Vec<(Title, Wanted)>> {
    let movies: Vec<WantedIdRow> = sqlx::query_as(
        "SELECT w.id, w.title, w.year, w.imdb_id, w.tmdb_id, NULL AS tvdb_id, w.original_language,
                w.quality_profile, w.profile_id, w.movie_id, NULL AS series_id, t.original_title
         FROM wanted_movies w LEFT JOIN tmdb_movies t ON t.tmdb_id = w.tmdb_id
         WHERE w.monitored = 1 ORDER BY w.id",
    )
    .fetch_all(pool)
    .await?;
    let series: Vec<WantedIdRow> = sqlx::query_as(
        "SELECT w.id, w.title, w.year, NULL AS imdb_id, w.tmdb_id, w.tvdb_id, w.original_language,
                w.quality_profile, w.profile_id, NULL AS movie_id, w.series_id, t.original_name AS original_title
         FROM wanted_series w LEFT JOIN tmdb_series t ON t.tmdb_id = w.tmdb_id
         WHERE w.monitored = 1 ORDER BY w.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(movies
        .into_iter()
        .map(|r| (Title::Movie(r.id), wanted_from(r.row)))
        .chain(
            series
                .into_iter()
                .map(|r| (Title::Series(r.id), wanted_from(r.row))),
        )
        .collect())
}
