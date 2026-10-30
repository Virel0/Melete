//! Finding something to ask for: movies and series on TMDB, artists, albums and
//! songs on MusicBrainz, each saying whether it's in the library already, wanted, or asked for
//! by the one searching. Only the words searched for leave the server.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::access::Access;
use crate::music_requests;
use crate::musicbrainz::{self, Credit};
use crate::requests::Refused;
use crate::tmdb;
use crate::want::year_of;

mod art;
mod soulseek;

pub use soulseek::*;

/// Something that can be asked for.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct Candidate {
    /// "movie", "series", "artist", "album" or "song".
    pub kind: String,
    /// A movie's or series' TMDB id.
    pub tmdb_id: Option<i64>,
    /// The artist's MusicBrainz id: an artist's own, or an album's or song's first credited.
    pub mb_artist_id: Option<String>,
    /// An album's MusicBrainz release group.
    pub mb_release_group_id: Option<String>,
    /// A song's MusicBrainz recording.
    pub mb_recording_id: Option<String>,
    /// The title, or an artist's name.
    pub title: String,
    /// The year it came out; `None` when the source gives no date.
    pub year: Option<i64>,
    /// An album's or song's credited artists, as one line.
    pub artist: Option<String>,
    /// What tells it from others of the name: an original title, an artist's kind and
    /// country, an album's type, the album a song is on and its length.
    pub detail: Option<String>,
    /// Already on disk: the title, the artist, the album or the song.
    pub in_library: bool,
    /// On the wanted list and looked out for.
    pub wanted: bool,
    /// The searcher's own request for it still open (asked for or approved): its id.
    pub requested: Option<i64>,
    /// Its poster or cover's address on this server, when it's here already: a title's TMDB
    /// poster Melete keeps, or an album's cover in the library. Nothing is fetched for one
    /// that isn't, so a search sends out only its words.
    pub poster: Option<String>,
}

fn first_artist(credits: &[Credit]) -> Option<(String, String)> {
    credits
        .first()
        .map(|c| (c.artist.id.clone(), musicbrainz::credit_line(credits)))
}

/// An outside service that didn't answer, logged; the searcher is told only which.
fn outside(which: &'static str, err: impl std::fmt::Display) -> Refused {
    tracing::warn!(error = %err, "{which} search for a request failed");
    Refused::Outside(which)
}

/// The answer, or why there's none, from a search that went outside.
macro_rules! answered {
    ($which:literal, $call:expr) => {
        match $call.await {
            Ok(found) => found,
            Err(err) => return Ok(Err(outside($which, err))),
        }
    };
}

/// Things of `kind` called `q` (by `artist`, for an album or song).
///
/// # Errors
///
/// The inner [`Refused`] when `q` or `artist` aren't usable, the service for `kind` isn't set
/// up, or it didn't answer; the outer error when the database fails.
pub async fn search(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    user_id: i64,
    kind: &str,
    q: &str,
    artist: Option<&str>,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    let q = q.trim();
    if q.is_empty() || q.chars().count() > 200 {
        return Ok(Err(Refused::Invalid("q is 1 to 200 characters".into())));
    }
    let artist = artist.map(str::trim).filter(|a| !a.is_empty());
    if artist.is_some_and(|a| a.chars().count() > 200) {
        return Ok(Err(Refused::Invalid(
            "artist is at most 200 characters".into(),
        )));
    }
    if artist.is_some() && !matches!(kind, "album" | "song") {
        return Ok(Err(Refused::Invalid(
            "artist narrows an album or song search".into(),
        )));
    }
    let found: Vec<Candidate> = match find(tmdb, mb, kind, q, artist).await? {
        Ok(found) => found,
        Err(refused) => return Ok(Err(refused)),
    };
    let mut marked = Vec::with_capacity(found.len());
    for candidate in found {
        marked.push(mark(pool, user_id, candidate).await?);
    }
    Ok(Ok(marked))
}

/// Things of `kind` called `q`, as TMDB or MusicBrainz knows them.
async fn find(
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    kind: &str,
    q: &str,
    artist: Option<&str>,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    match kind {
        "movie" | "series" => {
            let Some(tmdb) = tmdb else {
                return Ok(Err(Refused::NoMetadata));
            };
            screen(tmdb, kind, q).await
        }
        "artist" | "album" | "song" => {
            let Some(mb) = mb else {
                return Ok(Err(Refused::NoMusicBrainz));
            };
            match kind {
                "artist" => artists(mb, q).await,
                "album" => albums(mb, q, artist).await,
                _ => songs(mb, q, artist).await,
            }
        }
        _ => Ok(Err(Refused::Invalid(
            "kind is movie, series, artist, album or song".into(),
        ))),
    }
}

