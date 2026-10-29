//! Lidarr's artists and albums into the wanted list.

use super::*;

/// What a Lidarr import did, for the admin who ran it.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct LidarrReport {
    /// Artists Lidarr listed with a MusicBrainz id.
    pub artists: usize,
    /// Of those, the ones Lidarr is looking for.
    pub monitored_artists: usize,
    /// Albums Lidarr listed with a MusicBrainz id, for those artists, and not taken off the
    /// list in Melete.
    pub albums: usize,
    /// Of those, the ones Lidarr is looking for.
    pub monitored_albums: usize,
    /// Wanted albums already in a music library (by MusicBrainz release group).
    pub on_disk: usize,
    /// Artists whose root folder matched no music library.
    pub without_library: usize,
    /// Wanted artists and albums from Lidarr that Lidarr no longer lists, removed.
    pub removed_artists: u64,
    /// Wanted albums from Lidarr that Lidarr no longer lists, removed.
    pub removed_albums: u64,
}

/// The names of an album's secondary types, however this Lidarr wrote them.
pub(super) fn secondary_types(types: &[serde_json::Value]) -> Vec<&str> {
    types
        .iter()
        .filter_map(|t| t.as_str().or_else(|| t["name"].as_str()))
        .filter(|t| !t.is_empty())
        .collect()
}

/// The date part of Lidarr's timestamp; None when it's empty or a placeholder year.
pub(super) fn release_date(timestamp: Option<&str>) -> Option<&str> {
    let date = timestamp?.get(..10).unwrap_or(timestamp?);
    (date.len() >= 4 && !date.starts_with("0001")).then_some(date)
}

/// Imports Lidarr's artists and albums into the wanted list, in one transaction. Running it
/// again updates the same entries; entries added another way are left alone.
pub async fn import_lidarr(
    pool: &SqlitePool,
    lidarr: &ArrClient,
    root_folders: &HashMap<String, String>,
) -> anyhow::Result<LidarrReport> {
    lidarr.expect_app("Lidarr").await?;
    let profiles: HashMap<i64, String> = lidarr
        .lidarr_quality_profiles()
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let artists = lidarr.lidarr_artists().await?;
    let albums = lidarr.lidarr_albums().await?;
    let libraries = libraries(pool, "music").await?;
    let now = unix_now();

    let mut report = LidarrReport::default();
    let mut tx = pool.begin().await?;
    let listed_artists: Vec<&str> = artists
        .iter()
        .map(|a| a.foreign_artist_id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    refuse_to_empty(&mut tx, &listed_artists).await?;
    // Lidarr's artist id to the wanted entry holding its MusicBrainz id.
    let mut wanted_ids: HashMap<i64, i64> = HashMap::new();
    for artist in artists.iter().filter(|a| !a.foreign_artist_id.is_empty()) {
        report.artists += 1;
        report.monitored_artists += usize::from(artist.monitored);
        // Its albums are left off with it: they're only taken for artists on the list.
        if wanted_edit::removed_here(&mut tx, Kind::Artist, &artist.foreign_artist_id).await? {
            continue;
        }
        let library_id = library_for(artist.root_folder_path.as_deref(), &libraries, root_folders);
        if library_id.is_none() {
            report.without_library += 1;
        }
        let id = upsert_wanted_artist(&mut tx, artist, &profiles, library_id, now).await?;
        wanted_ids.insert(artist.id, id);
    }
    let mut listed_albums: Vec<&str> = Vec::new();
    for album in albums.iter().filter(|a| !a.foreign_album_id.is_empty()) {
        let Some(&artist) = wanted_ids.get(&album.artist_id) else {
            continue;
        };
        if wanted_edit::removed_here(&mut tx, Kind::Album, &album.foreign_album_id).await? {
            continue;
        }
        report.albums += 1;
        report.monitored_albums += usize::from(album.monitored);
        listed_albums.push(&album.foreign_album_id);
        upsert_wanted_album(&mut tx, album, artist, now).await?;
    }
    remove_unlisted_music(&mut tx, &listed_albums, &listed_artists, &mut report).await?;
    let on_disk: i64 = albums_on_disk(&mut tx).await?;
    report.on_disk = on_disk as usize;
    tx.commit().await?;
    Ok(report)
}

/// Refuses the import when Lidarr listed no artists while artists imported from it exist.
async fn refuse_to_empty(
    tx: &mut sqlx::SqliteConnection,
    listed_artists: &[&str],
) -> anyhow::Result<()> {
    if listed_artists.is_empty() {
        let existing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM wanted_artists WHERE source = 'lidarr'")
                .fetch_one(&mut *tx)
                .await?;
        if existing > 0 {
            return Err(RefusedToEmpty(format!(
                "lidarr listed nothing; refusing to remove all {existing} artists imported from it (check the URL)"
            ))
            .into());
        }
    }
    Ok(())
}

/// Adds the artist to the wanted list, or updates its entry when that came from Lidarr;
/// returns the id of the entry holding its MusicBrainz id.
async fn upsert_wanted_artist(
    tx: &mut sqlx::SqliteConnection,
    artist: &crate::arr::LidarrArtist,
    profiles: &HashMap<i64, String>,
    library_id: Option<i64>,
    now: i64,
) -> anyhow::Result<i64> {
    let monitor_new = match artist
        .monitor_new_items
        .as_deref()
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("none") => "none",
        Some("new") => "new",
        _ => "all",
    };
    let profile = artist.quality_profile_id.and_then(|id| profiles.get(&id));
    sqlx::query(
        "INSERT INTO wanted_artists (mb_artist_id, name, monitored, monitor_new, quality_profile,
                 library_id, source, source_id, added_at)
             VALUES (?, ?, ?, ?, ?, ?, 'lidarr', ?, ?)
             ON CONFLICT (mb_artist_id) DO UPDATE SET
                 name = excluded.name,
                 monitored = CASE wanted_artists.monitored_here WHEN 1 THEN wanted_artists.monitored
                                  ELSE excluded.monitored END,
                 monitor_new = CASE wanted_artists.monitor_new_here WHEN 1
                                    THEN wanted_artists.monitor_new ELSE excluded.monitor_new END,
                 quality_profile = excluded.quality_profile,
                 library_id = excluded.library_id, source_id = excluded.source_id
             WHERE wanted_artists.source = 'lidarr'",
    )
    .bind(&artist.foreign_artist_id)
    .bind(&artist.artist_name)
    .bind(artist.monitored)
    .bind(monitor_new)
    .bind(profile)
    .bind(library_id)
    .bind(artist.id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM wanted_artists WHERE mb_artist_id = ?")
        .bind(&artist.foreign_artist_id)
        .fetch_one(&mut *tx)
        .await?;
    Ok(id)
}

