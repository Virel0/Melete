//! Changing the wanted list by hand: monitoring or not monitoring a movie, series,
//! artist or album, and taking one off the list. Nothing on disk is touched: taking a title off
//! the list only stops Melete looking for it. Both are the admin's own choices, which imports
//! from Radarr, Sonarr and Lidarr leave alone (`monitored_here`, `wanted_removed`).

/// The kinds of wanted entry this changes, each with its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Series,
    Artist,
    Album,
}

impl Kind {
    /// The kind `wanted_removed` names: movie, series, artist or album.
    pub fn parse(name: &str) -> Option<Kind> {
        match name {
            "movie" => Some(Kind::Movie),
            "series" => Some(Kind::Series),
            "artist" => Some(Kind::Artist),
            "album" => Some(Kind::Album),
            _ => None,
        }
    }

    fn table(self) -> &'static str {
        match self {
            Kind::Movie => "wanted_movies",
            Kind::Series => "wanted_series",
            Kind::Artist => "wanted_artists",
            Kind::Album => "wanted_albums",
        }
    }

    /// Its name in `wanted_removed`, and the column imports know an entry by, with its title.
    fn removed(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Kind::Movie => ("movie", "CAST(tmdb_id AS TEXT)", "title"),
            Kind::Series => ("series", "CAST(tvdb_id AS TEXT)", "title"),
            Kind::Artist => ("artist", "mb_artist_id", "name"),
            Kind::Album => ("album", "mb_release_group_id", "title"),
        }
    }
}

/// Whether an admin took this title off the wanted list in Melete and it isn't back: an
/// import from Radarr, Sonarr or Lidarr then leaves it off.
pub async fn removed_here(
    conn: &mut sqlx::SqliteConnection,
    kind: Kind,
    key: &str,
) -> sqlx::Result<bool> {
    let (name, column, _) = kind.removed();
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM wanted_removed WHERE kind = ?1 AND key = ?2)
            AND NOT EXISTS (SELECT 1 FROM {} WHERE {column} = ?2)",
        kind.table()
    )))
    .bind(name)
    .bind(key)
    .fetch_one(conn)
    .await
}

/// Why an entry wasn't taken off the list.
#[derive(Debug, PartialEq, Eq)]
pub enum NotRemoved {
    NotFound,
    /// Downloads for it are on their way: taken off now, they'd arrive for nothing.
    Downloading(i64),
}

/// Monitors an entry or stops monitoring it; false when there's no such entry.
pub async fn set_monitored(
    pool: &sqlx::SqlitePool,
    kind: Kind,
    id: i64,
    monitored: bool,
) -> anyhow::Result<bool> {
    // Set here, so an import no longer changes it.
    let changed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {} SET monitored = ?, monitored_here = 1 WHERE id = ?",
        kind.table()
    )))
    .bind(monitored)
    .bind(id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Monitors a season of a wanted series or stops monitoring it, adding the season when it isn't
/// listed yet; false when there's no such series. Set here, an import from Sonarr keeps it
/// unless Sonarr's own value for the season changes.
pub async fn set_season_monitored(
    pool: &sqlx::SqlitePool,
    series: i64,
    season: i64,
    monitored: bool,
) -> anyhow::Result<bool> {
    let changed = sqlx::query(
        "INSERT INTO wanted_seasons (wanted_series_id, season, monitored, monitored_here)
         SELECT id, ?2, ?3, 1 FROM wanted_series WHERE id = ?1
         ON CONFLICT (wanted_series_id, season)
             DO UPDATE SET monitored = excluded.monitored, monitored_here = 1",
    )
    .bind(series)
    .bind(season)
    .bind(monitored)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(changed > 0)
}

/// Why an episode's monitoring wasn't changed.
#[derive(Debug, PartialEq, Eq)]
pub enum EpisodeNotSet {
    /// There's no such wanted series.
    NotFound,
    /// Its season isn't monitored, so the episode can't be: an episode is looked for only
    /// within a monitored season.
    SeasonOff,
}

/// Monitors an episode of a wanted series or leaves it out on its own. Set here, an import from
/// Sonarr keeps it unless Sonarr's own value for the episode changes.
pub async fn set_episode_monitored(
    pool: &sqlx::SqlitePool,
    series: i64,
    season: i64,
    episode: i64,
    monitored: bool,
) -> anyhow::Result<Result<(), EpisodeNotSet>> {
    let season_on: Option<Option<bool>> = sqlx::query_scalar(
        "SELECT (SELECT s.monitored = 1 FROM wanted_seasons s
                 WHERE s.wanted_series_id = w.id AND s.season = ?2)
         FROM wanted_series w WHERE w.id = ?1",
    )
    .bind(series)
    .bind(season)
    .fetch_optional(pool)
    .await?;
    match season_on {
        None => return Ok(Err(EpisodeNotSet::NotFound)),
        Some(on) if monitored && on != Some(true) => return Ok(Err(EpisodeNotSet::SeasonOff)),
        Some(_) => {}
    }
    sqlx::query(
        "INSERT INTO wanted_episode_overrides
             (wanted_series_id, season, episode, monitored, monitored_here)
         VALUES (?, ?, ?, ?, 1)
         ON CONFLICT (wanted_series_id, season, episode)
             DO UPDATE SET monitored = excluded.monitored, monitored_here = 1",
    )
    .bind(series)
    .bind(season)
    .bind(episode)
    .bind(monitored)
    .execute(pool)
    .await?;
    Ok(Ok(()))
}