/// An original title, when it isn't the title.
fn other(original: Option<String>, title: &str) -> Option<String> {
    original.filter(|o| o != title)
}

/// Movies or series called `q` on TMDB.
async fn screen(
    tmdb: &tmdb::Client,
    kind: &str,
    q: &str,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    Ok(Ok(if kind == "movie" {
        answered!("TMDB", tmdb.search_movies_page(q))
            .into_iter()
            .map(|r| Candidate {
                kind: "movie".into(),
                tmdb_id: Some(r.id),
                year: r.release_date.as_deref().and_then(year_of),
                detail: other(r.original_title, &r.title),
                title: r.title,
                ..Candidate::default()
            })
            .collect()
    } else {
        answered!("TMDB", tmdb.search_series_page(q))
            .into_iter()
            .map(|r| Candidate {
                kind: "series".into(),
                tmdb_id: Some(r.id),
                year: r.first_air_date.as_deref().and_then(year_of),
                detail: other(r.original_name, &r.name),
                title: r.name,
                ..Candidate::default()
            })
            .collect()
    }))
}

/// Artists called `q` on MusicBrainz.
async fn artists(
    mb: &musicbrainz::Client,
    q: &str,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    let found = answered!("MusicBrainz", mb.find_artists(q))
        .into_iter()
        .map(|(score, a)| {
            let about: Vec<String> = [a.disambiguation, a.kind, a.country]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect();
            let c = Candidate {
                kind: "artist".into(),
                mb_artist_id: Some(a.id),
                title: a.name,
                detail: (!about.is_empty()).then(|| about.join(", ")),
                ..Candidate::default()
            };
            (score, c)
        })
        .collect();
    Ok(Ok(ranked(q, found)))
}

/// Albums called `q` on MusicBrainz, by `artist` when given.
async fn albums(
    mb: &musicbrainz::Client,
    q: &str,
    artist: Option<&str>,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    let found = answered!("MusicBrainz", mb.find_albums(q, artist))
        .into_iter()
        .map(|(score, g)| {
            let by = first_artist(&g.artist_credit);
            let mut types: Vec<String> = g.secondary_types.unwrap_or_default();
            types.extend(g.primary_type);
            let c = Candidate {
                kind: "album".into(),
                mb_release_group_id: Some(g.id),
                mb_artist_id: by.as_ref().map(|(id, _)| id.clone()),
                artist: by.map(|(_, name)| name),
                year: g.first_release_date.as_deref().and_then(year_of),
                title: g.title,
                detail: (!types.is_empty()).then(|| types.join(" ")),
                ..Candidate::default()
            };
            (score, c)
        })
        .collect();
    Ok(Ok(ranked(q, found)))
}

/// Songs called `q` on MusicBrainz, by `artist` when given.
async fn songs(
    mb: &musicbrainz::Client,
    q: &str,
    artist: Option<&str>,
) -> anyhow::Result<Result<Vec<Candidate>, Refused>> {
    let found = answered!("MusicBrainz", mb.find_songs(q, artist))
        .into_iter()
        .map(|(score, r)| {
            let by = first_artist(&r.artist_credit);
            let album = r
                .releases
                .iter()
                .find(|rel| {
                    rel.release_group
                        .as_ref()
                        .and_then(|g| g.primary_type.as_deref())
                        == Some("Album")
                })
                .or(r.releases.first())
                .map(|rel| rel.title.clone());
            let length = r
                .length
                .map(|ms| (ms + 500) / 1000)
                .map(|s| format!("{}:{:02}", s / 60, s % 60));
            let about: Vec<String> = [album, length].into_iter().flatten().collect();
            let c = Candidate {
                kind: "song".into(),
                mb_recording_id: Some(r.id),
                mb_artist_id: by.as_ref().map(|(id, _)| id.clone()),
                artist: by.map(|(_, name)| name),
                title: r.title,
                detail: (!about.is_empty()).then(|| about.join(", ")),
                ..Candidate::default()
            };
            (score, c)
        })
        .collect();
    Ok(Ok(ranked(q, found)))
}

/// How closely a name matches what was typed, closest first: the same words, starting with
/// them, holding them, or not.
fn closeness(q: &str, name: &str) -> u8 {
    let (q, name) = (crate::search_keys::fold(q), crate::search_keys::fold(name));
    if q.is_empty() || name == q {
        0
    } else if name.starts_with(&q) {
        1
    } else if name.contains(&q) {
        2
    } else {
        3
    }
}

