//! Putting a TMDB title on the wanted list, as import lists and requests both do: the
//! title from TMDB, its folder when it's on disk already, and for a series its seasons, each
//! monitored as asked. A title already wanted is left as it is, except that a season asked for
//! is monitored.

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::tmdb;
use crate::wanted_edit::{self, Kind};

mod linking;

pub use linking::*;

/// Where a wanted title goes and how it's looked for.
pub struct Placement<'a> {
    /// Whether the title is monitored: looked for, and grabbed when found.
    pub monitored: bool,
    /// The quality profile by the *arr's name (lists), when there's one.
    pub quality_profile: Option<&'a str>,
    /// A quality profile of Melete's own (requests), when chosen.
    pub profile_id: Option<i64>,
    /// The library it goes in.
    pub library_id: i64,
    /// For a series, which seasons are monitored; a movie ignores it.
    pub seasons: Seasons,
    /// The import list that added it, when one did.
    pub list_id: Option<i64>,
    /// Leave off a title an admin took off the wanted list in Melete (`wanted_removed`), as
    /// imports do; a request or a hand puts it back.
    pub keep_removals: bool,
}

/// Which of a series' seasons are monitored. Specials only when asked for by number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seasons {
    /// Every season but the specials.
    All,
    /// The latest season only.
    Latest,
    /// No season.
    None,
    /// That season only (0 for the specials).
    Only(i64),
}

impl Seasons {
    /// A list's monitor_seasons: all, latest or none.
    pub fn named(name: &str) -> Seasons {
        match name {
            "all" => Seasons::All,
            "latest" => Seasons::Latest,
            _ => Seasons::None,
        }
    }

    fn monitors(self, season: i64, latest: Option<i64>) -> bool {
        match self {
            Seasons::All => season > 0,
            Seasons::Latest => season > 0 && Some(season) == latest,
            Seasons::None => false,
            Seasons::Only(n) => season == n,
        }
    }
}

/// What putting a title on the wanted list did.
#[derive(Debug, PartialEq, Eq)]
pub enum Wanted {
    /// Newly wanted: its id.
    Added(i64),
    /// Wanted already: its id.
    Already(i64),
    /// Couldn't be: why.
    Skipped(String),
}

/// Why an import list didn't add a title an admin took off the list.
const TAKEN_OFF: &str = "taken off the wanted list in Melete; lists leave it off until it's allowed back or added again";

/// Whether an admin took the title off the wanted list in Melete: a movie by its TMDB id, a
/// series by its TVDB id, as `wanted_removed` keeps them.
async fn removed(pool: &SqlitePool, kind: Kind, id: i64) -> anyhow::Result<bool> {
    let mut conn = pool.acquire().await?;
    Ok(wanted_edit::removed_here(&mut conn, kind, &id.to_string()).await?)
}

/// The year of a TMDB date ("1999-03-30"); `None` when it doesn't start with one, as an empty
/// date doesn't.
pub fn year_of(date: &str) -> Option<i64> {
    date.get(..4)?.parse().ok()
}

/// Puts a TMDB movie on the wanted list, with its folder when it's on disk already. One wanted
/// already is left as it is; one TMDB no longer has, or (with `keep_removals`) one an admin took
/// off the list, is skipped with why.
pub async fn movie(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    tmdb_id: i64,
    placement: &Placement<'_>,
) -> anyhow::Result<Wanted> {
    let wanted: Option<i64> = sqlx::query_scalar("SELECT id FROM wanted_movies WHERE tmdb_id = ?")
        .bind(tmdb_id)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = wanted {
        return Ok(Wanted::Already(id));
    }
    if placement.keep_removals && removed(pool, Kind::Movie, tmdb_id).await? {
        return Ok(Wanted::Skipped(TAKEN_OFF.into()));
    }
    let Some(movie) = tmdb.movie(tmdb_id).await? else {
        return Ok(Wanted::Skipped("TMDB no longer has it".into()));
    };
    // Its folder, when it's on disk already.
    let adopted: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM movies WHERE missing_since IS NULL
           AND (match_tmdb_id = ?1 OR arr_tmdb_id = ?1 OR tmdb_id = ?1)
         ORDER BY id LIMIT 1",
    )
    .bind(tmdb_id)
    .fetch_optional(pool)
    .await?;
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO wanted_movies (tmdb_id, imdb_id, title, year, monitored, quality_profile,
             profile_id, minimum_availability, library_id, movie_id, source, added_at,
             original_language, list_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'released', ?, ?, 'manual', ?, ?, ?)
         ON CONFLICT (tmdb_id) DO NOTHING RETURNING id",
    )
    .bind(tmdb_id)
    .bind(movie.imdb_id.filter(|i| !i.is_empty()))
    .bind(&movie.title)
    .bind(movie.release_date.as_deref().and_then(year_of))
    .bind(placement.monitored)
    .bind(placement.quality_profile)
    .bind(placement.profile_id)
    .bind(placement.library_id)
    .bind(adopted)
    .bind(unix_now())
    .bind(movie.original_language.filter(|l| !l.is_empty()))
    .bind(placement.list_id)
    .fetch_optional(pool)
    .await?;
    Ok(match id {
        Some(id) => Wanted::Added(id),
        // Added by something else meanwhile.
        None => Wanted::Already(
            sqlx::query_scalar("SELECT id FROM wanted_movies WHERE tmdb_id = ?")
                .bind(tmdb_id)
                .fetch_one(pool)
                .await?,
        ),
    })
}

