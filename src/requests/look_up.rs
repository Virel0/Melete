//! Finding what's asked for: a movie or series on TMDB, music on MusicBrainz.

use super::*;

/// What a request is for, as looked up.
pub(super) struct Found {
    pub(super) title: String,
    pub(super) year: Option<i64>,
    pub(super) poster: Option<String>,
    pub(super) artist: Option<String>,
    pub(super) tmdb_id: Option<i64>,
    pub(super) mb_artist_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    pub(super) mb_recording_id: Option<String>,
    /// For a series: whether TMDB has it as anime (animated and Japanese); None otherwise.
    pub(super) anime: Option<bool>,
}

pub(super) async fn look_up_title(
    tmdb: Option<&tmdb::Client>,
    new: &NewRequest,
) -> anyhow::Result<Result<Found, Refused>> {
    let tmdb_id = match new.tmdb_id {
        Some(id) if id > 0 => id,
        _ => {
            return Ok(Err(Refused::Invalid(
                "tmdb_id is needed for a movie or series".into(),
            )));
        }
    };
    if new.kind == "movie" && new.season.is_some() {
        return Ok(Err(Refused::Invalid(
            "a season is only for a series".into(),
        )));
    }
    if new.season.is_some_and(|s| !(0..=10_000).contains(&s)) {
        return Ok(Err(Refused::Invalid("season is out of range".into())));
    }
    let music = [
        &new.mb_artist_id,
        &new.mb_release_group_id,
        &new.mb_recording_id,
        &new.title,
        &new.artist,
    ];
    if music.iter().any(|v| v.is_some()) {
        return Ok(Err(Refused::Invalid(
            "a movie or series is asked for by tmdb_id alone".into(),
        )));
    }
    let Some(tmdb) = tmdb else {
        return Ok(Err(Refused::NoMetadata));
    };
    let found = if new.kind == "movie" {
        tmdb.movie(tmdb_id)
            .await?
            .map(|m| (m.title, m.release_date, m.poster_path, None))
    } else {
        tmdb.series(tmdb_id).await?.map(|s| {
            let anime = want::is_anime(&s);
            (s.name, s.first_air_date, s.poster_path, Some(anime))
        })
    };
    let Some((title, date, poster, anime)) = found else {
        return Ok(Err(Refused::Invalid("TMDB has no such title".into())));
    };
    Ok(Ok(Found {
        title,
        year: date.as_deref().and_then(want::year_of),
        poster: poster.filter(|p| p.starts_with('/') && !p.contains("..")),
        artist: None,
        tmdb_id: Some(tmdb_id),
        mb_artist_id: None,
        mb_release_group_id: None,
        mb_recording_id: None,
        anime,
    }))
}

pub(super) async fn look_up_music(
    mb: Option<&musicbrainz::Client>,
    new: &NewRequest,
) -> anyhow::Result<Result<Found, Refused>> {
    if new.tmdb_id.is_some() || new.season.is_some() {
        return Ok(Err(Refused::Invalid(
            "tmdb_id and season are only for a movie or series".into(),
        )));
    }
    // Each kind is asked for by its own id; a song by name has its title and artist.
    let (id, others) = match new.kind.as_str() {
        "artist" => (
            &new.mb_artist_id,
            [
                &new.mb_release_group_id,
                &new.mb_recording_id,
                &new.title,
                &new.artist,
            ],
        ),
        "album" => (
            &new.mb_release_group_id,
            [
                &new.mb_artist_id,
                &new.mb_recording_id,
                &new.title,
                &new.artist,
            ],
        ),
        _ if new.mb_recording_id.is_some() => (
            &new.mb_recording_id,
            [
                &new.mb_artist_id,
                &new.mb_release_group_id,
                &new.title,
                &new.artist,
            ],
        ),
        _ => (
            &None,
            [&new.mb_artist_id, &new.mb_release_group_id, &None, &None],
        ),
    };
    if others.iter().any(|v| v.is_some()) {
        let what = match new.kind.as_str() {
            "artist" => "an artist is asked for by mb_artist_id alone",
            "album" => "an album is asked for by mb_release_group_id alone",
            _ => "a song is asked for by mb_recording_id alone, or by title and artist",
        };
        return Ok(Err(Refused::Invalid(what.into())));
    }
    let looked = match music_requests::look_up(
        mb,
        &new.kind,
        id.as_deref(),
        new.title.as_deref(),
        new.artist.as_deref(),
    )
    .await?
    {
        Ok(looked) => looked,
        Err(refused) => return Ok(Err(refused)),
    };
    Ok(Ok(Found {
        title: looked.title,
        year: looked.year,
        poster: None,
        artist: looked.artist,
        tmdb_id: None,
        mb_artist_id: looked.mb_artist_id,
        mb_release_group_id: looked.mb_release_group_id,
        mb_recording_id: looked.mb_recording_id,
        anime: None,
    }))
}
