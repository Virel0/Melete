//! Sonarr's series and their episodes into the wanted list, with their ids for folders on disk.

use super::*;

/// What a Sonarr import did, for the admin who ran it.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct SonarrReport {
    /// Series Sonarr listed with a TVDB id.
    pub series: usize,
    /// Of those, the ones Sonarr is looking for.
    pub monitored: usize,
    /// Series whose folder Melete already adopted.
    pub on_disk: usize,
    /// Of those, folders to be matched again with Sonarr's ids: unmatched ones, and ones whose
    /// match disagreed (matches made by hand are left alone).
    pub rematching: usize,
    /// Series whose root folder matched no shows library.
    pub without_library: usize,
    /// Episodes unmonitored inside monitored seasons, kept as overrides.
    pub unmonitored_episodes: usize,
    /// Wanted series from Sonarr that Sonarr no longer lists, removed.
    pub removed: u64,
}

/// Sonarr's series type; anything unexpected counts as standard.
pub(super) fn series_type(value: Option<&str>) -> &'static str {
    match value {
        Some("daily") => "daily",
        Some("anime") => "anime",
        _ => "standard",
    }
}

/// Episode lists fetched at once from Sonarr.
pub(super) const PARALLEL_EPISODE_FETCHES: usize = 4;

/// Imports Sonarr's series into the wanted list, with season monitoring and the episodes left
/// unmonitored inside monitored seasons. Everything is fetched first and then written in one
/// transaction, so the database isn't held while Sonarr answers. Running it again updates each
/// series' seasons and overrides, keeping choices made in Melete that Sonarr hasn't changed since.
pub async fn import_sonarr(
    pool: &SqlitePool,
    sonarr: &ArrClient,
    root_folders: &HashMap<String, String>,
) -> anyhow::Result<SonarrReport> {
    sonarr.expect_app("Sonarr").await?;
    let profiles: HashMap<i64, String> = sonarr
        .quality_profiles()
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let series: Vec<_> = sonarr
        .sonarr_series()
        .await?
        .into_iter()
        .filter(|s| s.tvdb_id > 0)
        .collect();

    let episodes = fetch_episodes(sonarr, &series).await?;

    let libraries = libraries(pool, "shows").await?;
    let now = unix_now();
    let mut report = SonarrReport::default();
    let mut tx = pool.begin().await?;
    for show in &series {
        report.series += 1;
        report.monitored += usize::from(show.monitored);
        if wanted_edit::removed_here(&mut tx, Kind::Series, &show.tvdb_id.to_string()).await? {
            continue;
        }
        let library_id = library_for(show.root_folder_path.as_deref(), &libraries, root_folders);
        if library_id.is_none() {
            report.without_library += 1;
        }
        let tmdb_id = show.tmdb_id.filter(|id| *id > 0);

        let adopted_id = adopt_series(&mut tx, show, library_id, tmdb_id, &mut report).await?;
        let wanted_id = upsert_wanted_series(
            &mut tx, show, tmdb_id, &profiles, library_id, adopted_id, now,
        )
        .await?;
        let Some(wanted_id) = wanted_id else { continue };

        replace_seasons(&mut tx, show, wanted_id).await?;
        store_episode_overrides(&mut tx, show, wanted_id, &episodes, &mut report).await?;
    }
    let listed: Vec<i64> = series.iter().map(|s| s.tvdb_id).collect();
    report.removed = remove_unlisted(&mut tx, "sonarr", &listed).await?;
    tx.commit().await?;
    Ok(report)
}

/// Fetches each series' episodes from Sonarr, a few series at a time.
async fn fetch_episodes(
    sonarr: &ArrClient,
    series: &[crate::arr::SonarrSeries],
) -> anyhow::Result<HashMap<i64, Vec<crate::arr::SonarrEpisode>>> {
    use std::sync::Arc;

    // Episodes, four series at a time.
    let mut episodes: HashMap<i64, Vec<crate::arr::SonarrEpisode>> = HashMap::new();
    let slots = Arc::new(tokio::sync::Semaphore::new(PARALLEL_EPISODE_FETCHES));
    let mut fetches = tokio::task::JoinSet::new();
    for id in series.iter().map(|s| s.id) {
        let (slots, client) = (slots.clone(), sonarr.clone());
        fetches.spawn(async move {
            let _slot = slots
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            (id, client.sonarr_episodes(id).await)
        });
    }
    while let Some(fetched) = fetches.join_next().await {
        let (id, result) = fetched?;
        episodes.insert(id, result?);
    }
    Ok(episodes)
}