/// What happens to albums an artist releases later: `all`, `new` (those released from when
/// Melete first looked) or `none`. False when there's no such artist. Set here, an import
/// from Lidarr keeps it.
pub async fn set_monitor_new(
    pool: &sqlx::SqlitePool,
    artist: i64,
    rule: &str,
) -> anyhow::Result<bool> {
    let changed =
        sqlx::query("UPDATE wanted_artists SET monitor_new = ?, monitor_new_here = 1 WHERE id = ?")
            .bind(rule)
            .bind(artist)
            .execute(pool)
            .await?
            .rows_affected();
    Ok(changed > 0)
}

/// [`remove_by`] with no one recorded as taking it off, as tests do.
#[cfg(test)]
pub async fn remove(
    pool: &sqlx::SqlitePool,
    kind: Kind,
    id: i64,
) -> anyhow::Result<Result<(), NotRemoved>> {
    remove_by(pool, kind, id, None).await
}

/// Takes an entry off the wanted list (an artist with their albums), unless downloads for it
/// are on their way, remembering who (`by`) for the list of removed titles. Its
/// searches go with it; requests for it stay, no longer followed.
pub async fn remove_by(
    pool: &sqlx::SqlitePool,
    kind: Kind,
    id: i64,
    by: Option<i64>,
) -> anyhow::Result<Result<(), NotRemoved>> {
    let mut tx = pool.begin().await?;
    let exists: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {} WHERE id = ?",
        kind.table()
    )))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        return Ok(Err(NotRemoved::NotFound));
    }
    let on_the_way: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM downloads WHERE state NOT IN ('failed', 'imported') AND {}",
        match kind {
            Kind::Movie => "wanted_movie_id = ?",
            Kind::Series => "wanted_series_id = ?",
            Kind::Album => "wanted_album_id = ?",
            Kind::Artist =>
                "wanted_album_id IN (SELECT id FROM wanted_albums WHERE wanted_artist_id = ?)",
        }
    )))
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    if on_the_way > 0 {
        return Ok(Err(NotRemoved::Downloading(on_the_way)));
    }
    // Remembered, so an import doesn't add it back; an artist's albums go with the artist.
    let (name, column, title) = kind.removed();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO wanted_removed (kind, key, title, removed_at, removed_by)
         SELECT ?, {column}, {title}, ?, ? FROM {} WHERE id = ? AND {column} IS NOT NULL
         ON CONFLICT (kind, key) DO UPDATE SET title = excluded.title,
             removed_at = excluded.removed_at, removed_by = excluded.removed_by",
        kind.table()
    )))
    .bind(name)
    .bind(crate::db::unix_now())
    .bind(by)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM {} WHERE id = ?",
        kind.table()
    )))
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn monitors_and_removes_unless_a_download_is_on_its_way() {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 1, 'Dune', 1, 'manual', 0), (2, 2, 'Arrival', 1, 'manual', 0);
             INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES (1, 'a1', 'Arca', 1, 'all', 'manual', 0);
             INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
             VALUES (1, 1, 'rg1', 'Kick ii', 1, 'manual', 0);
             INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, added_at, updated_at)
             VALUES ('a11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Dune', 1, '{}', 'downloading', 0, 0),
                    ('b11bae70b2fdd9eb95b63e746f8594dce2650b0c', 'Arrival', 2, '{}', 'failed', 0, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(set_monitored(&pool, Kind::Movie, 1, false).await.unwrap());
        let monitored: bool =
            sqlx::query_scalar("SELECT monitored FROM wanted_movies WHERE id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!monitored);
        assert!(!set_monitored(&pool, Kind::Series, 9, true).await.unwrap());
        assert!(set_monitored(&pool, Kind::Album, 1, false).await.unwrap());

        // Dune is downloading: it stays. Arrival's download failed: it goes, the download stays.
        assert_eq!(
            remove(&pool, Kind::Movie, 1).await.unwrap(),
            Err(NotRemoved::Downloading(1))
        );
        remove(&pool, Kind::Movie, 2).await.unwrap().unwrap();
        let left: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM wanted_movies), (SELECT count(*) FROM downloads)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(left, (1, 2));
        assert_eq!(
            remove(&pool, Kind::Movie, 2).await.unwrap(),
            Err(NotRemoved::NotFound)
        );

        // An artist goes with their albums.
        remove(&pool, Kind::Artist, 1).await.unwrap().unwrap();
        let albums: i64 = sqlx::query_scalar("SELECT count(*) FROM wanted_albums")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(albums, 0);
    }
}
