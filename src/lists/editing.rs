//! Making, changing and removing lists, after checking their settings fit.

use super::*;

/// A list's settings, as made or changed. Checked before anything is stored.
#[derive(Debug, Clone, Deserialize)]
pub struct ListInput {
    /// Its name, 1 to 100 characters, unique among lists.
    pub name: String,
    /// Where it's read from: "tmdb", "imdb" or "rottentomatoes".
    pub provider: String,
    /// Which list: for TMDB a chart ("popular", "trending_week"…) or "list:<id>"; for IMDb
    /// "top", "popular" or a public list's "ls…" id; for Rotten Tomatoes a browse page's
    /// address.
    pub list: String,
    /// "movies" or "series".
    pub kind: String,
    /// The library its titles go in; one for its kind.
    pub library_id: i64,
    /// Whether titles it adds are monitored.
    pub monitored: bool,
    /// The Radarr or Sonarr quality profile its titles get, by name.
    pub quality_profile: String,
    /// How many titles from the top of the list are read, 1 to 250.
    pub max_items: i64,
    /// For series: which seasons a new title monitors, "all", "latest" or "none".
    pub monitor_seasons: String,
    /// Whether it's synced every 12 hours.
    pub enabled: bool,
}

/// TMDB's charts, by the name a list gives, for movies or series.
pub(super) fn tmdb_path(list: &str, series: bool) -> Option<String> {
    if let Some(id) = list.strip_prefix("list:") {
        return (!id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()))
            .then(|| format!("/list/{id}"));
    }
    let kind = if series { "tv" } else { "movie" };
    Some(match (list, series) {
        ("popular" | "top_rated", _) => format!("/{kind}/{list}"),
        ("trending_day", _) => format!("/trending/{kind}/day"),
        ("trending_week", _) => format!("/trending/{kind}/week"),
        ("now_playing" | "upcoming", false) | ("on_the_air" | "airing_today", true) => {
            format!("/{kind}/{list}")
        }
        _ => return None,
    })
}

pub(super) fn imdb_chart(list: &str, series: bool) -> Option<imdb::Chart> {
    Some(match (list, series) {
        ("top", false) => imdb::Chart::TopMovies,
        ("popular", false) => imdb::Chart::PopularMovies,
        ("top", true) => imdb::Chart::TopShows,
        ("popular", true) => imdb::Chart::PopularShows,
        _ => return None,
    })
}

/// Checks a list's settings; the library and profile must fit its kind.
pub(super) async fn check(
    pool: &SqlitePool,
    input: &ListInput,
) -> anyhow::Result<Result<(), String>> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Ok(Err("name is 1 to 100 characters".into()));
    }
    let series = match input.kind.as_str() {
        "movies" => false,
        "series" => true,
        _ => return Ok(Err("kind is movies or series".into())),
    };
    let list_ok = match input.provider.as_str() {
        "tmdb" => tmdb_path(&input.list, series).is_some(),
        "imdb" => imdb_chart(&input.list, series).is_some() || imdb::is_list_id(&input.list),
        "rottentomatoes" => rottentomatoes::is_browse_url(&input.list),
        _ => return Ok(Err("provider is tmdb, imdb or rottentomatoes".into())),
    };
    if !list_ok {
        return Ok(Err(format!(
            "{:?} isn't a {} list Melete reads for {}",
            input.list, input.provider, input.kind
        )));
    }
    if !(1..=250).contains(&input.max_items) {
        return Ok(Err("max_items is 1 to 250".into()));
    }
    if !["all", "latest", "none"].contains(&input.monitor_seasons.as_str()) {
        return Ok(Err("monitor_seasons is all, latest or none".into()));
    }
    let library: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT kind, category FROM libraries WHERE id = ?")
            .bind(input.library_id)
            .fetch_optional(pool)
            .await?;
    let wanted_library = if series { "shows" } else { "movies" };
    // A series list into a library with the anime category adds Anime's series, which take
    // Anime's profiles.
    let service = match &library {
        Some((_, Some(category))) if series && category == "anime" => "anime",
        _ if series => "series",
        _ => "movies",
    };
    match library.map(|(kind, _)| kind) {
        None => return Ok(Err("no such library".into())),
        Some(kind) if kind != wanted_library => {
            return Ok(Err(format!(
                "{} go in a {wanted_library} library",
                input.kind
            )));
        }
        Some(_) => {}
    }
    let profile = crate::profiles::service::named(pool, service, &input.quality_profile).await?;
    if profile.is_none() {
        let whose = match service {
            "anime" => "Anime",
            "series" => "sonarr",
            _ => "radarr",
        };
        return Ok(Err(format!(
            "there's no {whose} quality profile named {:?}",
            input.quality_profile
        )));
    }
    Ok(Ok(()))
}

/// Why a list change wasn't made.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// A setting doesn't fit: why.
    Invalid(String),
    /// There's no such list.
    NotFound,
    /// Another list has that name.
    Taken,
}

/// Makes a list from checked settings; returns its id.
pub async fn create(pool: &SqlitePool, input: &ListInput) -> anyhow::Result<Result<i64, Refused>> {
    if let Err(why) = check(pool, input).await? {
        return Ok(Err(Refused::Invalid(why)));
    }
    let made = sqlx::query_scalar(
        "INSERT INTO import_lists (name, provider, list, kind, library_id, monitored, quality_profile,
             max_items, monitor_seasons, enabled, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(input.name.trim())
    .bind(&input.provider)
    .bind(&input.list)
    .bind(&input.kind)
    .bind(input.library_id)
    .bind(input.monitored)
    .bind(&input.quality_profile)
    .bind(input.max_items)
    .bind(&input.monitor_seasons)
    .bind(input.enabled)
    .bind(unix_now())
    .fetch_one(pool)
    .await;
    match made {
        Ok(id) => Ok(Ok(id)),
        Err(err) if unique_violation(&err) => Ok(Err(Refused::Taken)),
        Err(err) => Err(err.into()),
    }
}

/// Changes a list's settings, checked as when it was made. The titles it added stay as they
/// are.
pub async fn update(
    pool: &SqlitePool,
    id: i64,
    input: &ListInput,
) -> anyhow::Result<Result<(), Refused>> {
    if let Err(why) = check(pool, input).await? {
        return Ok(Err(Refused::Invalid(why)));
    }
    let changed = sqlx::query(
        "UPDATE import_lists SET name = ?, provider = ?, list = ?, kind = ?, library_id = ?,
             monitored = ?, quality_profile = ?, max_items = ?, monitor_seasons = ?, enabled = ?
         WHERE id = ?",
    )
    .bind(input.name.trim())
    .bind(&input.provider)
    .bind(&input.list)
    .bind(&input.kind)
    .bind(input.library_id)
    .bind(input.monitored)
    .bind(&input.quality_profile)
    .bind(input.max_items)
    .bind(&input.monitor_seasons)
    .bind(input.enabled)
    .bind(id)
    .execute(pool)
    .await;
    match changed {
        Ok(done) if done.rows_affected() == 0 => Ok(Err(Refused::NotFound)),
        Ok(_) => Ok(Ok(())),
        Err(err) if unique_violation(&err) => Ok(Err(Refused::Taken)),
        Err(err) => Err(err.into()),
    }
}

/// Removes a list; the titles it added stay wanted.
pub async fn delete(pool: &SqlitePool, id: i64) -> anyhow::Result<bool> {
    Ok(sqlx::query("DELETE FROM import_lists WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}
