//! Requests: anyone signed in asks for a movie, a series or one of its seasons, an artist, an
//! album or a song. A request joins the wanted list once an admin approves it, or at once for
//! an admin and for users whose requests are let through; something already wanted is
//! approved at once and linked, so its requester can follow it. Approving puts the title where
//! the request defaults say (or where the approver chooses), with Melete's own quality profile.
//! Music is looked up on MusicBrainz and approved in `music_requests`.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::auth::User;
use crate::db::unix_now;
use crate::music_requests;
use crate::musicbrainz;
use crate::progress::{self, Progress, SeasonProgress};
use crate::tmdb;
use crate::want::{self, Placement, Seasons, Wanted};

mod approving;
mod changes;
mod choosing;
mod look_up;
mod view;

pub use approving::*;
pub use changes::*;
pub use choosing::*;
use look_up::*;
pub use view::*;

/// A request as asked for. Each kind is named by its own id; the title and year come from
/// TMDB or MusicBrainz, not from here.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewRequest {
    /// "movie", "series", "artist", "album" or "song".
    pub kind: String,
    /// A movie's or series' TMDB id.
    pub tmdb_id: Option<i64>,
    /// For a series: one season, or none for all of it.
    pub season: Option<i64>,
    /// An artist's MusicBrainz id.
    pub mb_artist_id: Option<String>,
    /// An album's MusicBrainz release group.
    pub mb_release_group_id: Option<String>,
    /// A song's MusicBrainz recording; without one, a song is asked for by title and artist.
    pub mb_recording_id: Option<String>,
    /// A song's title, when it's asked for by name.
    pub title: Option<String>,
    /// A song's artist, when it's asked for by name.
    pub artist: Option<String>,
    /// What an admin chooses instead of the defaults, as [`Choice`] has it; refused from
    /// anyone else.
    pub library_id: Option<i64>,
    /// See [`Choice::profile_id`].
    pub profile_id: Option<i64>,
    /// See [`Choice::music_profile_id`].
    pub music_profile_id: Option<i64>,
    /// See [`Choice::monitored`].
    pub monitored: Option<bool>,
    /// See [`Choice::search_now`].
    pub search_now: Option<bool>,
    /// See [`Choice::seasons`].
    pub seasons: Option<Vec<i64>>,
}

impl NewRequest {
    /// What it chooses instead of the defaults.
    pub fn choice(&self) -> Choice {
        Choice {
            library_id: self.library_id,
            profile_id: self.profile_id,
            music_profile_id: self.music_profile_id,
            monitored: self.monitored,
            search_now: self.search_now,
            seasons: self.seasons.clone(),
        }
    }
}

/// Why a request couldn't be made, found or decided on.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// Movies and series are looked up on TMDB, which isn't set up.
    NoMetadata,
    /// Music is looked up on MusicBrainz, which isn't set up.
    NoMusicBrainz,
    /// What was sent isn't usable: why.
    Invalid(String),
    /// There's no such request.
    NotFound,
    /// The same user already asked for it: that request's id.
    Already(i64),
    /// It isn't waiting for a decision any more.
    Decided,
    /// Nowhere is set for it to go.
    NoLibrary(String),
    /// It couldn't be put on the wanted list: why.
    Unavailable(String),
    /// Someone else's request.
    NotYours,
    /// TMDB or MusicBrainz didn't answer: which.
    Outside(&'static str),
    /// It can't be done as things stand: why.
    Conflict(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoMetadata => f.write_str(
                "movies and series are looked up on TMDB, which isn't set up (MELETE_TMDB_TOKEN)",
            ),
            Refused::NoMusicBrainz => f.write_str(
                "music is looked up on MusicBrainz, which isn't set up (MELETE_MUSICBRAINZ=on)",
            ),
            Refused::Invalid(why) => f.write_str(why),
            Refused::NotFound => f.write_str("no such request"),
            Refused::Already(id) => write!(f, "you've asked for it already (request {id})"),
            Refused::Decided => f.write_str("it isn't waiting for a decision any more"),
            Refused::NoLibrary(why) => f.write_str(why),
            Refused::Unavailable(why) => write!(f, "it can't be wanted: {why}"),
            Refused::NotYours => f.write_str("it's someone else's request"),
            Refused::Outside(which) => write!(f, "{which} didn't answer; try again in a moment"),
            Refused::Conflict(why) => f.write_str(why),
        }
    }
}

