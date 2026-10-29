//! The movie and series lists: searched, sorted, filtered and paged.

use super::*;

/// Table and column names for the two kinds of title. Constants only, never user input.
pub(super) struct Kind {
    pub(super) titles: &'static str,
    pub(super) metadata: &'static str,
    pub(super) metadata_title: &'static str,
    pub(super) file_column: &'static str,
}

pub(super) const MOVIES: Kind = Kind {
    titles: "movies",
    metadata: "tmdb_movies",
    metadata_title: "title",
    file_column: "movie_id",
};

pub(super) const SERIES: Kind = Kind {
    titles: "series",
    metadata: "tmdb_series",
    metadata_title: "name",
    file_column: "series_id",
};

#[derive(Deserialize)]
pub(super) struct ListQuery {
    pub(super) library: Option<String>,
    /// `anime`, `kids`, or `none` for libraries without a category.
    pub(super) category: Option<String>,
    /// Part of the title or original title; case, accents, spaces and punctuation aside.
    pub(super) q: Option<String>,
    /// Only titles TMDB gives this genre, case aside.
    pub(super) genre: Option<String>,
    /// Series ids to leave out, separated by commas; series lists only.
    pub(super) exclude_series: Option<String>,
    /// Library names to leave out, separated by commas.
    pub(super) exclude_library: Option<String>,
    /// Categories to leave out, separated by commas.
    pub(super) exclude_category: Option<String>,
    /// `only` for anime alone, `exclude` to leave anime out; series lists only.
    pub(super) anime: Option<String>,
    /// `title` (default), `year` (newest first) or `recent` (newest file first).
    pub(super) sort: Option<String>,
    pub(super) limit: Option<i64>,
    pub(super) offset: Option<i64>,
}

pub(super) async fn list_movies(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    list(
        &state.pool,
        &current.access,
        current.user.id,
        &MOVIES,
        query,
    )
    .await
}

pub(super) async fn list_series(
    State(state): State<MediaState>,
    Extension(current): Extension<CurrentUser>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    list(
        &state.pool,
        &current.access,
        current.user.id,
        &SERIES,
        query,
    )
    .await
}

/// A title's year, its own or else the one its folder name gives.
const YEAR: &str = "COALESCE(t.year, x.parsed_year)";

/// The ORDER BY a list's `sort` asks for.
fn order(kind: &Kind, sort: Option<&str>) -> Result<String, ApiError> {
    let Kind {
        metadata_title,
        file_column,
        ..
    } = kind;
    // The title as sorted: without a leading article, accents and punctuation aside. A title
    // whose key isn't worked out yet sorts as it is meanwhile.
    let sorted = format!("COALESCE(t.sort_key, t.{metadata_title}, x.sort_key, x.parsed_title)");
    Ok(match sort.unwrap_or("title") {
        "title" => format!("{sorted} COLLATE NOCASE, x.id"),
        "year" => format!("{YEAR} IS NULL, {YEAR} DESC, {sorted} COLLATE NOCASE, x.id"),
        "recent" => format!(
            "(SELECT max(coalesce(f.added_at, f.modified_at)) FROM media_files f
              WHERE f.{file_column} = x.id AND f.missing_since IS NULL) DESC, x.id DESC"
        ),
        _ => {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "sort is title, year or recent",
            ));
        }
    })
}