/// Finds the series' folder already on disk, gives it Sonarr's ids, and clears its match for
/// redoing unless it agrees or was made by hand; returns the adopted series' id.
async fn adopt_series(
    tx: &mut sqlx::SqliteConnection,
    show: &crate::arr::SonarrSeries,
    library_id: Option<i64>,
    tmdb_id: Option<i64>,
    report: &mut SonarrReport,
) -> anyhow::Result<Option<i64>> {
    let folder = show.path.as_deref().and_then(last_component);
    // (series id, matched TMDB id, match method, the matched series' TVDB id)
    type Adopted = (i64, Option<i64>, Option<String>, Option<i64>);
    let adopted: Option<Adopted> = match folder {
        Some(folder) => {
            let found: Vec<Adopted> = sqlx::query_as(
                "SELECT s.id, s.match_tmdb_id, s.match_method, t.tvdb_id FROM series s
                     JOIN libraries l ON l.id = s.library_id
                     LEFT JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
                     WHERE l.kind = 'shows' AND s.rel_path = ?1 AND s.missing_since IS NULL
                       AND (?2 IS NULL OR s.library_id = ?2)
                     LIMIT 2",
            )
            .bind(folder)
            .bind(library_id)
            .fetch_all(&mut *tx)
            .await?;
            if found.len() == 1 {
                found.into_iter().next()
            } else {
                None
            }
        }
        None => None,
    };
    let adopted_id = adopted.as_ref().map(|a| a.0);
    if let Some((series_id, matched, method, matched_tvdb)) = adopted {
        report.on_disk += 1;
        sqlx::query("UPDATE series SET arr_tvdb_id = ?, arr_tmdb_id = ? WHERE id = ?")
            .bind(show.tvdb_id)
            .bind(tmdb_id)
            .bind(series_id)
            .execute(&mut *tx)
            .await?;
        let agrees =
            (tmdb_id.is_some() && matched == tmdb_id) || matched_tvdb == Some(show.tvdb_id);
        if method.as_deref() != Some("manual") && !agrees {
            sqlx::query(
                "UPDATE series SET match_status = NULL, match_tmdb_id = NULL,
                         match_method = NULL, match_candidates = NULL
                     WHERE id = ?",
            )
            .bind(series_id)
            .execute(&mut *tx)
            .await?;
            report.rematching += 1;
        }
    }
    Ok(adopted_id)
}

/// Adds the series to the wanted list, or updates its entry when that came from Sonarr;
/// returns the entry's id, or None when an entry added another way already has the series.
async fn upsert_wanted_series(
    tx: &mut sqlx::SqliteConnection,
    show: &crate::arr::SonarrSeries,
    tmdb_id: Option<i64>,
    profiles: &HashMap<i64, String>,
    library_id: Option<i64>,
    adopted_id: Option<i64>,
    now: i64,
) -> anyhow::Result<Option<i64>> {
    let series_type = series_type(show.series_type.as_deref());
    let profile = show.quality_profile_id.and_then(|id| profiles.get(&id));
    // None when an entry added by hand already has this series: it's left alone.
    let wanted_id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO wanted_series (tvdb_id, tmdb_id, imdb_id, title, year, monitored,
                 series_type, quality_profile, library_id, series_id, source, source_id, added_at,
                 original_language)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'sonarr', ?, ?, ?)
             ON CONFLICT (tvdb_id) DO UPDATE SET
                 tmdb_id = excluded.tmdb_id, imdb_id = excluded.imdb_id, title = excluded.title,
                 year = excluded.year,
                 monitored = CASE wanted_series.monitored_here WHEN 1 THEN wanted_series.monitored
                                  ELSE excluded.monitored END,
                 series_type = excluded.series_type, quality_profile = excluded.quality_profile,
                                  library_id = excluded.library_id, series_id = excluded.series_id,
                 source_id = excluded.source_id, original_language = excluded.original_language
             WHERE wanted_series.source = 'sonarr'
             RETURNING id",
    )
    .bind(show.tvdb_id)
    .bind(tmdb_id)
    .bind(show.imdb_id.as_deref().filter(|i| !i.is_empty()))
    .bind(&show.title)
    .bind(show.year.filter(|y| *y > 0))
    .bind(show.monitored)
    .bind(series_type)
    .bind(profile)
    .bind(library_id)
    .bind(adopted_id)
    .bind(show.id)
    .bind(now)
    .bind(original_language(show.original_language.as_ref()))
    .fetch_optional(&mut *tx)
    .await?;
    Ok(wanted_id)
}