/// Best first: by MusicBrainz's score, then by how closely the name matches, then as
/// MusicBrainz listed them.
fn ranked(q: &str, mut found: Vec<(i64, Candidate)>) -> Vec<Candidate> {
    found.sort_by_key(|(score, c)| (std::cmp::Reverse(*score), closeness(q, &c.title)));
    found.into_iter().map(|(_, c)| c).collect()
}

/// Narrows a music candidate's `in_library` to the libraries `access` lets the user see: an
/// album, song or artist only in a music library hidden from them isn't in the library for
/// them, as the rest of the API answers. Movies and series the user may not see are left out
/// whole by the caller.
pub async fn as_seen_by(
    pool: &SqlitePool,
    access: &Access,
    mut c: Candidate,
) -> anyhow::Result<Candidate> {
    if !c.in_library || !access.limits_libraries() {
        return Ok(c);
    }
    let (sql, id) = match c.kind.as_str() {
        "album" => (
            format!(
                "SELECT EXISTS (SELECT 1 FROM music_albums a WHERE a.mb_release_group_id = ?
                     AND a.missing_since IS NULL AND {})",
                access.album_sql("a")
            ),
            &c.mb_release_group_id,
        ),
        "song" => (
            format!(
                "SELECT EXISTS (SELECT 1 FROM music_tracks t WHERE t.mb_recording_id = ?
                     AND t.missing_since IS NULL AND {})",
                access.track_sql("t")
            ),
            &c.mb_recording_id,
        ),
        // Seen as the music API sees an artist: with an album, or a track crediting them, the
        // user may see.
        "artist" => (
            format!(
                "SELECT EXISTS (SELECT 1 FROM music_artists ar WHERE ar.mb_artist_id = ?
                     AND (EXISTS (SELECT 1 FROM music_albums a WHERE a.artist_id = ar.id
                             AND a.missing_since IS NULL AND {})
                          OR EXISTS (SELECT 1 FROM music_track_artists ta
                             JOIN music_tracks t ON t.id = ta.track_id
                             WHERE ta.artist_id = ar.id AND t.missing_since IS NULL AND {})))",
                access.album_sql("a"),
                access.track_sql("t")
            ),
            &c.mb_artist_id,
        ),
        _ => return Ok(c),
    };
    c.in_library = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_one(pool)
        .await?;
    // An album's cover is shown only where the album is.
    if !c.in_library && c.kind == "album" {
        c.poster = None;
    }
    Ok(c)
}

