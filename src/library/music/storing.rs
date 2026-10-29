//! Saving a scan in one transaction: tracks, the ones gone missing, and the albums and artists
//! worked out afresh.

use super::*;

pub(super) async fn store(
    pool: &SqlitePool,
    library: &Library,
    found: &[FoundFile],
    mut read: HashMap<String, Result<Tags, String>>,
    artwork: &HashMap<String, Artwork>,
    (skipped, ignored): (usize, &[String]),
) -> anyhow::Result<ScanReport> {
    let now = unix_seconds(Some(SystemTime::now()));
    let mut tx = pool.begin().await?;
    let generation: i64 = sqlx::query_scalar(
        "UPDATE libraries SET scan_generation = scan_generation + 1, last_scan_at = ?
         WHERE id = ? RETURNING scan_generation",
    )
    .bind(now)
    .bind(library.id)
    .fetch_one(&mut *tx)
    .await?;
    let mut report = ScanReport {
        skipped,
        ..ScanReport::default()
    };

    for file in found {
        report.files += 1;
        let Some(result) = read.remove(&file.rel_path) else {
            // Unchanged since it was last read.
            mark_seen(&mut tx, library, file, generation).await?;
            continue;
        };
        let first_seen = upsert_track(&mut tx, library, file, result, generation).await?;
        if first_seen == generation {
            report.added += 1;
        }
    }
    // Tracks under a folder marked to be ignored stay as they were, present.
    super::super::walk::keep_ignored(
        &mut tx,
        "music_tracks",
        "rel_path",
        (library.id, generation),
        ignored,
    )
    .await?;
    report.newly_missing = mark_tracks_missing(&mut tx, library, now, generation).await?;

    store_albums(&mut tx, library, artwork, generation, &mut report).await?;
    sqlx::query(
        "UPDATE music_albums SET missing_since = ?
         WHERE library_id = ? AND seen_generation < ? AND missing_since IS NULL",
    )
    .bind(now)
    .bind(library.id)
    .bind(generation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(report)
}

/// Marks a track unchanged since it was last read as seen in this scan, and present.
async fn mark_seen(
    tx: &mut sqlx::SqliteConnection,
    library: &Library,
    file: &FoundFile,
    generation: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE music_tracks SET seen_generation = ?, missing_since = NULL
                 WHERE library_id = ? AND rel_path = ?",
    )
    .bind(generation)
    .bind(library.id)
    .bind(&file.rel_path)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Stores a track read in this scan, with its tags or the error reading them; returns the
/// generation it was first seen in.
async fn upsert_track(
    tx: &mut sqlx::SqliteConnection,
    library: &Library,
    file: &FoundFile,
    result: Result<Tags, String>,
    generation: i64,
) -> anyhow::Result<i64> {
    let (tags, read_error) = match result {
        Ok(tags) => (tags, None),
        Err(err) => (Tags::default(), Some(err)),
    };
    let named = from_name(&file.rel_path);
    let first_seen: i64 = sqlx::query_scalar(
            "INSERT INTO music_tracks (library_id, rel_path, size, modified_at, first_seen_generation,
                 seen_generation, tagged, read_error, title, artist, album_artist, album,
                 track_number, disc_number, year, genres, compilation, duration_seconds, codec,
                 bitrate_kbps, sample_rate, bit_depth, channels, isrc, mb_recording_id, mb_track_id,
                 mb_release_id, mb_release_group_id, mb_artist_id, mb_album_artist_id, has_picture,
                 encoder_delay, padding, composer, work, movement, movement_number, movement_total,
                 tags_version, artist_names, mb_artist_ids)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                 ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, rel_path) DO UPDATE SET
                 size = excluded.size, modified_at = excluded.modified_at,
                 seen_generation = excluded.seen_generation, missing_since = NULL,
                 tagged = excluded.tagged, read_error = excluded.read_error,
                 title = excluded.title, artist = excluded.artist,
                 album_artist = excluded.album_artist, album = excluded.album,
                 track_number = excluded.track_number, disc_number = excluded.disc_number,
                 year = excluded.year, genres = excluded.genres, compilation = excluded.compilation,
                 duration_seconds = excluded.duration_seconds, codec = excluded.codec,
                 bitrate_kbps = excluded.bitrate_kbps, sample_rate = excluded.sample_rate,
                 bit_depth = excluded.bit_depth, channels = excluded.channels, isrc = excluded.isrc,
                 mb_recording_id = excluded.mb_recording_id, mb_track_id = excluded.mb_track_id,
                 mb_release_id = excluded.mb_release_id,
                 mb_release_group_id = excluded.mb_release_group_id,
                 mb_artist_id = excluded.mb_artist_id,
                 mb_album_artist_id = excluded.mb_album_artist_id,
                 has_picture = excluded.has_picture,
                 encoder_delay = excluded.encoder_delay, padding = excluded.padding,
                 composer = excluded.composer, work = excluded.work,
                 movement = excluded.movement, movement_number = excluded.movement_number,
                 movement_total = excluded.movement_total,
                 tags_version = excluded.tags_version, artist_names = excluded.artist_names,
                 mb_artist_ids = excluded.mb_artist_ids,
                 -- A changed file may have moved: its release ids are linked again. One only
                 -- read again for a new field keeps them.
                 match_recording_id = CASE WHEN music_tracks.size = excluded.size
                     AND music_tracks.modified_at = excluded.modified_at
                     THEN music_tracks.match_recording_id END,
                 match_track_id = CASE WHEN music_tracks.size = excluded.size
                     AND music_tracks.modified_at = excluded.modified_at
                     THEN music_tracks.match_track_id END
             RETURNING first_seen_generation",
        )
        .bind(library.id)
        .bind(&file.rel_path)
        .bind(file.size)
        .bind(file.modified)
        .bind(generation)
        .bind(generation)
        .bind(tags.tagged)
        .bind(read_error)
        .bind(tags.title.as_deref().unwrap_or(&named.title))
        .bind(&tags.artist)
        .bind(&tags.album_artist)
        .bind(&tags.album)
        .bind(tags.track_number.or(named.track_number))
        .bind(tags.disc_number)
        .bind(tags.year)
        .bind(serde_json::to_string(&tags.genres)?)
        .bind(tags.compilation)
        .bind(tags.duration_seconds)
        .bind(tags.codec)
        .bind(tags.bitrate_kbps)
        .bind(tags.sample_rate)
        .bind(tags.bit_depth)
        .bind(tags.channels)
        .bind(&tags.isrc)
        .bind(&tags.mb_recording_id)
        .bind(&tags.mb_track_id)
        .bind(&tags.mb_release_id)
        .bind(&tags.mb_release_group_id)
        .bind(&tags.mb_artist_id)
        .bind(&tags.mb_album_artist_id)
        .bind(tags.has_picture)
        .bind(tags.gapless.map(|g| g.encoder_delay))
        .bind(tags.gapless.and_then(|g| g.padding))
        .bind(&tags.composer)
        .bind(&tags.work)
        .bind(&tags.movement)
        .bind(tags.movement_number)
        .bind(tags.movement_total)
        .bind(TAGS_VERSION)
        .bind(serde_json::to_string(&tags.artists)?)
        .bind(serde_json::to_string(&tags.mb_artist_ids)?)
        .fetch_one(&mut *tx)
        .await?;
    Ok(first_seen)
}

