//! Music requests: an artist, an album or a song, looked up on
//! MusicBrainz, and put on the wanted list once approved. A whole artist is followed for new
//! releases and wants every studio album; an album wants its artist too, followed for nothing
//! else; a song is wanted on its own. They go where the music defaults say, else
//! to the only music library, decided with the default music profile, else the preset that
//! prefers FLAC.

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::music_profiles;
use crate::musicbrainz::{self, Credit};
use crate::requests::{Choice, Refused};
use crate::want::year_of;

mod approving;

pub use approving::*;

/// What a music request says about itself, from MusicBrainz or as asked.
#[derive(Debug, Default, PartialEq)]
pub struct Looked {
    /// What's asked for by name: the artist's own name, or the album's or song's title.
    pub title: String,
    /// The album's year, from its first release; none for an artist or a song.
    pub year: Option<i64>,
    /// The album's or song's artist; none for an artist.
    pub artist: Option<String>,
    /// The artist's MusicBrainz id, when known.
    pub mb_artist_id: Option<String>,
    /// The album's MusicBrainz release group id, for an album.
    pub mb_release_group_id: Option<String>,
    /// The song's MusicBrainz recording id, for a song looked up by one.
    pub mb_recording_id: Option<String>,
}

fn first_artist(credits: &[Credit]) -> Option<(String, String)> {
    credits
        .first()
        .map(|c| (c.artist.id.clone(), c.artist.name.clone()))
}

fn lookup_error(err: musicbrainz::Error) -> anyhow::Result<Result<Looked, Refused>> {
    match err {
        musicbrainz::Error::NotFound => Ok(Err(Refused::Invalid(
            "MusicBrainz has no such thing".into(),
        ))),
        other => Err(anyhow::anyhow!("MusicBrainz: {other}")),
    }
}

/// Looks a music request up: an artist or album by its MusicBrainz id, a song by its
/// recording's, or as `title` by `artist` when it has none.
pub async fn look_up(
    mb: Option<&musicbrainz::Client>,
    kind: &str,
    id: Option<&str>,
    title: Option<&str>,
    artist: Option<&str>,
) -> anyhow::Result<Result<Looked, Refused>> {
    // A song asked for by name needs no lookup.
    if kind == "song" && id.is_none() {
        let text = |v: Option<&str>, field: &str| -> Result<String, Refused> {
            let v = v.map(str::trim).unwrap_or_default();
            if v.is_empty() || v.chars().count() > 300 {
                return Err(Refused::Invalid(format!(
                    "{field} must be 1 to 300 characters"
                )));
            }
            Ok(v.to_owned())
        };
        let (title, artist) = match (text(title, "title"), text(artist, "artist")) {
            (Ok(t), Ok(a)) => (t, a),
            (Err(e), _) | (_, Err(e)) => return Ok(Err(e)),
        };
        return Ok(Ok(Looked {
            title,
            artist: Some(artist),
            ..Looked::default()
        }));
    }
    let field = match kind {
        "artist" => "mb_artist_id",
        "album" => "mb_release_group_id",
        _ => "mb_recording_id",
    };
    let Some(id) = id else {
        return Ok(Err(Refused::Invalid(format!(
            "{field} is needed for an {kind}"
        ))));
    };
    if !crate::library::tags::is_mbid(id) {
        return Ok(Err(Refused::Invalid(format!(
            "{field} isn't a MusicBrainz id"
        ))));
    }
    let Some(mb) = mb else {
        return Ok(Err(Refused::NoMusicBrainz));
    };
    match kind {
        "artist" => match mb.artist(id).await {
            Ok(artist) => Ok(Ok(Looked {
                title: artist.name,
                mb_artist_id: Some(artist.id),
                ..Looked::default()
            })),
            Err(err) => lookup_error(err),
        },
        "album" => match mb.release_group(id).await {
            Ok(group) => {
                let artist = first_artist(&group.artist_credit);
                Ok(Ok(Looked {
                    title: group.title,
                    year: group.first_release_date.as_deref().and_then(year_of),
                    artist: artist.as_ref().map(|(_, name)| name.clone()),
                    mb_artist_id: artist.map(|(id, _)| id),
                    mb_release_group_id: Some(group.id),
                    ..Looked::default()
                }))
            }
            Err(err) => lookup_error(err),
        },
        _ => match mb.recording(id).await {
            Ok(song) => {
                let artist = first_artist(&song.artist_credit);
                Ok(Ok(Looked {
                    title: song.title,
                    artist: artist.as_ref().map(|(_, name)| name.clone()),
                    mb_artist_id: artist.map(|(id, _)| id),
                    mb_recording_id: Some(song.id),
                    ..Looked::default()
                }))
            }
            Err(err) => lookup_error(err),
        },
    }
}

