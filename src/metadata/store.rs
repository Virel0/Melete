//! Recording match attempts and storing what TMDB sent.

use super::*;

/// Records an automatic match attempt, like [`set_match`], but only if the title still has the
/// *arr ids the refresh read: an import that changed them meanwhile reset the title, and the
/// next refresh redoes it with the new ids. Never touches manual decisions. `false` if skipped.
pub(super) async fn set_auto_match(
    pool: &SqlitePool,
    kind: Kind,
    row: &TitleRow,
    matched: Option<(i64, &str)>,
    candidates: Option<&str>,
    now: i64,
) -> anyhow::Result<bool> {
    let status = match (matched, candidates) {
        (Some(_), _) => "matched",
        (None, Some(_)) => "ambiguous",
        (None, None) => "unmatched",
    };
    let query = match kind {
        Kind::Movie => {
            "UPDATE movies SET match_status = ?, match_tmdb_id = ?, match_method = ?,
                 match_candidates = ?, match_attempted_at = ?
             WHERE id = ? AND match_method IS NOT 'manual'
               AND arr_tmdb_id IS ? AND ? IS NULL"
        }
        Kind::Series => {
            "UPDATE series SET match_status = ?, match_tmdb_id = ?, match_method = ?,
                 match_candidates = ?, match_attempted_at = ?
             WHERE id = ? AND match_method IS NOT 'manual'
               AND arr_tmdb_id IS ? AND arr_tvdb_id IS ?"
        }
    };
    let written = sqlx::query(query)
        .bind(status)
        .bind(matched.map(|(tmdb_id, _)| tmdb_id))
        .bind(matched.map(|(_, method)| method))
        .bind(candidates)
        .bind(now)
        .bind(row.id)
        .bind(row.arr_tmdb_id)
        .bind(row.arr_tvdb_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(written > 0)
}

/// Records a match attempt: `matched` with an id, else `ambiguous` with candidates, else `unmatched`.
///
/// Only a manual match may replace a manual decision. A refresh picks its titles at the start
/// and can reach one after an admin decided it by hand; that write is skipped and `false`
/// returned.
pub(super) async fn set_match(
    pool: &SqlitePool,
    kind: Kind,
    id: i64,
    matched: Option<(i64, &str)>,
    candidates: Option<&str>,
    now: i64,
) -> anyhow::Result<bool> {
    let status = match (matched, candidates) {
        (Some(_), _) => "matched",
        (None, Some(_)) => "ambiguous",
        (None, None) => "unmatched",
    };
    let query = match kind {
        Kind::Movie => {
            "UPDATE movies SET match_status = ?, match_tmdb_id = ?, match_method = ?,
                 match_candidates = ?, match_attempted_at = ?
             WHERE id = ? AND (? = 'manual' OR match_method IS NOT 'manual')"
        }
        Kind::Series => {
            "UPDATE series SET match_status = ?, match_tmdb_id = ?, match_method = ?,
                 match_candidates = ?, match_attempted_at = ?
             WHERE id = ? AND (? = 'manual' OR match_method IS NOT 'manual')"
        }
    };
    let method = matched.map(|(_, method)| method);
    let written = sqlx::query(query)
        .bind(status)
        .bind(matched.map(|(tmdb_id, _)| tmdb_id))
        .bind(method)
        .bind(candidates)
        .bind(now)
        .bind(id)
        .bind(method)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(written > 0)
}

pub(super) async fn store_movie(
    pool: &SqlitePool,
    d: &MovieDetails,
    now: i64,
) -> anyhow::Result<()> {
    let genres: Vec<&str> = d.genres.iter().map(|g| g.name.as_str()).collect();
    if let Some(c) = &d.belongs_to_collection {
        sqlx::query(
            "INSERT INTO tmdb_collections (id, name, poster_path, backdrop_path)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name,
                 poster_path = excluded.poster_path, backdrop_path = excluded.backdrop_path",
        )
        .bind(c.id)
        .bind(&c.name)
        .bind(non_empty(&c.poster_path))
        .bind(non_empty(&c.backdrop_path))
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, imdb_id, title, original_title, release_date, year,
             overview, runtime_minutes, poster_path, backdrop_path, genres, fetched_at,
             language, english_fields, collection_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (tmdb_id) DO UPDATE SET
             imdb_id = excluded.imdb_id, title = excluded.title,
             original_title = excluded.original_title, release_date = excluded.release_date,
             year = excluded.year, overview = excluded.overview,
             runtime_minutes = excluded.runtime_minutes, poster_path = excluded.poster_path,
             backdrop_path = excluded.backdrop_path, genres = excluded.genres,
             fetched_at = excluded.fetched_at,
             -- A new language picks its logo again.
             images_fetched_at = CASE WHEN coalesce(tmdb_movies.language, 'en-US') = excluded.language
                 THEN tmdb_movies.images_fetched_at END,
             language = excluded.language, english_fields = excluded.english_fields,
             collection_id = excluded.collection_id",
    )
    .bind(d.id)
    .bind(non_empty(&d.imdb_id))
    .bind(&d.title)
    .bind(non_empty(&d.original_title))
    .bind(non_empty(&d.release_date))
    .bind(year_of(d.release_date.as_deref()))
    .bind(non_empty(&d.overview))
    .bind(d.runtime.filter(|&r| r > 0))
    .bind(non_empty(&d.poster_path))
    .bind(non_empty(&d.backdrop_path))
    .bind(serde_json::to_string(&genres)?)
    .bind(now)
    .bind(&d.language)
    .bind(english_fields(&d.from_english)?)
    .bind(d.belongs_to_collection.as_ref().map(|c| c.id))
    .execute(pool)
    .await?;
    // The title is searchable by its folded keys at once, not after the next scan.
    crate::search_keys::fill(pool).await?;
    Ok(())
}

/// Stores a series and replaces its episode list, in one transaction.
pub(super) async fn store_series(
    pool: &SqlitePool,
    d: &SeriesDetails,
    seasons: &[Season],
    now: i64,
) -> anyhow::Result<()> {
    let genres: Vec<&str> = d.genres.iter().map(|g| g.name.as_str()).collect();
    let ids = d.external_ids.as_ref();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO tmdb_series (tmdb_id, tvdb_id, imdb_id, name, original_name, first_air_date,
             year, overview, status, poster_path, backdrop_path, genres, fetched_at, language,
             english_fields)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (tmdb_id) DO UPDATE SET
             tvdb_id = excluded.tvdb_id, imdb_id = excluded.imdb_id, name = excluded.name,
             original_name = excluded.original_name, first_air_date = excluded.first_air_date,
             year = excluded.year, overview = excluded.overview, status = excluded.status,
             poster_path = excluded.poster_path, backdrop_path = excluded.backdrop_path,
             genres = excluded.genres, fetched_at = excluded.fetched_at,
             images_fetched_at = CASE WHEN coalesce(tmdb_series.language, 'en-US') = excluded.language
                 THEN tmdb_series.images_fetched_at END,
             language = excluded.language, english_fields = excluded.english_fields",
    )
    .bind(d.id)
    .bind(ids.and_then(|i| i.tvdb_id))
    .bind(ids.and_then(|i| non_empty(&i.imdb_id)))
    .bind(&d.name)
    .bind(non_empty(&d.original_name))
    .bind(non_empty(&d.first_air_date))
    .bind(year_of(d.first_air_date.as_deref()))
    .bind(non_empty(&d.overview))
    .bind(non_empty(&d.status))
    .bind(non_empty(&d.poster_path))
    .bind(non_empty(&d.backdrop_path))
    .bind(serde_json::to_string(&genres)?)
    .bind(now)
    .bind(&d.language)
    .bind(english_fields(&d.from_english)?)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM tmdb_episodes WHERE series_tmdb_id = ?")
        .bind(d.id)
        .execute(&mut *tx)
        .await?;
    for e in seasons.iter().flat_map(|s| &s.episodes) {
        sqlx::query(
            "INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, overview,
                 air_date, runtime_minutes, still_path, english_fields)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(d.id)
        .bind(e.season_number)
        .bind(e.episode_number)
        .bind(e.id)
        .bind(non_empty(&e.name))
        .bind(non_empty(&e.overview))
        .bind(non_empty(&e.air_date))
        .bind(e.runtime.filter(|&r| r > 0))
        .bind(non_empty(&e.still_path))
        .bind(english_fields(&e.from_english)?)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    crate::search_keys::fill(pool).await?;
    Ok(())
}

/// The fields filled from English, as JSON; NULL when none were.
fn english_fields(fields: &[&str]) -> serde_json::Result<Option<String>> {
    (!fields.is_empty())
        .then(|| serde_json::to_string(fields))
        .transpose()
}

/// TMDB sends `""` for unknown values; store those as NULL.
pub(super) fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

pub(super) fn year_of(date: Option<&str>) -> Option<i64> {
    date?.get(..4)?.parse().ok()
}
