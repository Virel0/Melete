//! Approving a music request: putting the artist, album or song on the wanted list, with what
//! MusicBrainz knows of it (an artist's studio albums, a song's album).

use super::*;

/// Puts an approved music request on the wanted list; returns the requests column it links
/// through and the wanted item's id.
pub async fn approve(
    pool: &SqlitePool,
    mb: Option<&musicbrainz::Client>,
    asked: &Asked,
    choice: &Choice,
) -> anyhow::Result<Result<(&'static str, i64), Refused>> {
    let column = match asked.kind.as_str() {
        "artist" => "wanted_artist_id",
        "album" => "wanted_album_id",
        _ => "wanted_song_id",
    };
    // A song wanted already, or an album or artist looked out for already: linked as it is.
    if let Some(id) = wanted_already(pool, asked).await? {
        return Ok(Ok((column, id)));
    }
    let (library, profile) = match place(pool, choice).await? {
        Ok(placed) => placed,
        Err(refused) => return Ok(Err(refused)),
    };
    let id = match asked.kind.as_str() {
        "artist" => approve_artist(pool, mb, asked, library, profile).await?,
        "album" => approve_album(pool, mb, asked, library, profile).await?,
        _ => approve_song(pool, mb, asked, library, profile).await?,
    };
    Ok(id.map(|id| (column, id)))
}

/// A MusicBrainz lookup that failed: refused when MusicBrainz no longer has it.
fn approval_lookup_error<T>(err: musicbrainz::Error) -> anyhow::Result<Result<T, Refused>> {
    match err {
        musicbrainz::Error::NotFound => Ok(Err(Refused::Unavailable(
            "MusicBrainz no longer has it".into(),
        ))),
        other => Err(anyhow::anyhow!("MusicBrainz: {other}")),
    }
}

/// Wants an artist with every album MusicBrainz lists for it.
async fn approve_artist(
    pool: &SqlitePool,
    mb: Option<&musicbrainz::Client>,
    asked: &Asked,
    library: i64,
    profile: i64,
) -> anyhow::Result<Result<i64, Refused>> {
    let Some(mb) = mb else {
        return Ok(Err(Refused::NoMusicBrainz));
    };
    let artist_id = asked.mb_artist_id.as_deref().unwrap_or_default();
    let albums = match mb.artist_albums(artist_id).await {
        Ok(albums) => albums,
        Err(err) => return approval_lookup_error(err),
    };
    let artist = want_artist(pool, artist_id, &asked.title, "all", library, profile).await?;
    for album in &albums {
        want_album(pool, artist, album).await?;
    }
    Ok(Ok(artist))
}

/// Wants an album as MusicBrainz lists it, and its artist with no other albums.
async fn approve_album(
    pool: &SqlitePool,
    mb: Option<&musicbrainz::Client>,
    asked: &Asked,
    library: i64,
    profile: i64,
) -> anyhow::Result<Result<i64, Refused>> {
    let Some(mb) = mb else {
        return Ok(Err(Refused::NoMusicBrainz));
    };
    let group = match mb
        .release_group(asked.mb_release_group_id.as_deref().unwrap_or_default())
        .await
    {
        Ok(group) => group,
        Err(err) => return approval_lookup_error(err),
    };
    let Some((artist_id, name)) = first_artist(&group.artist_credit) else {
        return Ok(Err(Refused::Unavailable(
            "MusicBrainz names no artist for it".into(),
        )));
    };
    let artist = want_artist(pool, &artist_id, &name, "none", library, profile).await?;
    let as_listed = musicbrainz::ReleaseGroup {
        id: group.id,
        title: Some(group.title),
        primary_type: group.primary_type,
        secondary_types: group.secondary_types,
        first_release_date: group.first_release_date,
    };
    Ok(Ok(want_album(pool, artist, &as_listed).await?))
}

/// Wants a song, with its length and album when MusicBrainz knows them.
async fn approve_song(
    pool: &SqlitePool,
    mb: Option<&musicbrainz::Client>,
    asked: &Asked,
    library: i64,
    profile: i64,
) -> anyhow::Result<Result<i64, Refused>> {
    // A song from MusicBrainz knows its length and album; one asked for by name doesn't.
    let (length, album) = match (asked.mb_recording_id.as_deref(), mb) {
        (Some(id), Some(mb)) => match mb.recording(id).await {
            Ok(song) => {
                let on = song
                    .releases
                    .iter()
                    .find(|r| {
                        r.status.as_deref() == Some("Official")
                            && r.release_group
                                .as_ref()
                                .and_then(|g| g.primary_type.as_deref())
                                == Some("Album")
                    })
                    .or(song.releases.first());
                (
                    song.length.map(|ms| (ms + 500) / 1000),
                    on.map(|r| {
                        (
                            r.title.clone(),
                            r.release_group.as_ref().map(|g| g.id.clone()),
                        )
                    }),
                )
            }
            Err(err) => return approval_lookup_error(err),
        },
        (Some(_), None) => return Ok(Err(Refused::NoMusicBrainz)),
        (None, _) => (None, None),
    };
    let (album, group) = album.unzip();
    Ok(Ok(sqlx::query_scalar(
        "INSERT INTO wanted_songs (title, artist, album, length_seconds, mb_recording_id,
                     mb_artist_id, mb_release_group_id, library_id, music_profile_id, source, added_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'manual', ?) RETURNING id",
    )
    .bind(&asked.title)
    .bind(asked.artist.as_deref().unwrap_or_default())
    .bind(album)
    .bind(length.filter(|l| *l > 0))
    .bind(&asked.mb_recording_id)
    .bind(&asked.mb_artist_id)
    .bind(group.flatten())
    .bind(library)
    .bind(profile)
    .bind(unix_now())
    .fetch_one(pool)
    .await?))
}
