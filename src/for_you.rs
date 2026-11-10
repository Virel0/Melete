//! For You, while an admin has it on (`MELETE_LISTENBRAINZ`, off by default, with MusicBrainz):
//! songs for each listener from their own play history, some in the library to play and some
//! not, to request, each with the song or artist of theirs it came from.
//!
//! Its seeds are the recordings and artists a listener finished most in the last 90 days, of
//! the tracks they may see, leaving out any they flagged not interested. ListenBrainz
//! ([`crate::listenbrainz`]) is asked, by MusicBrainz id alone, for the recordings listened to
//! alongside each seed recording and the artists alongside each seed artist, then for the
//! artists and album of the recordings it suggests. A suggestion the library has (a present
//! track the listener may see with its recording id, or with one of its artists' ids and the
//! same title) is one to play, unless they heard it in the last 30 days, finished it three times
//! or flagged it; the rest are to request. Tracks by the artists alongside theirs are ones to
//! play too. Nothing else leaves the server: never a name, an account, play counts or titles.
//!
//! Lists are kept and served at once, and made in the background: for a listener with recent
//! plays the first time, then again once a day has passed and what they played has changed, a
//! few listeners a run. ListenBrainz's answers are kept a week. Turned off, every list and
//! answer is forgotten.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::access::Access;
use crate::listenbrainz;

mod asking;
mod picking;
mod seeds;
mod stored;

use stored::*;

/// The most songs on a list; [`picking`] gives each kind, to play and to request, half first.
pub const ITEMS: usize = 40;
/// How long a list stands, however much its listener plays meanwhile.
const REMAKE_AFTER: i64 = 24 * 3600;
/// Making a list starts at most this often for each listener, so a failing one isn't retried
/// on every run.
const TRY_EVERY: i64 = 3600;
/// The most lists one background run makes: each is up to some 30 requests at one a second.
pub const PER_RUN: usize = 5;
/// How often the background run looks for lists to make.
const EVERY: Duration = Duration::from_secs(3600);
/// How far back plays count: seeds come from finished plays this recent.
const SEED_WINDOW_MS: i64 = crate::aoide::taste::WINDOW_MS;
/// A track heard this recently isn't one to play: For You is discovery.
const HEARD_LATELY_MS: i64 = 30 * 86_400_000;
/// Nor one finished this often, ever.
const PLAYED_OFTEN: i64 = 3;

/// What a song on the list came from, for "Because you listened to …".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Because {
    /// "song" for a seed recording, "artist" for a seed artist.
    pub kind: String,
    /// The seed song's title; none for an artist.
    pub title: Option<String>,
    /// The seed song's artist, or the seed artist's name.
    pub artist: String,
}

/// A song not in the library, with what an app shows of it and asks for it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Song {
    /// Its MusicBrainz recording, which a request for it is made by.
    pub mb_recording_id: String,
    /// Its title, as ListenBrainz gives it.
    pub title: String,
    /// Its artists, as one line.
    pub artist: String,
    /// Its credited artists' MusicBrainz ids, in order; empty when ListenBrainz doesn't say.
    pub mb_artist_ids: Vec<String>,
    /// The album it's listed from, by name.
    pub release: Option<String>,
    /// That album's release group, whose cover it's shown with.
    pub release_group: Option<String>,
}

/// One song on a list: in the library (by track id) or to request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// The library's track, for a song to play.
    pub track_id: Option<i64>,
    /// The song, for one to request.
    pub song: Option<Song>,
    /// The seed it came from.
    pub because: Because,
}

/// Where a listener's list stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// An admin hasn't turned For You on, or MusicBrainz is off.
    Off,
    /// Nothing finished in the last 90 days to go on.
    NoHistory,
    /// Not made yet: it's made in the background.
    Waiting,
    /// Made; these are its songs.
    Ready,
}

/// A listener's list as served.
#[derive(Debug, Clone, PartialEq)]
pub struct Served {
    /// Where it stands.
    pub state: State,
    /// When it was made, in seconds since 1970.
    pub made_at: Option<i64>,
    /// Its songs still to offer, best first: those to play still there for the listener, not
    /// heard lately nor flagged since; those to request still not in the library for them.
    pub items: Vec<Item>,
}

/// SQL for a listener's plays as a list's `plays` notes them, for the user whose id is `user`
/// (an SQL expression): how many, and the last one's id.
fn plays_key(user: &str) -> String {
    format!(
        "(SELECT count(*) || ':' || coalesce(max(id), 0) FROM music_plays WHERE user_id = {user})"
    )
}

/// `user`'s plays as a list notes them ([`plays_key`]).
async fn plays_now(pool: &SqlitePool, user: i64) -> sqlx::Result<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {}", plays_key("?"))))
        .bind(user)
        .fetch_one(pool)
        .await
}

/// A track id as the flags name it, when it's one of the library's.
fn track_ids(flags: HashSet<String>) -> HashSet<i64> {
    flags.iter().filter_map(|f| f.parse().ok()).collect()
}