/// What's asked for, as the request stores it.
#[derive(sqlx::FromRow)]
pub struct Asked {
    /// `artist`, `album` or `song`.
    pub kind: String,
    /// The artist's name, or the album's or song's title.
    pub title: String,
    /// The album's or song's artist; none for an artist.
    pub artist: Option<String>,
    /// The artist's MusicBrainz id, when known.
    pub mb_artist_id: Option<String>,
    /// The album's MusicBrainz release group id, for an album.
    pub mb_release_group_id: Option<String>,
    /// The song's MusicBrainz recording id, when it was looked up by one.
    pub mb_recording_id: Option<String>,
}

/// Whether it's on the wanted list already and looked out for: its id there.
pub async fn wanted_already(pool: &SqlitePool, asked: &Asked) -> sqlx::Result<Option<i64>> {
    match asked.kind.as_str() {
        // An artist followed for one album isn't the whole artist asked for.
        "artist" => {
            sqlx::query_scalar(
                "SELECT id FROM wanted_artists WHERE mb_artist_id = ? AND monitored = 1 AND monitor_new = 'all'",
            )
                .bind(&asked.mb_artist_id)
                .fetch_optional(pool)
                .await
        }
        "album" => {
            sqlx::query_scalar(
                "SELECT w.id FROM wanted_albums w JOIN wanted_artists a ON a.id = w.wanted_artist_id
                 WHERE w.mb_release_group_id = ? AND w.monitored = 1 AND a.monitored = 1",
            )
            .bind(&asked.mb_release_group_id)
            .fetch_optional(pool)
            .await
        }
        _ => {
            sqlx::query_scalar(
                "SELECT id FROM wanted_songs
                 WHERE (?1 IS NOT NULL AND mb_recording_id = ?1)
                    OR (title = ?2 COLLATE NOCASE AND artist = ?3 COLLATE NOCASE)
                 ORDER BY id LIMIT 1",
            )
            .bind(&asked.mb_recording_id)
            .bind(&asked.title)
            .bind(&asked.artist)
            .fetch_optional(pool)
            .await
        }
    }
}

/// Where approved music goes and how it's decided: the approver's choice, else the music
/// defaults, else the only music library and the preset that prefers FLAC.
async fn place(pool: &SqlitePool, choice: &Choice) -> anyhow::Result<Result<(i64, i64), Refused>> {
    let set: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT library_id, music_profile_id FROM request_defaults WHERE kind = 'music'",
    )
    .fetch_optional(pool)
    .await?;
    let (set_library, set_profile) = set.unwrap_or_default();
    let library = match choice.library_id.or(set_library) {
        Some(id) => {
            let kind: Option<String> =
                sqlx::query_scalar("SELECT kind FROM libraries WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
            if kind.as_deref() != Some("music") {
                return Ok(Err(Refused::Invalid(format!(
                    "library {id} isn't a music library"
                ))));
            }
            id
        }
        None => {
            let only: Vec<i64> =
                sqlx::query_scalar("SELECT id FROM libraries WHERE kind = 'music' ORDER BY id")
                    .fetch_all(pool)
                    .await?;
            match only.as_slice() {
                [one] => *one,
                [] => {
                    return Ok(Err(Refused::NoLibrary(
                        "there's no music library to put it in".into(),
                    )));
                }
                _ => {
                    return Ok(Err(Refused::NoLibrary(
                        "there's more than one music library; choose one, or set where approved music goes".into(),
                    )));
                }
            }
        }
    };
    let profile = match choice.music_profile_id.or(set_profile) {
        Some(id) => {
            if music_profiles::get(pool, id).await?.is_none() {
                return Ok(Err(Refused::Invalid(format!(
                    "there's no music profile {id}"
                ))));
            }
            id
        }
        None => music_profiles::lossless(pool).await?,
    };
    Ok(Ok((library, profile)))
}

