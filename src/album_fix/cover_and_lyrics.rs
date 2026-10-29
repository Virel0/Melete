//! Files beside the tracks: a front cover from the Cover Art Archive and synced lyrics from LRCLIB,
//! never over anything already there.

use super::*;

/// Fetches the album's front cover into its folder `dir` when no track carries a picture.
/// Returns a note for the message.
pub(super) async fn cover(
    fixes: &Fixes,
    wanted: &Wanted,
    dir: &Path,
    tracks: &[PathBuf],
) -> Result<Option<String>, String> {
    let (Some(client), Some(first)) = (&fixes.coverart, tracks.first()) else {
        return Ok(None);
    };
    let first_track = first.clone();
    let embedded = tokio::task::spawn_blocking(move || {
        crate::library::tags::picture(&first_track).is_ok_and(|p| p.is_some())
    })
    .await
    .unwrap_or(false);
    if embedded {
        return Ok(None);
    }
    let (bytes, content_type) = match client
        .front_sized(&wanted.mb_release_group_id, COVER_SIZE)
        .await
    {
        Ok(Fetched::Image {
            bytes,
            content_type,
        }) => (bytes, content_type),
        Ok(Fetched::None) => return Ok(None),
        Err(err) => {
            tracing::warn!(
                error = format!("{err:#}"),
                "couldn't fetch an album's cover"
            );
            return Ok(Some("no cover: the Cover Art Archive didn't answer".into()));
        }
    };
    let extension = match content_type.split(';').next().unwrap_or_default().trim() {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => return Ok(Some(format!("no cover: it came as {content_type}"))),
    };
    let target = dir.join(format!("cover.{extension}"));
    let written = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o664)
            .open(&target)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o664))
    })
    .await
    .map_err(|err| format!("a cover task failed: {err}"))?;
    match written {
        Ok(()) => Ok(Some("cover fetched".into())),
        // Something is already there: it's never replaced.
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(format!("writing the cover: {err}")),
    }
}

/// Writes each track's synced lyrics from LRCLIB beside it ("01.lrc" for "01.flac"), never
/// over a file already there; `artist` and `album` stand in for tags a track lacks. Returns a
/// note for the message.
pub(super) async fn lyrics(
    client: &lrclib::Client,
    artist: &str,
    album: &str,
    download: i64,
    tracks: &[PathBuf],
) -> Result<String, String> {
    let artist_fallback = artist;
    let mut found = 0;
    for track in tracks {
        let target = track.with_extension("lrc");
        if std::fs::symlink_metadata(&target).is_ok() {
            found += 1;
            continue;
        }
        let path = track.clone();
        let tags = tokio::task::spawn_blocking(move || crate::library::tags::read(&path)).await;
        let Ok(Ok(tags)) = tags else { continue };
        let (Some(title), Some(seconds)) = (tags.title, tags.duration_seconds) else {
            continue;
        };
        let artist = tags.artist.or(tags.album_artist);
        let asked = lrclib::Track {
            title: &title,
            artist: artist.as_deref().unwrap_or(artist_fallback),
            album: tags.album.as_deref().unwrap_or(album),
            seconds: seconds.round() as u64,
        };
        let text = match client.synced(&asked).await {
            Ok(lrclib::Found::Synced(text)) => text,
            Ok(lrclib::Found::None) => continue,
            Err(err) => {
                tracing::warn!(error = format!("{err:#}"), "couldn't ask LRCLIB for lyrics");
                return Ok("no lyrics: LRCLIB didn't answer".into());
            }
        };
        let temp = temporary(&target, download);
        tokio::task::spawn_blocking(move || write_new(&temp, &target, text.as_bytes()))
            .await
            .map_err(|err| format!("a lyrics task failed: {err}"))??;
        found += 1;
    }
    Ok(format!(
        "synced lyrics for {found} of {} tracks",
        tracks.len()
    ))
}

/// Puts `bytes` at `target` by way of `temp`, never replacing anything at `target`.
pub(super) fn write_new(temp: &Path, target: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let written = (|| {
        match std::fs::symlink_metadata(temp) {
            Ok(_) => std::fs::remove_file(temp)?,
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::set_permissions(temp, std::fs::Permissions::from_mode(0o664))
    })();
    let placed = written
        .map_err(|err| format!("writing {}: {err}", target.display()))
        .and_then(|()| crate::transfer::move_without_replacing(temp, target));
    if placed.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    placed
}
