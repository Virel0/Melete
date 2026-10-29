//! The household's libraries, genre counts, and the category and exclusion filters the lists take.

use super::*;

#[derive(serde::Serialize, sqlx::FromRow)]
pub(super) struct BrowseLibrary {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) kind: String,
    /// `anime`, `kids` or null.
    pub(super) category: Option<String>,
    /// Movies or series present.
    pub(super) titles: i64,
}

/// The movies and shows libraries the user may see, with their categories and the titles in
/// each they may see: what an app needs to lay out its rows without guessing from names. No
/// paths; those are an admin's.
pub(super) async fn browse_libraries(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
) -> Result<Json<Vec<BrowseLibrary>>, ApiError> {
    let access = &current.access;
    let libraries = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT l.id, l.name, l.kind, l.category,
             (SELECT count(*) FROM movies m WHERE m.library_id = l.id AND m.missing_since IS NULL
                 AND {movie})
             + (SELECT count(*) FROM series s WHERE s.library_id = l.id AND s.missing_since IS NULL
                 AND {series}) AS titles
         FROM libraries l WHERE l.kind IN ('movies', 'shows') AND {library} ORDER BY l.name",
        movie = access.movie_sql("m"),
        series = access.series_sql("s"),
        library = access.library_sql("l.id"),
    )))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(libraries))
}

#[derive(Deserialize)]
pub(super) struct GenreQuery {
    pub(super) library: Option<String>,
    pub(super) category: Option<String>,
}

/// Every genre of the movies and series present, with how many of each have it, most titles
/// first: one request for a genre row, as `/music/genres` is for music. Genres come from
/// TMDB, so only matched titles have any.
pub(super) async fn browse_genres(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<GenreQuery>,
) -> Result<Json<Value>, ApiError> {
    let access = &current.access;
    let category = category_filter(query.category.as_deref())?;
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH titled AS (
             SELECT 'movie' AS kind, x.id, t.genres FROM movies x
             JOIN libraries l ON l.id = x.library_id
             JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.missing_since IS NULL AND (?1 IS NULL OR l.name = ?1) AND {category}
                 AND {movie}
             UNION ALL
             SELECT 'series', x.id, t.genres FROM series x
             JOIN libraries l ON l.id = x.library_id
             JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
             WHERE x.missing_since IS NULL AND (?1 IS NULL OR l.name = ?1) AND {category}
                 AND {series}
         )
         SELECT min(g.value),
             count(DISTINCT CASE WHEN titled.kind = 'movie' THEN titled.id END),
             count(DISTINCT CASE WHEN titled.kind = 'series' THEN titled.id END)
         FROM titled, json_each(titled.genres) g
         GROUP BY lower(g.value)
         ORDER BY count(DISTINCT titled.kind || titled.id) DESC, lower(g.value)",
        category = category_clause(2),
        movie = access.movie_sql("x"),
        series = access.series_sql("x"),
    )))
    .bind(&query.library)
    .bind(category)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!(
        rows.iter()
            .map(
                |(name, movies, series)| json!({ "name": name, "movies": movies, "series": series })
            )
            .collect::<Vec<_>>()
    )))
}

/// The `category` filter: titles in libraries of that category, or with `none`, in libraries
/// without one. Checked here, so a typo is a 400 rather than an empty list.
pub(in super::super) fn category_filter(category: Option<&str>) -> Result<Option<&str>, ApiError> {
    match category {
        None | Some("anime" | "kids" | "none") => Ok(category),
        Some(_) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "category is anime, kids or none",
        )),
    }
}

/// SQL that holds when series `series`, in library `library`, is anime: wanted as anime (series
/// type `anime`), or in a library with the anime category. Aliases are constants, never input.
pub(in super::super) fn anime_sql(series: &str, library: &str) -> String {
    format!(
        "({library}.category IS 'anime' OR EXISTS (SELECT 1 FROM wanted_series w
             WHERE w.series_id = {series}.id AND w.series_type = 'anime'))"
    )
}

/// The `anime` filter a series list takes: `only` for anime alone, `exclude` to leave it out.
/// Some(true) or Some(false) for the anime condition to hold or not; None for no filter.
pub(super) fn anime_filter(anime: Option<&str>) -> Result<Option<bool>, ApiError> {
    match anime {
        None => Ok(None),
        Some("only") => Ok(Some(true)),
        Some("exclude") => Ok(Some(false)),
        Some(_) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "anime is only or exclude",
        )),
    }
}

/// Most entries an `exclude_*` list takes.
pub(super) const MAX_EXCLUDED: usize = 100;

/// What a browse query leaves out: series by id, libraries by name and categories, each
/// a comma-separated list. Kept as JSON arrays, bound as three parameters.
pub(in super::super) struct Exclusions {
    /// Series ids, as a JSON array.
    pub series: String,
    /// Library names, as a JSON array.
    pub libraries: String,
    /// Categories, as a JSON array.
    pub categories: String,
}

impl Exclusions {
    pub(in super::super) fn parse(
        series: Option<&str>,
        libraries: Option<&str>,
        categories: Option<&str>,
    ) -> Result<Self, ApiError> {
        let split = |list: Option<&str>| -> Result<Vec<String>, ApiError> {
            let items: Vec<String> = list
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect();
            if items.len() > MAX_EXCLUDED {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("an exclude list takes at most {MAX_EXCLUDED} entries"),
                ));
            }
            Ok(items)
        };
        let series = split(series)?
            .iter()
            .map(|id| id.parse::<i64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| {
                ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "exclude_series is series ids, separated by commas",
                )
            })?;
        let categories = split(categories)?;
        if categories.iter().any(|c| c != "anime" && c != "kids") {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "exclude_category is anime or kids, separated by commas",
            ));
        }
        Ok(Self {
            series: serde_json::to_string(&series)?,
            libraries: serde_json::to_string(&split(libraries)?)?,
            categories: serde_json::to_string(&categories)?,
        })
    }

    /// SQL that holds when a title passes: its series (`series`, an expression that is NULL
    /// for movies) isn't excluded, nor its library `l`, by name or category. The lists are
    /// bound at `?{n}`, `?{n + 1}` and `?{n + 2}`: series, libraries, categories.
    pub(in super::super) fn clause(series: &str, n: u8) -> String {
        let (libraries, categories) = (n + 1, n + 2);
        format!(
            "({series} IS NULL OR {series} NOT IN (SELECT value FROM json_each(?{n})))
             AND l.name NOT IN (SELECT value FROM json_each(?{libraries}))
             AND (l.category IS NULL
                  OR l.category NOT IN (SELECT value FROM json_each(?{categories})))"
        )
    }
}

/// SQL that holds when library `l` passes the `category` filter bound at `?{n}`.
pub(in super::super) fn category_clause(n: u8) -> String {
    format!("(?{n} IS NULL OR (?{n} = 'none' AND l.category IS NULL) OR l.category = ?{n})")
}