/// The wanted artist with this MusicBrainz id, made if it's new and looked out for either way.
async fn want_artist(
    pool: &SqlitePool,
    mb_artist_id: &str,
    name: &str,
    monitor_new: &str,
    library: i64,
    profile: i64,
) -> sqlx::Result<i64> {
    // Asked for: looked out for, and new releases too when the whole artist was asked for.
    sqlx::query_scalar(
        "INSERT INTO wanted_artists (mb_artist_id, name, monitored, monitor_new, library_id,
             profile_id, source, added_at)
         VALUES (?, ?, 1, ?, ?, ?, 'manual', ?)
         ON CONFLICT (mb_artist_id) DO UPDATE SET monitored = 1,
             monitor_new = CASE WHEN excluded.monitor_new = 'all' THEN 'all' ELSE monitor_new END
         RETURNING id",
    )
    .bind(mb_artist_id)
    .bind(name)
    .bind(monitor_new)
    .bind(library)
    .bind(profile)
    .bind(unix_now())
    .fetch_one(pool)
    .await
}

async fn want_album(
    pool: &SqlitePool,
    artist: i64,
    group: &musicbrainz::ReleaseGroup,
) -> sqlx::Result<i64> {
    let secondary = serde_json::to_string(group.secondary_types.as_deref().unwrap_or_default())
        .unwrap_or_else(|_| "[]".into());
    sqlx::query_scalar(
        "INSERT INTO wanted_albums (wanted_artist_id, mb_release_group_id, title, album_type,
             secondary_types, release_date, monitored, source, added_at)
         VALUES (?, ?, ?, ?, ?, ?, 1, 'manual', ?)
         ON CONFLICT (mb_release_group_id) DO UPDATE SET monitored = 1
         RETURNING id",
    )
    .bind(artist)
    .bind(&group.id)
    .bind(group.title.as_deref().unwrap_or("Untitled"))
    .bind(&group.primary_type)
    .bind(secondary)
    .bind(&group.first_release_date)
    .bind(unix_now())
    .fetch_one(pool)
    .await
}