/// Stores each season's monitoring from Sonarr. A season monitored (or not) in Melete keeps that
/// choice while Sonarr's own value for it is what Sonarr said the time before, or Sonarr hadn't
/// said; once Sonarr's value changes, Sonarr's is taken, as the later change. Seasons Sonarr no
/// longer lists go, unless set in Melete.
async fn replace_seasons(
    tx: &mut sqlx::SqliteConnection,
    show: &crate::arr::SonarrSeries,
    wanted_id: i64,
) -> anyhow::Result<()> {
    let listed: Vec<i64> = show.seasons.iter().map(|s| s.season_number).collect();
    sqlx::query(
        "DELETE FROM wanted_seasons WHERE wanted_series_id = ? AND monitored_here = 0
           AND season NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(wanted_id)
    .bind(serde_json::to_string(&listed)?)
    .execute(&mut *tx)
    .await?;
    for season in &show.seasons {
        sqlx::query(
            "INSERT INTO wanted_seasons (wanted_series_id, season, monitored, arr_monitored)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT (wanted_series_id, season) DO UPDATE SET
                 monitored = CASE WHEN wanted_seasons.monitored_here = 1
                                       AND coalesce(wanted_seasons.arr_monitored
                                                    = excluded.arr_monitored, 1)
                                  THEN wanted_seasons.monitored ELSE excluded.monitored END,
                 monitored_here = wanted_seasons.monitored_here = 1
                                  AND coalesce(wanted_seasons.arr_monitored
                                               = excluded.arr_monitored, 1),
                 arr_monitored = excluded.arr_monitored",
        )
        .bind(wanted_id)
        .bind(season.season_number)
        .bind(season.monitored)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

/// Keeps the episodes left unmonitored inside monitored seasons as overrides of the entry. An
/// episode monitored or left out in Melete keeps that choice as a season does: until Sonarr's
/// own value for it changes.
async fn store_episode_overrides(
    tx: &mut sqlx::SqliteConnection,
    show: &crate::arr::SonarrSeries,
    wanted_id: i64,
    episodes: &HashMap<i64, Vec<crate::arr::SonarrEpisode>>,
    report: &mut SonarrReport,
) -> anyhow::Result<()> {
    let listed: HashMap<(i64, i64), bool> = episodes
        .get(&show.id)
        .into_iter()
        .flatten()
        .map(|e| ((e.season_number, e.episode_number), e.monitored))
        .collect();
    let here: Vec<(i64, i64, Option<bool>)> = sqlx::query_as(
        "SELECT season, episode, arr_monitored FROM wanted_episode_overrides
         WHERE wanted_series_id = ? AND monitored_here = 1",
    )
    .bind(wanted_id)
    .fetch_all(&mut *tx)
    .await?;
    for (season, episode, said) in here {
        let Some(&now) = listed.get(&(season, episode)) else {
            continue;
        };
        // Sonarr changed its mind since: its value is the later one.
        let query = if said.is_some_and(|said| said != now) {
            "DELETE FROM wanted_episode_overrides
             WHERE wanted_series_id = ?1 AND season = ?2 AND episode = ?3"
        } else {
            "UPDATE wanted_episode_overrides SET arr_monitored = ?4
             WHERE wanted_series_id = ?1 AND season = ?2 AND episode = ?3"
        };
        sqlx::query(query)
            .bind(wanted_id)
            .bind(season)
            .bind(episode)
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "DELETE FROM wanted_episode_overrides WHERE wanted_series_id = ? AND monitored_here = 0",
    )
    .bind(wanted_id)
    .execute(&mut *tx)
    .await?;
    let monitored_seasons: Vec<i64> = show
        .seasons
        .iter()
        .filter(|s| s.monitored)
        .map(|s| s.season_number)
        .collect();
    for episode in episodes.get(&show.id).into_iter().flatten() {
        if episode.monitored || !monitored_seasons.contains(&episode.season_number) {
            continue;
        }
        sqlx::query(
            "INSERT OR IGNORE INTO wanted_episode_overrides
                     (wanted_series_id, season, episode, monitored, arr_monitored)
                 VALUES (?, ?, ?, 0, 0)",
        )
        .bind(wanted_id)
        .bind(episode.season_number)
        .bind(episode.episode_number)
        .execute(&mut *tx)
        .await?;
        report.unmonitored_episodes += 1;
    }
    Ok(())
}
