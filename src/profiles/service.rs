//! The Manage service a quality profile belongs to: Movies for Radarr's, and Series or Anime for
//! Sonarr's, each with profiles of its own. A series is Anime's when the managed lists put it
//! there (wanted as anime, or its folder's library has the anime category).

use super::*;

/// The services quality profiles belong to.
pub const SERVICES: [&str; 3] = ["movies", "series", "anime"];

/// The app whose custom formats a service's profiles score; None for a service there isn't.
pub fn app_of(service: &str) -> Option<App> {
    match service {
        "movies" => Some(App::Radarr),
        "series" | "anime" => Some(App::Sonarr),
        _ => None,
    }
}

/// The service an app's profiles go to when nothing says otherwise: Movies for Radarr's,
/// Series for Sonarr's.
pub fn default_for(app: App) -> &'static str {
    match app {
        App::Radarr => "movies",
        App::Sonarr => "series",
    }
}

/// A wanted series' service: anime or series.
pub async fn of_wanted_series(pool: &SqlitePool, id: i64) -> sqlx::Result<String> {
    let service: Option<String> = sqlx::query_scalar(
        "SELECT service FROM title_event_titles WHERE kind = 'series' AND wanted_id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(service.unwrap_or_else(|| "series".into()))
}

/// A profile's service; None when there's no such profile.
pub async fn of_profile(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT service FROM quality_profiles WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// The profile a service has by this name.
pub async fn by_name(pool: &SqlitePool, service: &str, name: &str) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT id FROM quality_profiles WHERE service = ? AND name = ?")
        .bind(service)
        .bind(name)
        .fetch_optional(pool)
        .await
}

/// The profile an arr's name means for a title of `service`: that service's of the name, else,
/// for an anime series, Series' of the name. Sonarr shares its profiles between both, so a name
/// it gives an anime series means its own profile until Anime has a copy (profiles imported
/// before the series were).
pub async fn named(pool: &SqlitePool, service: &str, name: &str) -> sqlx::Result<Option<i64>> {
    match by_name(pool, service, name).await? {
        None if service == "anime" => by_name(pool, "series", name).await,
        found => Ok(found),
    }
}

/// The profile of `service` that stands for profile `id`: itself when it's that service's, else
/// that service's profile of the same name, if there's one.
pub async fn equivalent(pool: &SqlitePool, id: i64, service: &str) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT q.id FROM quality_profiles p
         JOIN quality_profiles q ON q.service = ? AND q.name = p.name AND q.source = p.source
         WHERE p.id = ?",
    )
    .bind(service)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// How many profiles a service has.
pub async fn count(pool: &SqlitePool, service: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM quality_profiles WHERE service = ?")
        .bind(service)
        .fetch_one(pool)
        .await
}

/// Where an app's imported profile goes: Radarr's to Movies; a Sonarr profile to each service
/// whose wanted series name it, Series when none do.
pub(super) async fn import_services(
    tx: &mut SqliteConnection,
    app: App,
    name: &str,
) -> sqlx::Result<Vec<String>> {
    if app == App::Radarr {
        return Ok(vec!["movies".into()]);
    }
    let used: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT t.service FROM wanted_series w
         JOIN title_event_titles t ON t.kind = 'series' AND t.wanted_id = w.id
         WHERE w.quality_profile = ? ORDER BY t.service DESC",
    )
    .bind(name)
    .fetch_all(&mut *tx)
    .await?;
    Ok(if used.is_empty() {
        vec!["series".into()]
    } else {
        used
    })
}

#[cfg(test)]
mod tests;