/// Wants a song's album: its artist followed for nothing else, the album looked out for,
/// and the song linked to it. None when the song doesn't know its album and artist on
/// MusicBrainz or where it goes; the album's id when it's wanted (or was already).
pub async fn want_song_album(pool: &SqlitePool, song: i64) -> anyhow::Result<Option<i64>> {
    type SongAlbum = (
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let row: Option<SongAlbum> = sqlx::query_as(
        "SELECT mb_release_group_id, mb_artist_id, artist, album, library_id, music_profile_id,
             wanted_album_id
         FROM wanted_songs WHERE id = ?",
    )
    .bind(song)
    .fetch_optional(pool)
    .await?;
    let Some((Some(group), Some(artist_id), artist, Some(album), Some(library), profile, linked)) =
        row
    else {
        return Ok(None);
    };
    if linked.is_some() {
        return Ok(linked);
    }
    let profile = match profile {
        Some(id) => id,
        None => music_profiles::lossless(pool).await?,
    };
    let artist = want_artist(pool, &artist_id, &artist, "none", library, profile).await?;
    let wanted = musicbrainz::ReleaseGroup {
        id: group,
        title: Some(album),
        primary_type: None,
        secondary_types: None,
        first_release_date: None,
    };
    let album = want_album(pool, artist, &wanted).await?;
    sqlx::query("UPDATE wanted_songs SET wanted_album_id = ? WHERE id = ?")
        .bind(album)
        .bind(song)
        .execute(pool)
        .await?;
    Ok(Some(album))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{self, User};
    use crate::db;
    use crate::musicbrainz::testing::{self, OK_COMPUTER, PARANOID_ANDROID, RADIOHEAD};
    use crate::requests::{self, NewRequest, Refused};

    struct Rig {
        pool: SqlitePool,
        mb: musicbrainz::Client,
        admin: User,
        alice: User,
        bob: User,
    }

    async fn rig() -> Rig {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES
                 (1, 'movies', 'movies', '/m'), (4, 'music', 'music', '/music')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let user = |name: &'static str, admin| {
            let pool = pool.clone();
            async move {
                auth::create_user(&pool, name, "a-long-password".into(), admin)
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        let (admin, alice, bob) = (
            user("admin", true).await,
            user("alice", false).await,
            user("bob", false).await,
        );
        sqlx::query("UPDATE users SET requests_approved = 1 WHERE id = ?")
            .bind(bob.id)
            .execute(&pool)
            .await
            .unwrap();
        let (mb, _) = testing::start(0).await;
        Rig {
            pool,
            mb,
            admin,
            alice,
            bob,
        }
    }

    fn album(id: &str) -> NewRequest {
        NewRequest {
            kind: "album".into(),
            mb_release_group_id: Some(id.into()),
            ..NewRequest::default()
        }
    }

    fn artist(id: &str) -> NewRequest {
        NewRequest {
            kind: "artist".into(),
            mb_artist_id: Some(id.into()),
            ..NewRequest::default()
        }
    }

    fn song(id: &str) -> NewRequest {
        NewRequest {
            kind: "song".into(),
            mb_recording_id: Some(id.into()),
            ..NewRequest::default()
        }
    }

    fn song_by_name(title: &str, by: &str) -> NewRequest {
        NewRequest {
            kind: "song".into(),
            title: Some(title.into()),
            artist: Some(by.into()),
            ..NewRequest::default()
        }
    }

    async fn ask(r: &Rig, user: &User, new: NewRequest) -> Result<requests::RequestView, Refused> {
        requests::create(&r.pool, None, Some(&r.mb), user, new)
            .await
            .unwrap()
    }

    async fn approve(r: &Rig, id: i64, choice: &Choice) -> Result<requests::RequestView, Refused> {
        requests::approve(&r.pool, None, Some(&r.mb), id, &r.admin, choice)
            .await
            .unwrap()
    }

    async fn preset(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT id FROM music_profiles WHERE source = 'preset'")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    type ArtistRow = (String, String, bool, String, Option<i64>, Option<i64>);
    type SongRow = (
        String,
        String,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
    );

    async fn artists(pool: &SqlitePool) -> Vec<ArtistRow> {
        sqlx::query_as(
            "SELECT mb_artist_id, name, monitored, monitor_new, library_id, profile_id
             FROM wanted_artists ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn albums(pool: &SqlitePool) -> Vec<(String, bool, Option<String>)> {
        sqlx::query_as("SELECT title, monitored, release_date FROM wanted_albums ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn an_album_request_wants_the_album_and_follows_its_artist_for_nothing_else() {
        let r = rig().await;
        let asked = ask(&r, &r.alice, album(OK_COMPUTER)).await.unwrap();
        assert_eq!(
            (
                asked.state.as_str(),
                asked.title.as_str(),
                asked.year,
                asked.artist.as_deref(),
                asked.mb_artist_id.as_deref(),
                asked.poster.as_deref(),
            ),
            (
                "requested",
                "OK Computer",
                Some(1997),
                Some("Radiohead"),
                Some(RADIOHEAD),
                None
            )
        );
        assert!(
            albums(&r.pool).await.is_empty(),
            "nothing is wanted before approval"
        );

        let approved = approve(&r, asked.id, &Choice::default()).await.unwrap();
        assert_eq!(approved.state, "approved");
        let preset = preset(&r.pool).await;
        assert_eq!(
            artists(&r.pool).await,
            [(
                RADIOHEAD.into(),
                "Radiohead".into(),
                true,
                "none".into(),
                Some(4),
                Some(preset)
            )]
        );
        assert_eq!(
            albums(&r.pool).await,
            [("OK Computer".into(), true, Some("1997-05-21".into()))]
        );
        let album_id: i64 = sqlx::query_scalar("SELECT id FROM wanted_albums")
            .fetch_one(&r.pool)
            .await
            .unwrap();
        assert_eq!(approved.wanted_id, Some(album_id));
        assert!(
            approved.progress.is_some(),
            "an album's progress is followed"
        );

        // Asked for again by someone else once it's wanted: approved at once, by no one.
        let again = ask(&r, &r.admin, album(OK_COMPUTER)).await.unwrap();
        assert_eq!(
            (
                again.state.as_str(),
                again.decided_by.as_deref(),
                again.wanted_id
            ),
            ("approved", None, Some(album_id))
        );
    }

    #[tokio::test]
    async fn an_artist_request_wants_its_studio_albums_and_new_releases() {
        let r = rig().await;
        // Followed for one album only: the whole artist is still a request to decide.
        assert_eq!(
            ask(&r, &r.admin, album(OK_COMPUTER)).await.unwrap().state,
            "approved"
        );
        let asked = ask(&r, &r.alice, artist(RADIOHEAD)).await.unwrap();
        assert_eq!(
            (
                asked.state.as_str(),
                asked.title.as_str(),
                asked.artist.as_deref()
            ),
            ("requested", "Radiohead", None)
        );
        let approved = approve(&r, asked.id, &Choice::default()).await.unwrap();
        assert_eq!(approved.state, "approved");
        assert_eq!(
            approved.progress, None,
            "an artist has no one step of its own"
        );
        let followed = artists(&r.pool).await;
        assert_eq!(
            (followed.len(), followed[0].2, followed[0].3.as_str()),
            (1, true, "all")
        );
        // Every studio album; live albums and singles aren't.
        assert_eq!(
            albums(&r.pool).await,
            [
                ("OK Computer".into(), true, Some("1997-05-21".into())),
                ("Kid A".into(), true, Some("2000-10-02".into())),
            ]
        );

        // Now it's followed whole, someone else asking is approved at once, by no one.
        let again = ask(&r, &r.bob, artist(RADIOHEAD)).await.unwrap();
        assert_eq!(
            (
                again.state.as_str(),
                again.decided_by.as_deref(),
                again.wanted_id
            ),
            ("approved", None, approved.wanted_id)
        );
    }

    #[tokio::test]
    async fn a_song_request_wants_the_song_with_its_album_and_length() {
        let r = rig().await;
        let asked = ask(&r, &r.alice, song(PARANOID_ANDROID)).await.unwrap();
        assert_eq!(
            (asked.title.as_str(), asked.artist.as_deref()),
            ("Paranoid Android", Some("Radiohead"))
        );
        let approved = approve(&r, asked.id, &Choice::default()).await.unwrap();
        let songs: Vec<SongRow> = sqlx::query_as(
            "SELECT title, artist, album, length_seconds, mb_recording_id, library_id
                 FROM wanted_songs ORDER BY id",
        )
        .fetch_all(&r.pool)
        .await
        .unwrap();
        assert_eq!(
            songs,
            [(
                "Paranoid Android".into(),
                "Radiohead".into(),
                Some("OK Computer".into()),
                Some(383),
                Some(PARANOID_ANDROID.into()),
                Some(4),
            )]
        );
        assert!(approved.progress.is_some());
        assert!(artists(&r.pool).await.is_empty(), "a song wants no artist");

        // By name, without MusicBrainz: wanted on its own, its album unknown.
        let by_name = requests::create(
            &r.pool,
            None,
            None,
            &r.bob,
            song_by_name(" Creep ", "Radiohead"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (by_name.state.as_str(), by_name.title.as_str()),
            ("approved", "Creep")
        );
        let album: Option<String> =
            sqlx::query_scalar("SELECT album FROM wanted_songs WHERE title = 'Creep'")
                .fetch_one(&r.pool)
                .await
                .unwrap();
        assert_eq!(album, None);
        // The same song asked for by name, in other case, is the one wanted already.
        let again = ask(&r, &r.alice, song_by_name("creep", "RADIOHEAD"))
            .await
            .unwrap();
        assert_eq!(
            (again.state.as_str(), again.wanted_id),
            ("approved", by_name.wanted_id)
        );
    }

    #[tokio::test]
    async fn music_requests_refused() {
        let r = rig().await;
        let asked = ask(&r, &r.alice, album(OK_COMPUTER)).await.unwrap();
        assert_eq!(
            ask(&r, &r.alice, album(OK_COMPUTER)).await,
            Err(Refused::Already(asked.id))
        );
        assert_eq!(
            requests::create(&r.pool, None, None, &r.alice, artist(RADIOHEAD))
                .await
                .unwrap(),
            Err(Refused::NoMusicBrainz)
        );
        let invalid = |why: &str| Err(Refused::Invalid(why.into()));
        for (new, why) in [
            (
                album("00000000-0000-4000-8000-000000000000"),
                "MusicBrainz has no such thing",
            ),
            (
                album("not-an-id"),
                "mb_release_group_id isn't a MusicBrainz id",
            ),
            (
                NewRequest {
                    kind: "artist".into(),
                    ..NewRequest::default()
                },
                "mb_artist_id is needed for an artist",
            ),
            (
                NewRequest {
                    title: Some("OK Computer".into()),
                    ..album(OK_COMPUTER)
                },
                "an album is asked for by mb_release_group_id alone",
            ),
            (
                NewRequest {
                    tmdb_id: Some(603),
                    ..artist(RADIOHEAD)
                },
                "tmdb_id and season are only for a movie or series",
            ),
            (
                song_by_name("", "Radiohead"),
                "title must be 1 to 300 characters",
            ),
            (
                NewRequest {
                    kind: "song".into(),
                    title: Some("Creep".into()),
                    ..NewRequest::default()
                },
                "artist must be 1 to 300 characters",
            ),
            (
                NewRequest {
                    kind: "movie".into(),
                    tmdb_id: Some(603),
                    mb_artist_id: Some(RADIOHEAD.into()),
                    ..NewRequest::default()
                },
                "a movie or series is asked for by tmdb_id alone",
            ),
        ] {
            assert_eq!(ask(&r, &r.alice, new).await, invalid(why));
        }

        // Where it goes: not a movies library, and with two music libraries, one chosen.
        let not_music = Choice {
            library_id: Some(1),
            ..Choice::default()
        };
        assert_eq!(
            approve(&r, asked.id, &not_music).await,
            invalid("library 1 isn't a music library")
        );
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (5, 'more', 'music', '/more')",
        )
        .execute(&r.pool)
        .await
        .unwrap();
        assert!(matches!(
            approve(&r, asked.id, &Choice::default()).await,
            Err(Refused::NoLibrary(_))
        ));
        let no_profile = Choice {
            library_id: Some(5),
            music_profile_id: Some(999),
            ..Choice::default()
        };
        assert_eq!(
            approve(&r, asked.id, &no_profile).await,
            invalid("there's no music profile 999")
        );
        let chosen = Choice {
            library_id: Some(5),
            ..Choice::default()
        };
        assert_eq!(
            approve(&r, asked.id, &chosen).await.unwrap().state,
            "approved"
        );
        assert_eq!(artists(&r.pool).await[0].4, Some(5));
    }

    #[tokio::test]
    async fn a_songs_album_is_wanted_for_it_once() {
        let r = rig().await;
        let asked = ask(&r, &r.admin, song(PARANOID_ANDROID)).await.unwrap();
        let id = asked.wanted_id.unwrap();
        let group: Option<String> =
            sqlx::query_scalar("SELECT mb_release_group_id FROM wanted_songs WHERE id = ?")
                .bind(id)
                .fetch_one(&r.pool)
                .await
                .unwrap();
        assert_eq!(
            group.as_deref(),
            Some(OK_COMPUTER),
            "the album it's on, from MusicBrainz"
        );

        let album = want_song_album(&r.pool, id).await.unwrap().unwrap();
        assert_eq!(albums(&r.pool).await, [("OK Computer".into(), true, None)]);
        let followed = artists(&r.pool).await;
        assert_eq!(
            (followed[0].0.as_str(), followed[0].3.as_str()),
            (RADIOHEAD, "none"),
            "its artist is followed for nothing else"
        );
        assert_eq!(
            want_song_album(&r.pool, id).await.unwrap(),
            Some(album),
            "once"
        );
        assert_eq!(albums(&r.pool).await.len(), 1);

        // A song asked for by name doesn't know its album.
        let by_name = ask(&r, &r.admin, song_by_name("Creep", "Radiohead"))
            .await
            .unwrap();
        assert_eq!(
            want_song_album(&r.pool, by_name.wanted_id.unwrap())
                .await
                .unwrap(),
            None
        );
    }
}