/// Marks the tracks this scan didn't see as missing since now; returns how many.
async fn mark_tracks_missing(
    tx: &mut sqlx::SqliteConnection,
    library: &Library,
    now: i64,
    generation: i64,
) -> anyhow::Result<u64> {
    let newly_missing = sqlx::query(
        "UPDATE music_tracks SET missing_since = ?
         WHERE library_id = ? AND seen_generation < ? AND missing_since IS NULL",
    )
    .bind(now)
    .bind(library.id)
    .bind(generation)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    Ok(newly_missing)
}

/// Works out the albums and their artists from every present track and links each track to
/// its album.
async fn store_albums(
    tx: &mut sqlx::SqliteConnection,
    library: &Library,
    artwork: &HashMap<String, Artwork>,
    generation: i64,
    report: &mut ScanReport,
) -> anyhow::Result<()> {
    // Albums are worked out afresh from every present track, so a retagged file moves.
    let tracks: Vec<TrackFacts> = sqlx::query_as(
        "SELECT id, rel_path, tagged, artist, album_artist, album, year, compilation,
             mb_release_id, mb_release_group_id, mb_artist_id, mb_album_artist_id
         FROM music_tracks WHERE library_id = ? AND missing_since IS NULL",
    )
    .bind(library.id)
    .fetch_all(&mut *tx)
    .await?;
    report.unparsed = tracks.iter().filter(|t| !t.tagged).count();
    let mut groups: HashMap<String, Vec<&TrackFacts>> = HashMap::new();
    for track in &tracks {
        groups.entry(album_key(track)).or_default().push(track);
    }
    for (key, members) in &groups {
        upsert_album(&mut *tx, library, key, members, artwork, generation).await?;
    }
    Ok(())
}