/// Adds the album to the wanted list under its artist's entry, or updates its entry when that
/// came from Lidarr.
async fn upsert_wanted_album(
    tx: &mut sqlx::SqliteConnection,
    album: &crate::arr::LidarrAlbum,
    artist: i64,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO wanted_albums (wanted_artist_id, mb_release_group_id, title, album_type,
                 secondary_types, release_date, monitored, source, source_id, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'lidarr', ?, ?)
             ON CONFLICT (mb_release_group_id) DO UPDATE SET
                 wanted_artist_id = excluded.wanted_artist_id, title = excluded.title,
                 album_type = excluded.album_type, secondary_types = excluded.secondary_types,
                 release_date = excluded.release_date,
                 monitored = CASE wanted_albums.monitored_here WHEN 1 THEN wanted_albums.monitored
                                  ELSE excluded.monitored END,
                 source_id = excluded.source_id
             WHERE wanted_albums.source = 'lidarr'",
    )
    .bind(artist)
    .bind(&album.foreign_album_id)
    .bind(&album.title)
    .bind(album.album_type.as_deref().filter(|t| !t.is_empty()))
    .bind(serde_json::to_string(&secondary_types(
        &album.secondary_types,
    ))?)
    .bind(release_date(album.release_date.as_deref()))
    .bind(album.monitored)
    .bind(album.id)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Removes the wanted albums, then artists, from Lidarr that it no longer lists.
async fn remove_unlisted_music(
    tx: &mut sqlx::SqliteConnection,
    listed_albums: &[&str],
    listed_artists: &[&str],
    report: &mut LidarrReport,
) -> anyhow::Result<()> {
    report.removed_albums = sqlx::query(
        "DELETE FROM wanted_albums WHERE source = 'lidarr'
         AND mb_release_group_id NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&listed_albums)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    report.removed_artists = sqlx::query(
        "DELETE FROM wanted_artists WHERE source = 'lidarr'
         AND mb_artist_id NOT IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&listed_artists)?)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    Ok(())
}

/// How many wanted albums from Lidarr are already in a music library.
async fn albums_on_disk(tx: &mut sqlx::SqliteConnection) -> anyhow::Result<i64> {
    let on_disk: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM wanted_albums w WHERE w.source = 'lidarr' AND EXISTS (
             SELECT 1 FROM music_albums m WHERE m.missing_since IS NULL
               AND w.mb_release_group_id IN (m.mb_release_group_id, m.match_release_group_id))",
    )
    .fetch_one(&mut *tx)
    .await?;
    Ok(on_disk)
}