/// Records a request and, for an admin, a user whose requests are let through, or something
/// already wanted, approves it at once.
pub async fn create(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    user: &User,
    new: NewRequest,
) -> anyhow::Result<Result<RequestView, Refused>> {
    let choice = new.choice();
    if let Some(why) = refuse_choice(pool, user, &new.kind, &choice).await? {
        return Ok(Err(Refused::Invalid(why)));
    }
    let found = match new.kind.as_str() {
        "movie" | "series" => look_up_title(tmdb, &new).await?,
        "artist" | "album" | "song" => look_up_music(mb, &new).await?,
        _ => {
            return Ok(Err(Refused::Invalid(
                "kind is movie, series, artist, album or song".into(),
            )));
        }
    };
    let found = match found {
        Ok(found) => found,
        Err(refused) => return Ok(Err(refused)),
    };
    let (id, reopened) = match record(pool, user, &new, &found).await? {
        Ok(recorded) => recorded,
        Err(refused) => return Ok(Err(refused)),
    };
    let wanted_already = wanted_already(pool, &new, &found).await?;
    let let_through: bool = user.is_admin
        || sqlx::query_scalar("SELECT requests_approved = 1 FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_one(pool)
            .await?;
    if wanted_already || let_through {
        // Approved by the requester's standing, or because it's on its way already.
        let by = (!wanted_already).then_some(user.id);
        if let Err(refused) = approve_at_once(pool, tmdb, mb, id, by, &choice).await? {
            restore_or_drop(pool, id, reopened).await?;
            return Ok(Err(refused));
        }
    }
    Ok(Ok(get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {id} vanished"))?))
}

/// Approves a request as it's made. One that can't be approved stays asked for, for an admin
/// to choose where it goes; but when an admin's own choice is what doesn't work, they're told
/// and nothing is kept ([`restore_or_drop`]).
async fn approve_at_once(
    pool: &SqlitePool,
    tmdb: Option<&tmdb::Client>,
    mb: Option<&musicbrainz::Client>,
    id: i64,
    by: Option<i64>,
    choice: &Choice,
) -> anyhow::Result<Result<(), Refused>> {
    match approve_as(pool, tmdb, mb, id, by, choice).await? {
        Err(refused @ (Refused::Invalid(_) | Refused::NoLibrary(_))) if choice.chooses() => {
            Ok(Err(refused))
        }
        Err(refused) => {
            tracing::info!(request = id, why = %refused, "a request couldn't be approved at once");
            Ok(Ok(()))
        }
        Ok(()) => Ok(Ok(())),
    }
}

/// Records what's asked for: a new request, or the user's approved one again when what it
/// became was taken off the wanted list (with how it stood); refused when the user's request
/// for it is still open.
async fn record(
    pool: &SqlitePool,
    user: &User,
    new: &NewRequest,
    found: &Found,
) -> anyhow::Result<Result<(i64, Option<Was>), Refused>> {
    if let Some(id) = insert_request(pool, user, new, found).await? {
        return Ok(Ok((id, None)));
    }
    let open = open_request(pool, user, new, found).await?;
    Ok(match reopen_unfollowed(pool, open).await? {
        Some(was) => Ok((open, Some(was))),
        None => Err(Refused::Already(open)),
    })
}

/// How an approved request stood before it was asked for again: when it was decided, by whom,
/// and when it was first asked for.
struct Was {
    decided_at: Option<i64>,
    decided_by: Option<i64>,
    created_at: i64,
}

/// Asks again for what an approved request became once that's off the wanted list (taken off,
/// so the request no longer leads anywhere): the request goes back to waiting, to be approved
/// as a new one would be. How it stood, or None when it's still open and followed.
async fn reopen_unfollowed(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<Was>> {
    const UNFOLLOWED: &str = "id = ? AND state = 'approved'
        AND coalesce(wanted_movie_id, wanted_series_id, wanted_artist_id, wanted_album_id,
                     wanted_song_id) IS NULL";
    let mut tx = pool.begin().await?;
    let was: Option<(Option<i64>, Option<i64>, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(
        format!("SELECT decided_at, decided_by, created_at FROM requests WHERE {UNFOLLOWED}"),
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((decided_at, decided_by, created_at)) = was else {
        return Ok(None);
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE requests SET state = 'requested', decided_at = NULL, decided_by = NULL,
             created_at = ?
         WHERE {UNFOLLOWED}"
    )))
    .bind(unix_now())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(Was {
        decided_at,
        decided_by,
        created_at,
    }))
}

/// Undoes a request an admin's own choice couldn't place: a new one is dropped, one asked for
/// again goes back to how it stood.
async fn restore_or_drop(pool: &SqlitePool, id: i64, was: Option<Was>) -> anyhow::Result<()> {
    let Some(was) = was else {
        sqlx::query("DELETE FROM requests WHERE id = ? AND state = 'requested'")
            .bind(id)
            .execute(pool)
            .await?;
        return Ok(());
    };
    sqlx::query(
        "UPDATE requests SET state = 'approved', decided_at = ?, decided_by = ?, created_at = ?
         WHERE id = ? AND state = 'requested'",
    )
    .bind(was.decided_at)
    .bind(was.decided_by)
    .bind(was.created_at)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Records the request as asked for; None when the user has an open request for it already.
async fn insert_request(
    pool: &SqlitePool,
    user: &User,
    new: &NewRequest,
    found: &Found,
) -> anyhow::Result<Option<i64>> {
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO requests (user_id, kind, tmdb_id, season, mb_artist_id, mb_release_group_id,
             mb_recording_id, title, year, artist, poster, anime, state, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'requested', ?)
         ON CONFLICT DO NOTHING RETURNING id",
    )
    .bind(user.id)
    .bind(&new.kind)
    .bind(found.tmdb_id)
    .bind(new.season)
    .bind(&found.mb_artist_id)
    .bind(&found.mb_release_group_id)
    .bind(&found.mb_recording_id)
    .bind(&found.title)
    .bind(found.year)
    .bind(&found.artist)
    .bind(&found.poster)
    .bind(found.anime)
    .bind(unix_now())
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// The user's open request for the same thing.
async fn open_request(
    pool: &SqlitePool,
    user: &User,
    new: &NewRequest,
    found: &Found,
) -> anyhow::Result<i64> {
    // The one open request for the same thing, as the requests_once index tells them apart.
    let open: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM requests WHERE user_id = ? AND kind = ? AND state != 'declined'
               AND coalesce(tmdb_id, -1) = coalesce(?, -1) AND coalesce(season, -1) = coalesce(?, -1)
               AND coalesce(mb_recording_id, mb_release_group_id, mb_artist_id,
                            lower(title || '␟' || coalesce(artist, '')))
                 = coalesce(?, ?, ?, lower(? || '␟' || coalesce(?, '')))",
    )
    .bind(user.id)
    .bind(&new.kind)
    .bind(found.tmdb_id)
    .bind(new.season)
    .bind(&found.mb_recording_id)
    .bind(&found.mb_release_group_id)
    .bind(&found.mb_artist_id)
    .bind(&found.title)
    .bind(&found.artist)
    .fetch_optional(pool)
    .await?;
    Ok(open.unwrap_or_default())
}

/// Whether what's asked for is on the wanted list already.
async fn wanted_already(
    pool: &SqlitePool,
    new: &NewRequest,
    found: &Found,
) -> anyhow::Result<bool> {
    Ok(match (new.kind.as_str(), found.tmdb_id) {
        ("movie", Some(tmdb_id)) => {
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wanted_movies WHERE tmdb_id = ?)")
                .bind(tmdb_id)
                .fetch_one(pool)
                .await?
        }
        ("series", Some(tmdb_id)) => {
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM wanted_series WHERE tmdb_id = ?)")
                .bind(tmdb_id)
                .fetch_one(pool)
                .await?
        }
        _ => {
            let asked = music_requests::Asked {
                kind: new.kind.clone(),
                title: found.title.clone(),
                artist: found.artist.clone(),
                mb_artist_id: found.mb_artist_id.clone(),
                mb_release_group_id: found.mb_release_group_id.clone(),
                mb_recording_id: found.mb_recording_id.clone(),
            };
            music_requests::wanted_already(pool, &asked)
                .await?
                .is_some()
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::auth;
    use crate::db;
    use crate::tmdb::testing::{FakeTmdb, Reply};

    struct Rig {
        pool: SqlitePool,
        tmdb: tmdb::Client,
        admin: User,
        alice: User,
        bob: User,
    }

    fn series(id: i64, name: &str, genres: &[&str], language: &str) -> Reply {
        Reply::ok(json!({
            "id": id, "name": name, "first_air_date": "2011-04-17", "original_language": language,
            "poster_path": "/thrones.jpg",
            "genres": genres.iter().map(|g| json!({"id": 1, "name": g})).collect::<Vec<_>>(),
            "seasons": [{"season_number": 0}, {"season_number": 1}, {"season_number": 2}, {"season_number": 3}],
            "external_ids": {"tvdb_id": id + 100000, "imdb_id": "tt0944947"},
        }))
    }

    async fn rig() -> Rig {
        let pool = db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES
                 (1, 'movies', 'movies', '/m'), (2, 'shows', 'shows', '/s'), (3, 'anime', 'shows', '/a')",
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
        let fake = FakeTmdb::default();
        fake.reply(
            "/movie/603",
            vec![Reply::ok(json!({"id": 603, "title": "The Matrix", "release_date": "1999-03-30", "poster_path": "/matrix.jpg"}))],
        )
        .reply(
            "/movie/604",
            vec![Reply::ok(json!({"id": 604, "title": "The Matrix Reloaded", "release_date": "2003-05-15"}))],
        )
        .reply("/tv/1399?append_to_response=external_ids", vec![series(1399, "Game of Thrones", &["Drama"], "en")])
        .reply(
            "/tv/31910?append_to_response=external_ids",
            vec![series(31910, "Naruto Shippuden", &["Animation", "Action"], "ja")],
        );
        let tmdb = fake.start().await;
        Rig {
            pool,
            tmdb,
            admin,
            alice,
            bob,
        }
    }

    fn movie(id: i64) -> NewRequest {
        NewRequest {
            kind: "movie".into(),
            tmdb_id: Some(id),
            season: None,
            ..NewRequest::default()
        }
    }

    fn show(id: i64, season: Option<i64>) -> NewRequest {
        NewRequest {
            kind: "series".into(),
            tmdb_id: Some(id),
            season,
            ..NewRequest::default()
        }
    }

    async fn wanted_movies(pool: &SqlitePool) -> Vec<(i64, String, i64, bool, Option<i64>)> {
        sqlx::query_as("SELECT tmdb_id, title, library_id, monitored, profile_id FROM wanted_movies ORDER BY tmdb_id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_request_waits_for_an_admin_then_joins_the_wanted_list() {
        let r = rig().await;
        let asked = create(&r.pool, Some(&r.tmdb), None, &r.alice, movie(603))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                asked.state.as_str(),
                asked.title.as_str(),
                asked.year,
                asked.poster.as_deref(),
                asked.username.as_str()
            ),
            (
                "requested",
                "The Matrix",
                Some(1999),
                Some("/api/v1/artwork/w342/matrix.jpg"),
                "alice"
            )
        );
        assert_eq!(asked.progress, None);
        assert!(
            wanted_movies(&r.pool).await.is_empty(),
            "not wanted until approved"
        );
        assert_eq!(
            create(&r.pool, Some(&r.tmdb), None, &r.alice, movie(603))
                .await
                .unwrap(),
            Err(Refused::Already(asked.id))
        );

        let approved = approve(
            &r.pool,
            Some(&r.tmdb),
            None,
            asked.id,
            &r.admin,
            &Choice::default(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (approved.state.as_str(), approved.decided_by.as_deref()),
            ("approved", Some("admin"))
        );
        assert_eq!(
            wanted_movies(&r.pool).await,
            [(603, "The Matrix".into(), 1, true, None)]
        );
        let progress = approved.progress.unwrap();
        assert_eq!(
            (progress.step, progress.message.as_deref()),
            (progress::Step::Searching, Some("looking out for a release"))
        );
        assert_eq!(
            approve(
                &r.pool,
                Some(&r.tmdb),
                None,
                asked.id,
                &r.admin,
                &Choice::default()
            )
            .await
            .unwrap(),
            Err(Refused::Decided)
        );

        // Someone else asking for what's wanted already is approved at once, by no one.
        let again = create(&r.pool, Some(&r.tmdb), None, &r.bob, movie(603))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((again.state.as_str(), again.decided_by), ("approved", None));
        assert_eq!(again.wanted_id, approved.wanted_id);
    }

    #[tokio::test]
    async fn a_title_taken_off_the_list_can_be_asked_for_again() {
        use crate::wanted_edit::{self, Kind};
        let r = rig().await;
        let own = create(&r.pool, Some(&r.tmdb), None, &r.admin, movie(603))
            .await
            .unwrap()
            .unwrap();
        let asked = create(&r.pool, Some(&r.tmdb), None, &r.alice, movie(604))
            .await
            .unwrap()
            .unwrap();
        let approved = approve(
            &r.pool,
            Some(&r.tmdb),
            None,
            asked.id,
            &r.admin,
            &Choice::default(),
        )
        .await
        .unwrap()
        .unwrap();
        for id in [own.wanted_id, approved.wanted_id] {
            wanted_edit::remove(&r.pool, Kind::Movie, id.unwrap())
                .await
                .unwrap()
                .unwrap();
        }
        assert!(wanted_movies(&r.pool).await.is_empty());

        // An admin's request is approved again at once, and leads to the title again.
        let again = create(&r.pool, Some(&r.tmdb), None, &r.admin, movie(603))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (again.id, again.state.as_str(), again.decided_by.as_deref()),
            (own.id, "approved", Some("admin"))
        );
        assert!(again.wanted_id.is_some());
        assert_eq!(
            wanted_movies(&r.pool).await,
            [(603, "The Matrix".into(), 1, true, None)]
        );
        // Followed again, so asking once more is refused as before.
        assert_eq!(
            create(&r.pool, Some(&r.tmdb), None, &r.admin, movie(603))
                .await
                .unwrap(),
            Err(Refused::Already(own.id))
        );

        // A user's waits for an admin again, as a new request would.
        let again = create(&r.pool, Some(&r.tmdb), None, &r.alice, movie(604))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (again.id, again.state.as_str(), again.decided_by),
            (asked.id, "requested", None)
        );
        assert_eq!(wanted_movies(&r.pool).await.len(), 1);
    }

    #[tokio::test]
    async fn asked_again_with_a_choice_that_fails_it_stays_as_it_was() {
        use crate::wanted_edit::{self, Kind};
        let r = rig().await;
        let own = create(
            &r.pool,
            Some(&r.tmdb),
            None,
            &r.admin,
            NewRequest {
                library_id: Some(2),
                ..show(1399, None)
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(own.state, "approved");
        let decided: (Option<i64>, i64) =
            sqlx::query_as("SELECT decided_at, created_at FROM requests WHERE id = ?")
                .bind(own.id)
                .fetch_one(&r.pool)
                .await
                .unwrap();
        wanted_edit::remove(&r.pool, Kind::Series, own.wanted_id.unwrap())
            .await
            .unwrap()
            .unwrap();
        // Two shows libraries and none chosen: the admin is told, and the request stays.
        let refused = create(
            &r.pool,
            Some(&r.tmdb),
            None,
            &r.admin,
            NewRequest {
                monitored: Some(false),
                ..show(1399, None)
            },
        )
        .await
        .unwrap();
        assert!(matches!(refused, Err(Refused::NoLibrary(_))), "{refused:?}");
        let kept = get(&r.pool, own.id).await.unwrap().unwrap();
        assert_eq!(
            (kept.state.as_str(), kept.decided_by.as_deref()),
            ("approved", Some("admin"))
        );
        let after: (Option<i64>, i64) =
            sqlx::query_as("SELECT decided_at, created_at FROM requests WHERE id = ?")
                .bind(own.id)
                .fetch_one(&r.pool)
                .await
                .unwrap();
        assert_eq!(after, decided);
    }

    #[tokio::test]
    async fn admins_and_users_let_through_skip_the_wait() {
        let r = rig().await;
        let own = create(&r.pool, Some(&r.tmdb), None, &r.admin, movie(604))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(own.state, "approved");
        // Two shows libraries and nothing set: it waits for an admin to choose.
        let asked = create(&r.pool, Some(&r.tmdb), None, &r.bob, show(1399, Some(2)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(asked.state, "requested");
        assert_eq!(
            approve(
                &r.pool,
                Some(&r.tmdb),
                None,
                asked.id,
                &r.admin,
                &Choice::default()
            )
            .await
            .unwrap(),
            Err(Refused::NoLibrary(
                "there's more than one shows library; choose one, or set where approved series go"
                    .into()
            ))
        );
        let chosen = Choice {
            library_id: Some(2),
            profile_id: None,
            music_profile_id: None,
            ..Choice::default()
        };
        let approved = approve(&r.pool, Some(&r.tmdb), None, asked.id, &r.admin, &chosen)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(approved.state, "approved");
        let seasons: Vec<(i64, bool)> =
            sqlx::query_as("SELECT season, monitored FROM wanted_seasons ORDER BY season")
                .fetch_all(&r.pool)
                .await
                .unwrap();
        assert_eq!(
            seasons,
            [(0, false), (1, false), (2, true), (3, false)],
            "only the season asked for"
        );

        // Another season of the same series: wanted already, so approved at once, and monitored.
        create(&r.pool, Some(&r.tmdb), None, &r.alice, show(1399, Some(3)))
            .await
            .unwrap()
            .unwrap();
        let monitored: bool =
            sqlx::query_scalar("SELECT monitored FROM wanted_seasons WHERE season = 3")
                .fetch_one(&r.pool)
                .await
                .unwrap();
        assert!(monitored);
    }

    #[tokio::test]
    async fn approved_titles_go_where_the_defaults_say_and_anime_where_its_own_do() {
        let r = rig().await;
        sqlx::query(
            "INSERT INTO request_defaults (kind, library_id) VALUES ('series', 2), ('anime', 3)",
        )
        .execute(&r.pool)
        .await
        .unwrap();
        create(&r.pool, Some(&r.tmdb), None, &r.bob, show(1399, None))
            .await
            .unwrap()
            .unwrap();
        create(&r.pool, Some(&r.tmdb), None, &r.bob, show(31910, None))
            .await
            .unwrap()
            .unwrap();
        let placed: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT title, library_id, series_type FROM wanted_series ORDER BY title",
        )
        .fetch_all(&r.pool)
        .await
        .unwrap();
        assert_eq!(
            placed,
            [
                ("Game of Thrones".into(), 2, "standard".into()),
                ("Naruto Shippuden".into(), 3, "anime".into())
            ]
        );
        let monitored: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT s.season, s.monitored FROM wanted_seasons s JOIN wanted_series w ON w.id = s.wanted_series_id
             WHERE w.title = 'Game of Thrones' ORDER BY season",
        )
        .fetch_all(&r.pool)
        .await
        .unwrap();
        assert_eq!(
            monitored,
            [(0, false), (1, true), (2, true), (3, true)],
            "all of it, but specials"
        );
    }

    #[tokio::test]
    async fn with_nothing_set_a_library_category_says_where_anime_goes() {
        let r = rig().await;
        sqlx::query("UPDATE libraries SET category = 'anime' WHERE id = 3")
            .execute(&r.pool)
            .await
            .unwrap();
        create(&r.pool, Some(&r.tmdb), None, &r.bob, show(1399, None))
            .await
            .unwrap()
            .unwrap();
        create(&r.pool, Some(&r.tmdb), None, &r.bob, show(31910, None))
            .await
            .unwrap()
            .unwrap();
        let placed: Vec<(String, i64)> =
            sqlx::query_as("SELECT title, library_id FROM wanted_series ORDER BY title")
                .fetch_all(&r.pool)
                .await
                .unwrap();
        assert_eq!(
            placed,
            [
                ("Game of Thrones".into(), 2),
                ("Naruto Shippuden".into(), 3)
            ]
        );
    }

    #[tokio::test]
    async fn declined_withdrawn_and_refused() {
        let r = rig().await;
        let asked = create(&r.pool, Some(&r.tmdb), None, &r.alice, movie(603))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            decline(&r.pool, asked.id, &r.admin, "  ").await.unwrap(),
            Err(Refused::Invalid("say why, in 1 to 500 characters".into()))
        );
        let declined = decline(&r.pool, asked.id, &r.admin, "Not in this house.")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (declined.state.as_str(), declined.reason.as_deref()),
            ("declined", Some("Not in this house."))
        );
        // Asking again after a decline is a new request.
        let again = create(&r.pool, Some(&r.tmdb), None, &r.alice, movie(603))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(again.id, asked.id);
        assert_eq!(
            withdraw(&r.pool, again.id, &r.bob).await.unwrap(),
            Err(Refused::NotYours)
        );
        assert_eq!(withdraw(&r.pool, again.id, &r.alice).await.unwrap(), Ok(()));
        assert_eq!(get(&r.pool, again.id).await.unwrap(), None);
        assert_eq!(
            withdraw(&r.pool, asked.id, &r.alice).await.unwrap(),
            Err(Refused::Decided)
        );

        assert_eq!(
            create(&r.pool, None, None, &r.alice, movie(603))
                .await
                .unwrap(),
            Err(Refused::NoMetadata)
        );
        for (new, why) in [
            (
                NewRequest {
                    kind: "book".into(),
                    tmdb_id: Some(1),
                    season: None,
                    ..NewRequest::default()
                },
                "kind is movie, series, artist, album or song",
            ),
            (
                NewRequest {
                    kind: "movie".into(),
                    tmdb_id: None,
                    season: None,
                    ..NewRequest::default()
                },
                "tmdb_id is needed for a movie or series",
            ),
            (
                NewRequest {
                    kind: "movie".into(),
                    tmdb_id: Some(603),
                    season: Some(1),
                    ..NewRequest::default()
                },
                "a season is only for a series",
            ),
        ] {
            assert_eq!(
                create(&r.pool, Some(&r.tmdb), None, &r.alice, new)
                    .await
                    .unwrap(),
                Err(Refused::Invalid(why.into()))
            );
        }
        let (total, mine) = list(&r.pool, Some(r.alice.id), None, 50, 0).await.unwrap();
        assert_eq!(
            (total, mine.len(), mine[0].state.as_str()),
            (1, 1, "declined")
        );
        assert_eq!(
            list(&r.pool, Some(r.bob.id), None, 50, 0).await.unwrap().0,
            0
        );
    }

    /// Series asked for before requests kept whether they're anime count as anime when the
    /// wanted list or a folder in an anime library says so.
    #[tokio::test]
    async fn series_asked_for_earlier_are_anime_when_the_library_says_so() {
        use sqlx::migrate::Migrator;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(sqlx::sqlite::SqliteConnectOptions::new().in_memory(true))
            .await
            .unwrap();
        let all = sqlx::migrate!();
        let before = Migrator {
            migrations: all
                .migrations
                .iter()
                .take_while(|m| m.description != "request anime")
                .cloned()
                .collect(),
            ..Migrator::DEFAULT
        };
        assert!(before.migrations.len() < all.migrations.len());
        before.run(&pool).await.unwrap();
        sqlx::raw_sql(
            "INSERT INTO users (id, username, password_hash, is_admin, created_at)
                 VALUES (1, 'alice', 'x', 0, 0);
             INSERT INTO libraries (id, name, kind, path, category)
                 VALUES (1, 'Shows', 'shows', '/s', NULL), (2, 'Anime', 'shows', '/a', 'anime');
             INSERT INTO series (id, library_id, rel_path, parsed_title, tmdb_id, seen_generation)
                 VALUES (1, 2, 'Mushishi', 'Mushishi', 3, 0);
             INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type,
                 library_id, source, added_at)
                 VALUES (1, 12, 2, 'Frieren', 1, 'anime', 1, 'manual', 0),
                        (2, 14, 4, 'Severance', 1, 'standard', 1, 'manual', 0);
             INSERT INTO requests (user_id, kind, tmdb_id, title, state, created_at) VALUES
                 (1, 'series', 2, 'Frieren', 'requested', 0),
                 (1, 'series', 3, 'Mushishi', 'requested', 0),
                 (1, 'series', 4, 'Severance', 'requested', 0),
                 (1, 'series', 5, 'Unknown', 'requested', 0),
                 (1, 'movie', 2, 'A film', 'requested', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        all.run(&pool).await.unwrap();
        let anime: Vec<(String, Option<i64>)> =
            sqlx::query_as("SELECT title, anime FROM requests ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            anime,
            [
                ("Frieren".into(), Some(1)),
                ("Mushishi".into(), Some(1)),
                ("Severance".into(), None),
                ("Unknown".into(), None),
                ("A film".into(), None),
            ]
        );
    }
}