/// Stores one album worked out from its tracks, with its artist, and links the tracks to it.
async fn upsert_album(
    tx: &mut sqlx::SqliteConnection,
    library: &Library,
    key: &str,
    members: &[&TrackFacts],
    artwork: &HashMap<String, Artwork>,
    generation: i64,
) -> anyhow::Result<()> {
    let album = album_of(members);
    let artist_id = upsert_artist(&mut *tx, &album).await?;
    let album_id: i64 = sqlx::query_scalar(
        "INSERT INTO music_albums (library_id, key, title, artist_id, artist_name, year, folder,
                 compilation, mb_release_id, mb_release_group_id, seen_generation, cover_file,
                 motion_file)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, key) DO UPDATE SET
                 title = excluded.title, artist_id = excluded.artist_id,
                 artist_name = excluded.artist_name, year = excluded.year, folder = excluded.folder,
                 compilation = excluded.compilation, mb_release_id = excluded.mb_release_id,
                 mb_release_group_id = excluded.mb_release_group_id,
                 seen_generation = excluded.seen_generation, missing_since = NULL,
                 cover_file = excluded.cover_file, motion_file = excluded.motion_file
             RETURNING id",
    )
    .bind(library.id)
    .bind(key)
    .bind(&album.title)
    .bind(artist_id)
    .bind(album.artist.as_deref().unwrap_or(VARIOUS))
    .bind(album.year)
    .bind(&album.folder)
    .bind(album.artist.is_none())
    .bind(&album.mb_release_id)
    .bind(&album.mb_release_group_id)
    .bind(generation)
    .bind(artwork.get(&album.folder).and_then(|a| a.cover.as_deref()))
    .bind(artwork.get(&album.folder).and_then(|a| a.motion.as_deref()))
    .fetch_one(&mut *tx)
    .await?;
    for track in members {
        sqlx::query("UPDATE music_tracks SET album_id = ? WHERE id = ?")
            .bind(album_id)
            .bind(track.id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

/// Stores the album's artist; None for an album without one (various artists).
async fn upsert_artist(
    tx: &mut sqlx::SqliteConnection,
    album: &AlbumFacts,
) -> anyhow::Result<Option<i64>> {
    let artist_id: Option<i64> = match &album.artist {
        Some(artist) => Some(
            sqlx::query_scalar(
                "INSERT INTO music_artists (key, name, mb_artist_id) VALUES (?, ?, ?)
                     ON CONFLICT (key) DO UPDATE SET
                         mb_artist_id = coalesce(excluded.mb_artist_id, music_artists.mb_artist_id)
                     RETURNING id",
            )
            .bind(artist_key(artist, album.mb_artist_id.as_deref()))
            .bind(artist)
            .bind(&album.mb_artist_id)
            .fetch_one(&mut *tx)
            .await?,
        ),
        None => None,
    };
    Ok(artist_id)
}