/// `user`'s list as it stands, for them alone; asks the background run for one when it's due.
///
/// # Errors
///
/// Fails when the database can't be read.
pub async fn for_listener(
    pool: &SqlitePool,
    user: i64,
    access: &Access,
    now: i64,
) -> anyhow::Result<Served> {
    let none = |state| Served {
        state,
        made_at: None,
        items: Vec::new(),
    };
    if !crate::settings::listenbrainz(pool).await? {
        return Ok(none(State::Off));
    }
    if !seeds::listened(pool, user, now).await? {
        return Ok(none(State::NoHistory));
    }
    let kept = kept(pool, user).await?;
    let plays = plays_now(pool, user).await?;
    let due = kept.as_ref().is_none_or(|k| {
        k.tried_at <= now - TRY_EVERY
            && k.made_at
                .is_none_or(|at| at <= now - REMAKE_AFTER && k.plays != plays)
    });
    if due {
        crate::jobs::FOR_YOU.notify_one();
    }
    let Some(Kept {
        made_at: Some(made_at),
        items,
        ..
    }) = kept
    else {
        return Ok(none(State::Waiting));
    };
    Ok(Served {
        state: State::Ready,
        made_at: Some(made_at),
        items: picking::still_there(pool, user, access, items, now).await?,
    })
}

/// What a background run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Made {
    /// Lists made.
    pub made: usize,
    /// Lists that couldn't be made, kept as they were to be tried again later.
    pub failed: usize,
}

/// Makes, with For You on, the lists due: of listeners with finished plays in the last 90 days,
/// those never made, then those a day old whose plays changed, at most [`PER_RUN`], none tried
/// in the last hour. Forgets ListenBrainz's answers past their week first.
///
/// # Errors
///
/// Fails when the database can't be read or written; a list that couldn't be made is counted.
pub async fn make_due(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    now: i64,
) -> anyhow::Result<Made> {
    if !crate::settings::listenbrainz(pool).await? {
        return Ok(Made::default());
    }
    crate::plays::catch_up(pool).await?;
    asking::forget_old(pool, now).await?;
    let due: Vec<(i64, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.id, u.is_admin FROM users u LEFT JOIN for_you f ON f.user_id = u.id
         WHERE EXISTS (SELECT 1 FROM music_plays p WHERE p.user_id = u.id AND p.completed = 1
                 AND p.started_at >= ?1)
           AND (f.user_id IS NULL OR (f.tried_at <= ?2 AND (f.made_at IS NULL
                 OR (f.made_at <= ?3 AND f.plays != {}))))
         ORDER BY f.made_at IS NOT NULL, f.made_at, u.id LIMIT ?4",
        plays_key("u.id")
    )))
    .bind(now * 1000 - SEED_WINDOW_MS)
    .bind(now - TRY_EVERY)
    .bind(now - REMAKE_AFTER)
    .bind(i64::try_from(PER_RUN).unwrap_or(1))
    .fetch_all(pool)
    .await?;
    let mut made = Made::default();
    for (user, is_admin) in due {
        // ListenBrainz asked for a long pause: the next run carries on.
        if client.held() {
            break;
        }
        let access = Access::of(pool, user, is_admin).await?;
        match make_for(pool, client, user, &access, now).await {
            Ok(()) => made.made += 1,
            Err(err) => {
                made.failed += 1;
                tracing::warn!(
                    error = format!("{err:#}"),
                    "a For You list couldn't be made"
                );
            }
        }
    }
    Ok(made)
}

/// Makes `user`'s list now and keeps it; on failure keeps why, and the list from before.
///
/// # Errors
///
/// Fails when ListenBrainz doesn't answer, or the database can't be read or written.
pub async fn make_for(
    pool: &SqlitePool,
    client: &listenbrainz::Client,
    user: i64,
    access: &Access,
    now: i64,
) -> anyhow::Result<()> {
    let plays = plays_now(pool, user).await?;
    claim(pool, user, &plays, now).await?;
    match picking::pick(pool, client, user, access, now).await {
        Ok(items) => keep(pool, user, &plays, &items, now).await,
        Err(err) => {
            fail(pool, user, &format!("{err:#}")).await?;
            Err(err)
        }
    }
}

/// Makes the lists due once an hour and when someone's is due, through one ListenBrainz client,
/// so together they keep to its pace.
pub async fn run(pool: SqlitePool, client: Arc<listenbrainz::Client>) {
    crate::jobs::repeat_task("for-you", EVERY, &crate::jobs::FOR_YOU, || async {
        match make_due(&pool, &client, crate::db::unix_now()).await {
            Ok(made) if made == Made::default() => {}
            Ok(made) => {
                tracing::info!(made = made.made, failed = made.failed, "made For You lists")
            }
            Err(err) => tracing::error!(error = format!("{err:#}"), "making For You lists stopped"),
        }
    })
    .await;
}

/// Forgets every list and every answer kept from ListenBrainz: what For You being off means.
/// How many were forgotten.
///
/// # Errors
///
/// Fails when the database can't be written.
pub async fn forget(pool: &SqlitePool) -> sqlx::Result<u64> {
    let lists = sqlx::query("DELETE FROM for_you").execute(pool).await?;
    let answers = sqlx::query("DELETE FROM listenbrainz_answers")
        .execute(pool)
        .await?;
    Ok(lists.rows_affected() + answers.rows_affected())
}

/// [`forget`], logged: run while For You is off, at startup and whenever the outside services'
/// settings are saved.
pub async fn forget_while_off(pool: SqlitePool) {
    match forget(&pool).await {
        Ok(0) => {}
        Ok(forgotten) => tracing::info!(forgotten, "For You is off; forgot its lists"),
        Err(err) => tracing::warn!(
            error = format!("{err:#}"),
            "couldn't forget For You's lists"
        ),
    }
}

#[cfg(test)]
pub mod testing;

#[cfg(test)]
mod tests;