/// The titles of a kind the user may see, searched, filtered, sorted and paged, each with when
/// it came in and, for a series, the episodes `me` hasn't watched.
pub(super) async fn list(
    pool: &SqlitePool,
    access: &crate::access::Access,
    me: i64,
    kind: &Kind,
    query: ListQuery,
) -> Result<Json<Value>, ApiError> {
    let (limit, offset) = page(&query)?;
    let category = category_filter(query.category.as_deref())?;
    let anime = anime_filter(query.anime.as_deref())?;
    if anime.is_some() && kind.titles != "series" {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "anime is for series lists only",
        ));
    }
    let excluded = Exclusions::parse(
        query.exclude_series.as_deref(),
        query.exclude_library.as_deref(),
        query.exclude_category.as_deref(),
    )?;
    let title = title_sql(kind);
    let year = YEAR;
    let order = order(kind, query.sort.as_deref())?;
    let from = from_sql(kind, access);
    let pattern = query.q.as_deref().map(|q| format!("%{}%", escape_like(q)));
    let folded = crate::search_keys::pattern(query.q.as_deref());

    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {from}")))
        .bind(&query.library)
        .bind(&pattern)
        .bind(category)
        .bind(&query.genre)
        .bind(&excluded.series)
        .bind(&excluded.libraries)
        .bind(&excluded.categories)
        .bind(&folded)
        .bind(anime)
        .fetch_one(pool)
        .await?;
    let rows: Vec<ListRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT x.id, {title} AS title, {year} AS year,
             COALESCE(x.local_poster_key, t.poster_path) AS poster_path, x.match_status
             {from} ORDER BY {order} LIMIT ?10 OFFSET ?11"
    )))
    .bind(&query.library)
    .bind(&pattern)
    .bind(category)
    .bind(&query.genre)
    .bind(&excluded.series)
    .bind(&excluded.libraries)
    .bind(&excluded.categories)
    .bind(&folded)
    .bind(anime)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let mut items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "title": row.title,
                "year": row.year,
                "poster": artwork_url("w342", row.poster_path),
                "matched": row.match_status.as_deref() == Some("matched"),
            })
        })
        .collect();
    add_list_fields(pool, me, kind.titles == "series", &mut items).await?;
    Ok(Json(json!({ "total": total, "items": items })))
}

/// A list's limit and offset, checked.
fn page(query: &ListQuery) -> Result<(i64, i64), ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    let offset = query.offset.unwrap_or(0);
    if !(1..=MAX_LIMIT).contains(&limit) || offset < 0 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "limit is 1 to 500 and offset is 0 or more",
        ));
    }
    Ok((limit, offset))
}

/// A title as listed: its metadata's, or else the one its folder name gives. Spelled out
/// rather than using the SELECT aliases: inside an expression SQLite would read `year` as
/// tmdb's own column, which is NULL for unmatched titles.
fn title_sql(kind: &Kind) -> String {
    format!("COALESCE(t.{}, x.parsed_title)", kind.metadata_title)
}

/// The FROM and WHERE of a list of a kind the user may see. Its parameters, ?1 to ?9: the
/// library, the title pattern, the category, the genre, the three exclusions, the folded search
/// key and the anime filter.
fn from_sql(kind: &Kind, access: &crate::access::Access) -> String {
    let Kind {
        titles, metadata, ..
    } = kind;
    let title = title_sql(kind);
    format!(
        "FROM {titles} x
         JOIN libraries l ON l.id = x.library_id
         LEFT JOIN {metadata} t ON t.tmdb_id = x.match_tmdb_id
         WHERE x.missing_since IS NULL
           AND (?1 IS NULL OR l.name = ?1)
           AND (?2 IS NULL OR {title} LIKE ?2 ESCAPE '\\'
                OR COALESCE(t.search_key, x.search_key) LIKE ?8)
           AND {}
           AND (?4 IS NULL OR EXISTS (SELECT 1 FROM json_each(t.genres) g
                                      WHERE lower(g.value) = lower(?4)))
           AND {}
           AND {}
           AND (?9 IS NULL OR {} = ?9)",
        category_clause(3),
        Exclusions::clause(if *titles == "series" { "x.id" } else { "NULL" }, 5),
        if *titles == "series" {
            access.series_sql("x")
        } else {
            access.movie_sql("x")
        },
        // Movies never get here with a filter; any condition does for the unbound clause.
        if *titles == "series" {
            anime_sql("x", "l")
        } else {
            "0".to_owned()
        }
    )
}

#[derive(sqlx::FromRow)]
pub(super) struct ListRow {
    pub(super) id: i64,
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) poster_path: Option<String>,
    pub(super) match_status: Option<String>,
}