/// Says whether it's in the library, wanted, or asked for already.
async fn mark(pool: &SqlitePool, user_id: i64, mut c: Candidate) -> anyhow::Result<Candidate> {
    let exists = |sql: &'static str, id: Option<String>| async move {
        sqlx::query_scalar::<_, bool>(sql)
            .bind(id)
            .fetch_one(pool)
            .await
    };
    match c.kind.as_str() {
        "movie" | "series" => {
            let table = if c.kind == "movie" {
                "movies"
            } else {
                "series"
            };
            let wanted = if c.kind == "movie" {
                "wanted_movies"
            } else {
                "wanted_series"
            };
            c.in_library = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT EXISTS (SELECT 1 FROM {table} WHERE missing_since IS NULL
                   AND (match_tmdb_id = ?1 OR arr_tmdb_id = ?1 OR tmdb_id = ?1))"
            )))
            .bind(c.tmdb_id)
            .fetch_one(pool)
            .await?;
            c.wanted = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT EXISTS (SELECT 1 FROM {wanted} WHERE tmdb_id = ? AND monitored = 1)"
            )))
            .bind(c.tmdb_id)
            .fetch_one(pool)
            .await?;
        }
        "artist" => {
            c.in_library = exists(
                "SELECT EXISTS (SELECT 1 FROM music_artists WHERE mb_artist_id = ?)",
                c.mb_artist_id.clone(),
            )
            .await?;
        }
        "album" => {
            c.in_library = exists(
                "SELECT EXISTS (SELECT 1 FROM music_albums
                                WHERE mb_release_group_id = ? AND missing_since IS NULL)",
                c.mb_release_group_id.clone(),
            )
            .await?;
        }
        _ => {
            c.in_library = exists(
                "SELECT EXISTS (SELECT 1 FROM music_tracks
                                WHERE mb_recording_id = ? AND missing_since IS NULL)",
                c.mb_recording_id.clone(),
            )
            .await?;
        }
    }
    if !matches!(c.kind.as_str(), "movie" | "series") {
        let asked = music_requests::Asked {
            kind: c.kind.clone(),
            title: c.title.clone(),
            artist: c.artist.clone(),
            mb_artist_id: c.mb_artist_id.clone(),
            mb_release_group_id: c.mb_release_group_id.clone(),
            mb_recording_id: c.mb_recording_id.clone(),
        };
        c.wanted = music_requests::wanted_already(pool, &asked)
            .await?
            .is_some();
    }
    c.poster = art::here(pool, &c).await?;
    c.requested = sqlx::query_scalar(
        "SELECT id FROM requests WHERE user_id = ? AND kind = ? AND state != 'declined'
           AND (tmdb_id = ? OR mb_recording_id = ? OR mb_release_group_id = ?
                OR (kind = 'artist' AND mb_artist_id = ?))
         ORDER BY id DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(&c.kind)
    .bind(c.tmdb_id)
    .bind(&c.mb_recording_id)
    .bind(&c.mb_release_group_id)
    .bind(&c.mb_artist_id)
    .fetch_optional(pool)
    .await?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::auth;
    use crate::db;
    use crate::musicbrainz::testing::{self, OK_COMPUTER, PARANOID_ANDROID, RADIOHEAD};
    use crate::tmdb::testing::{FakeTmdb, Reply};

    async fn rig() -> (SqlitePool, i64, tmdb::Client, musicbrainz::Client) {
        let pool = db::open_in_memory().await;
        let user = auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap()
            .id;
        let fake = FakeTmdb::default();
        fake.reply(
            "/search/movie?include_adult=false&query=Matrix",
            vec![Reply::ok(json!({"page": 1, "total_pages": 3, "results": [
                {"id": 603, "title": "The Matrix", "original_title": "The Matrix", "release_date": "1999-03-30"},
                {"id": 9999, "title": "Matrix", "original_title": "Matrix (Sweden)", "release_date": ""},
            ]}))],
        );
        let (mb, _) = testing::start(0).await;
        (pool, user, fake.start().await, mb)
    }

    #[tokio::test]
    async fn finds_titles_on_tmdb_marked_wanted_or_asked_for() {
        let (pool, user, tmdb, mb) = rig().await;
        sqlx::query(
            "INSERT INTO wanted_movies (tmdb_id, title, monitored, source, added_at)
             VALUES (603, 'The Matrix', 1, 'manual', 0);
             INSERT INTO requests (user_id, kind, tmdb_id, title, state, created_at)
             VALUES (?, 'movie', 603, 'The Matrix', 'requested', 0)",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        let found = search(
            &pool,
            Some(&tmdb),
            Some(&mb),
            user,
            "movie",
            " Matrix ",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        let summary: Vec<_> = found
            .iter()
            .map(|c| {
                (
                    c.tmdb_id,
                    c.title.as_str(),
                    c.year,
                    c.detail.as_deref(),
                    c.wanted,
                    c.requested.is_some(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (Some(603), "The Matrix", Some(1999), None, true, true),
                (
                    Some(9999),
                    "Matrix",
                    None,
                    Some("Matrix (Sweden)"),
                    false,
                    false
                ),
            ],
            "one page only, and the original title only when it differs"
        );
    }

    #[tokio::test]
    async fn finds_music_on_musicbrainz_marked_in_the_library() {
        let (pool, user, tmdb, mb) = rig().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music');
             INSERT INTO music_albums (library_id, key, title, artist_name, folder, seen_generation, mb_release_group_id)
             VALUES (4, 'dir:OK Computer', 'OK Computer', 'Radiohead', 'Radiohead/OK Computer', 1, ?)",
        )
        .bind(OK_COMPUTER)
        .execute(&pool)
        .await
        .unwrap();
        let artists = search(
            &pool,
            Some(&tmdb),
            Some(&mb),
            user,
            "artist",
            "Radiohead",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            artists
                .iter()
                .map(|a| (a.mb_artist_id.as_deref(), a.detail.as_deref()))
                .collect::<Vec<_>>(),
            [
                (Some(RADIOHEAD), Some("Group, GB")),
                (
                    Some("0a0b0c0d-1e2f-4a5b-8c6d-7e8f9a0b1c2d"),
                    Some("a tribute act, Person")
                ),
            ]
        );
        let albums = search(
            &pool,
            None,
            Some(&mb),
            user,
            "album",
            "OK Computer",
            Some("Radiohead"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (
                albums[0].title.as_str(),
                albums[0].artist.as_deref(),
                albums[0].year,
                albums[0].detail.as_deref(),
                albums[0].in_library,
            ),
            (
                "OK Computer",
                Some("Radiohead"),
                Some(1997),
                Some("Album"),
                true
            )
        );
        let songs = search(
            &pool,
            None,
            Some(&mb),
            user,
            "song",
            "Paranoid Android",
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (
                songs[0].mb_recording_id.as_deref(),
                songs[0].detail.as_deref(),
                songs[0].in_library
            ),
            (Some(PARANOID_ANDROID), Some("OK Computer, 6:23"), false)
        );
    }

    #[tokio::test]
    async fn refuses_what_it_cant_search_for() {
        let (pool, user, tmdb, mb) = rig().await;
        let refused = |kind: &'static str, q: &'static str, artist: Option<&'static str>| {
            let (pool, tmdb, mb) = (pool.clone(), &tmdb, &mb);
            async move {
                search(&pool, Some(tmdb), Some(mb), user, kind, q, artist)
                    .await
                    .unwrap()
                    .unwrap_err()
            }
        };
        assert_eq!(
            refused("movie", "  ", None).await,
            Refused::Invalid("q is 1 to 200 characters".into())
        );
        assert_eq!(
            refused("book", "x", None).await,
            Refused::Invalid("kind is movie, series, artist, album or song".into())
        );
        assert_eq!(
            refused("movie", "Matrix", Some("someone")).await,
            Refused::Invalid("artist narrows an album or song search".into())
        );
        // TMDB has no answer for this search: it failed, it didn't find nothing.
        assert_eq!(
            refused("movie", "Nothing", None).await,
            Refused::Outside("TMDB")
        );
        assert_eq!(
            search(&pool, None, None, user, "series", "x", None)
                .await
                .unwrap(),
            Err(Refused::NoMetadata)
        );
        assert_eq!(
            search(&pool, None, None, user, "song", "x", None)
                .await
                .unwrap(),
            Err(Refused::NoMusicBrainz)
        );
    }

    #[test]
    fn results_rank_by_score_then_by_how_closely_the_name_matches() {
        let named = |score: i64, title: &str| {
            (
                score,
                Candidate {
                    title: title.into(),
                    ..Candidate::default()
                },
            )
        };
        let found = ranked(
            "radiohead",
            vec![
                named(90, "Radiohead"),
                named(100, "The Radiohead Tribute"),
                named(100, "Radiohead Tribute"),
                named(100, "Radio-Head"),
                named(100, "Shadows"),
            ],
        );
        assert_eq!(
            found.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            [
                "Radio-Head",
                "Radiohead Tribute",
                "The Radiohead Tribute",
                "Shadows",
                "Radiohead"
            ]
        );
    }

    #[tokio::test]
    async fn music_is_found_by_its_words_loosely() {
        let pool = db::open_in_memory().await;
        let (mb, log) = testing::start(0).await;
        let found = search(&pool, None, Some(&mb), 1, "artist", "radiohed", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found[0].mb_artist_id.as_deref(), Some(RADIOHEAD));
        let songs = search(
            &pool,
            None,
            Some(&mb),
            1,
            "song",
            "paranoid andriod",
            Some("radiohead"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(songs[0].mb_recording_id.as_deref(), Some(PARANOID_ANDROID));
        let asked = log.lock().unwrap().join("\n");
        // Each word loosely, and only the words.
        assert!(asked.contains("artist%3A%28radiohed%7E%29"), "{asked}");
        assert!(
            asked.contains("recording%3A%28paranoid%7E+AND+andriod%7E%29"),
            "{asked}"
        );
    }

    #[tokio::test]
    async fn pictures_come_only_from_whats_here() {
        let (pool, user, tmdb, mb) = rig().await;
        sqlx::query(
            "INSERT INTO tmdb_movies (tmdb_id, title, poster_path, fetched_at)
                 VALUES (603, 'The Matrix', '/matrix.jpg', 0);
             INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                     seen_generation, mb_release_group_id)
                 VALUES (7, 4, 'k', 'OK Computer', 'Radiohead', 'Radiohead/OK Computer', 1, ?)",
        )
        .bind(OK_COMPUTER)
        .execute(&pool)
        .await
        .unwrap();
        let movies = search(&pool, Some(&tmdb), None, user, "movie", "Matrix", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            movies
                .iter()
                .map(|c| (c.tmdb_id, c.poster.as_deref()))
                .collect::<Vec<_>>(),
            [
                (Some(603), Some("/api/v1/artwork/w342/matrix.jpg")),
                (Some(9999), None)
            ]
        );
        let albums = search(&pool, None, Some(&mb), user, "album", "ok computer", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            albums[0].poster.as_deref(),
            Some("/api/v1/music/albums/7/cover")
        );
    }
}