/// Whether TMDB's series is anime: animated and Japanese, as Sonarr's anime type means.
pub fn is_anime(show: &tmdb::SeriesDetails) -> bool {
    let animated = show.genres.iter().any(|g| g.name == "Animation");
    animated && show.original_language.as_deref() == Some("ja")
}

/// Puts a TMDB series on the wanted list with its seasons monitored as `placement` says, and
/// its folder when it's on disk already. One wanted already keeps its settings, except that a
/// season asked for is monitored. Skipped with why when TMDB no longer has it, has no TVDB id
/// for it, or (with `keep_removals`) an admin took it off the list.
pub async fn series(
    pool: &SqlitePool,
    tmdb: &tmdb::Client,
    tmdb_id: i64,
    placement: &Placement<'_>,
) -> anyhow::Result<Wanted> {
    let wanted: Option<i64> = sqlx::query_scalar("SELECT id FROM wanted_series WHERE tmdb_id = ?")
        .bind(tmdb_id)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = wanted {
        monitor_asked(pool, id, placement.seasons).await?;
        return Ok(Wanted::Already(id));
    }
    let Some(show) = tmdb.series(tmdb_id).await? else {
        return Ok(Wanted::Skipped("TMDB no longer has it".into()));
    };
    // The wanted list knows series by TVDB id, as Sonarr does.
    let Some(tvdb_id) = show.external_ids.as_ref().and_then(|i| i.tvdb_id) else {
        return Ok(Wanted::Skipped("TMDB has no TVDB id for it".into()));
    };
    let already: Option<i64> = sqlx::query_scalar("SELECT id FROM wanted_series WHERE tvdb_id = ?")
        .bind(tvdb_id)
        .fetch_optional(pool)
        .await?;
    if let Some(id) = already {
        monitor_asked(pool, id, placement.seasons).await?;
        return Ok(Wanted::Already(id));
    }
    if placement.keep_removals && removed(pool, Kind::Series, tvdb_id).await? {
        return Ok(Wanted::Skipped(TAKEN_OFF.into()));
    }
    let original = show.original_language.clone().filter(|l| !l.is_empty());
    let series_type = if is_anime(&show) { "anime" } else { "standard" };
    let adopted: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM series WHERE missing_since IS NULL
           AND (match_tmdb_id = ?1 OR arr_tmdb_id = ?1 OR tmdb_id = ?1 OR arr_tvdb_id = ?2 OR tvdb_id = ?2)
         ORDER BY id LIMIT 1",
    )
    .bind(tmdb_id)
    .bind(tvdb_id)
    .fetch_optional(pool)
    .await?;
    let mut tx = pool.begin().await?;
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO wanted_series (tvdb_id, tmdb_id, imdb_id, title, year, monitored, series_type,
             quality_profile, profile_id, library_id, series_id, source, added_at,
             original_language, list_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'manual', ?, ?, ?)
         ON CONFLICT (tvdb_id) DO NOTHING RETURNING id",
    )
    .bind(tvdb_id)
    .bind(tmdb_id)
    .bind(
        show.external_ids
            .as_ref()
            .and_then(|i| i.imdb_id.clone())
            .filter(|i| !i.is_empty()),
    )
    .bind(&show.name)
    .bind(show.first_air_date.as_deref().and_then(year_of))
    .bind(placement.monitored)
    .bind(series_type)
    .bind(placement.quality_profile)
    .bind(placement.profile_id)
    .bind(placement.library_id)
    .bind(adopted)
    .bind(unix_now())
    .bind(&original)
    .bind(placement.list_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = id else {
        let id: i64 = sqlx::query_scalar("SELECT id FROM wanted_series WHERE tvdb_id = ?")
            .bind(tvdb_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        monitor_asked(pool, id, placement.seasons).await?;
        return Ok(Wanted::Already(id));
    };
    let seasons: Vec<i64> = show.seasons.iter().map(|s| s.season_number).collect();
    let latest = seasons.iter().copied().filter(|s| *s > 0).max();
    for season in seasons {
        sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored) VALUES (?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(season)
        .bind(placement.seasons.monitors(season, latest))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Wanted::Added(id))
}

/// For a series wanted already: the one season asked for is monitored, added if it's new; as
/// a choice made in Melete, which an import from Sonarr keeps.
pub async fn monitor_asked(pool: &SqlitePool, id: i64, seasons: Seasons) -> anyhow::Result<()> {
    if let Seasons::Only(season) = seasons {
        sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored, monitored_here)
             VALUES (?, ?, 1, 1)
             ON CONFLICT (wanted_series_id, season) DO UPDATE SET monitored = 1, monitored_here = 1",
        )
        .bind(id)
        .bind(season)
        .execute(pool)
        .await?;
    }
    Ok(())
}
