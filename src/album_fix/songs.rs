//! Fixing the one track a download of a wanted song placed.

use super::*;

/// The song as it's wanted: what fills its track's tags' gaps.
#[derive(Debug, sqlx::FromRow)]
pub(super) struct WantedSong {
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) album: Option<String>,
    pub(super) mb_recording_id: Option<String>,
    pub(super) mb_artist_id: Option<String>,
}

/// Fixes the one track a download of wanted song `song` placed: its title,
/// artist, album and MusicBrainz ids where its tags lack them, its ReplayGain (a track's only:
/// one track says nothing of an album's loudness), and its synced lyrics when they're on.
/// Returns the download's message; fails only when the library can't be written.
pub async fn fix_song(
    pool: &SqlitePool,
    fixes: &Fixes,
    download: i64,
    song: i64,
    track: &Path,
) -> Result<String, String> {
    let wanted: Option<WantedSong> = sqlx::query_as(
        "SELECT title, artist, album, mb_recording_id, mb_artist_id FROM wanted_songs WHERE id = ?",
    )
    .bind(song)
    .fetch_optional(pool)
    .await
    .map_err(|err| format!("the database: {err}"))?;
    let Some(wanted) = wanted else {
        return Ok("in the library".into());
    };
    let mut notes: Vec<String> = Vec::new();
    let tracks = [track.to_path_buf()];
    let (loudness, _) = measure(&fixes.tools, &tracks, &mut notes).await;
    let loudness = loudness.into_iter().next().flatten();
    let mut fill = vec![
        (ItemKey::TrackTitle, wanted.title.clone()),
        (ItemKey::TrackArtist, wanted.artist.clone()),
    ];
    if let Some(album) = &wanted.album {
        fill.push((ItemKey::AlbumTitle, album.clone()));
        fill.push((ItemKey::AlbumArtist, wanted.artist.clone()));
    }
    for (key, id) in [
        (ItemKey::MusicBrainzRecordingId, &wanted.mb_recording_id),
        (ItemKey::MusicBrainzArtistId, &wanted.mb_artist_id),
    ] {
        if let Some(id) = id {
            fill.push((key, id.clone()));
        }
    }
    let mut set = Vec::new();
    if let Some(track) = loudness {
        set.push((ItemKey::ReplayGainTrackGain, gain(track)));
        set.push((ItemKey::ReplayGainTrackPeak, peak(track)));
    }
    let writes = Writes { fill, set };
    let (path, temp) = (track.to_path_buf(), temporary(track, download));
    let retagged = tokio::task::spawn_blocking(move || retag(&path, &temp, &writes))
        .await
        .map_err(|err| format!("a tagging task failed: {err}"))?;
    match retagged {
        Ok(_) => {
            if loudness.is_some() {
                notes.insert(0, "ReplayGain added".into());
            }
        }
        Err(Retag::Tags(why)) => {
            tracing::warn!(file = %track.display(), error = why, "couldn't tag a track");
            notes.push(format!("couldn't tag {}", file_name(track)));
        }
        Err(Retag::Library(err)) => {
            return Err(format!("tagging {}: {err}", track.display()));
        }
    }
    if let Some(client) = &fixes.lyrics {
        let album = wanted.album.as_deref().unwrap_or(&wanted.title);
        notes.push(lyrics(client, &wanted.artist, album, download, &tracks).await?);
    }
    Ok(match notes.as_slice() {
        [] => "in the library".into(),
        notes => format!("in the library; {}", notes.join("; ")),
    })
}
