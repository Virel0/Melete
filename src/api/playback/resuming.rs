//! What the user is partway through, and their progress on one movie or series.

use super::*;

/// A file the user is partway through, with its title.
#[derive(sqlx::FromRow)]
pub(in super::super) struct ContinueRow {
    /// The file.
    pub file_id: i64,
    /// Where playback was last reported, in seconds.
    pub position_seconds: f64,
    /// The file's length in seconds, when a player said.
    pub duration_seconds: Option<f64>,
    /// When progress was last reported, in Unix seconds.
    pub updated_at: i64,
    /// The movie, for a movie's file.
    pub movie_id: Option<i64>,
    /// The series, for an episode's file.
    pub series_id: Option<i64>,
    /// The episode's season.
    pub season: Option<i64>,
    /// The episode's number.
    pub episode: Option<i64>,
    /// The movie's or series' title.
    pub title: String,
    /// The movie's or series' year.
    pub year: Option<i64>,
    /// The episode's name, from TMDB.
    pub episode_name: Option<String>,
    /// The title's poster on TMDB's image server.
    pub poster_path: Option<String>,
    /// The episode's still on TMDB's image server.
    pub still_path: Option<String>,
}

/// What `/me/continue` and `/me/next-up` leave out, each a comma-separated list.
#[derive(Deserialize)]
pub(in super::super) struct ExcludeQuery {
    /// Series ids to leave out.
    pub exclude_series: Option<String>,
    /// Library names to leave out.
    pub exclude_library: Option<String>,
    /// Categories to leave out.
    pub exclude_category: Option<String>,
}

impl ExcludeQuery {
    pub(in super::super) fn parse(&self) -> Result<Exclusions, ApiError> {
        Exclusions::parse(
            self.exclude_series.as_deref(),
            self.exclude_library.as_deref(),
            self.exclude_category.as_deref(),
        )
    }
}

/// Files the user is partway through, most recent first.
pub(super) async fn continue_watching(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ExcludeQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let excluded = query.parse()?;
    let rows = continue_rows(
        &pool,
        current.user.id,
        &current.access,
        &excluded,
        CONTINUE_LIMIT,
    )
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                json!({
                    "file_id": r.file_id,
                    "position_seconds": r.position_seconds,
                    "duration_seconds": r.duration_seconds,
                    "updated_at": r.updated_at,
                    "movie_id": r.movie_id,
                    "series_id": r.series_id,
                    "season": r.season,
                    "episode": r.episode,
                    "title": r.title,
                    "year": r.year,
                    "episode_name": r.episode_name,
                    "poster": artwork_url("w342", r.poster_path),
                    "still": artwork_url("w300", r.still_path),
                })
            })
            .collect(),
    ))
}

/// Up to `limit` files `user_id` is partway through, of what `access` sees, most recent
/// first, leaving out what
/// `excluded` names.
pub(in super::super) async fn continue_rows(
    pool: &SqlitePool,
    user_id: i64,
    access: &crate::access::Access,
    excluded: &Exclusions,
    limit: i64,
) -> sqlx::Result<Vec<ContinueRow>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT p.file_id, p.position_seconds, p.duration_seconds, p.updated_at,
             f.movie_id, f.series_id, f.season, f.episode,
             COALESCE(tm.title, m.parsed_title, ts.name, s.parsed_title) AS title,
             COALESCE(tm.year, m.parsed_year, ts.year, s.parsed_year) AS year,
             te.name AS episode_name,
             COALESCE(m.local_poster_key, tm.poster_path, s.local_poster_key, ts.poster_path)
                 AS poster_path,
             te.still_path
         FROM playback_progress p
         JOIN media_files f ON f.id = p.file_id
         JOIN libraries l ON l.id = f.library_id
         LEFT JOIN movies m ON m.id = f.movie_id
         LEFT JOIN tmdb_movies tm ON tm.tmdb_id = m.match_tmdb_id
         LEFT JOIN series s ON s.id = f.series_id
         LEFT JOIN tmdb_series ts ON ts.tmdb_id = s.match_tmdb_id
         LEFT JOIN tmdb_episodes te ON te.series_tmdb_id = ts.tmdb_id
             AND te.season = f.season AND te.episode = f.episode
         WHERE p.user_id = ?1 AND p.position_seconds > 0 AND f.missing_since IS NULL
           AND {} AND {}
         ORDER BY p.updated_at DESC, p.file_id DESC
         LIMIT ?2",
        Exclusions::clause("f.series_id", 3),
        access.file_sql("f"),
    )))
    .bind(user_id)
    .bind(limit)
    .bind(&excluded.series)
    .bind(&excluded.libraries)
    .bind(&excluded.categories)
    .fetch_all(pool)
    .await
}

#[derive(Deserialize)]
pub(super) struct ProgressQuery {
    pub(super) movie_id: Option<i64>,
    pub(super) series_id: Option<i64>,
}

#[derive(sqlx::FromRow)]
pub(super) struct ProgressRow {
    pub(super) file_id: i64,
    #[sqlx(flatten)]
    pub(super) progress: Progress,
    pub(super) updated_at: i64,
}

/// The user's progress on every file of one movie or series, to mark what's been watched.
pub(super) async fn progress(
    State(pool): State<SqlitePool>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ProgressQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    if query.movie_id.is_some() == query.series_id.is_some() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "give exactly one of movie_id or series_id",
        ));
    }
    let rows: Vec<ProgressRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT p.file_id, p.position_seconds, p.duration_seconds, p.watched, p.play_count,
             p.updated_at
         FROM playback_progress p JOIN media_files f ON f.id = p.file_id
         WHERE p.user_id = ? AND (f.movie_id = ? OR f.series_id = ?) AND f.missing_since IS NULL
             AND {}
         ORDER BY p.file_id",
        current.access.file_sql("f")
    )))
    .bind(current.user.id)
    .bind(query.movie_id)
    .bind(query.series_id)
    .fetch_all(&pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| {
                let mut entry = progress_json(&r.progress);
                entry["file_id"] = json!(r.file_id);
                entry["updated_at"] = json!(r.updated_at);
                entry
            })
            .collect(),
    ))
}
