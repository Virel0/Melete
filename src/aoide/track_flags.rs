//! A listener's track flags, read from their sync log and written into it the way an Aoide
//! device does, so the web can set them and every device follows: "not interested" keeps a
//! track out of mixes, stations, shuffle and smart playlists; "don't count plays" keeps its
//! new listens out of the history.
//!
//! A track's flags are one row of the `track_flags` entity: its own id, the track
//! (`jellyfinId`, Melete's track id, and `contentKey`), `notInterested`, `dontCount`,
//! `updatedAt`, `deleted`, `originDevice`, and `fieldUpdatedAt`, when each flag last changed,
//! which the apps merge flag by flag. A change here stamps only the flag it changes, so a
//! device's change to the other flag isn't undone; a flag the row never stamped keeps the
//! row's old time. The row read is the latest op naming the track, as the server reads flags
//! elsewhere ([`super::listening`]).

use serde_json::{Map, Value};
use sqlx::SqlitePool;

use super::listening::normalise;
use super::writing::{self, Change, WriteError};

/// One of a track's two flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    NotInterested,
    DontCount,
}

impl Flag {
    /// Its field in the row.
    pub fn field(self) -> &'static str {
        match self {
            Self::NotInterested => "notInterested",
            Self::DontCount => "dontCount",
        }
    }
}

/// A track's flags as they stand: the row (as its latest op wrote it) and both flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Flags {
    /// The track's id as the row wrote it.
    pub track: String,
    pub content_key: String,
    pub not_interested: bool,
    pub dont_count: bool,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: i64,
    row: Map<String, Value>,
}

fn read(payload: &str) -> Option<Flags> {
    let Ok(Value::Object(row)) = serde_json::from_str::<Value>(payload) else {
        return None;
    };
    let get = |camel: &str, snake: &str| row.get(camel).or_else(|| row.get(snake)).cloned();
    let flag = |camel: &str, snake: &str| match get(camel, snake) {
        Some(Value::Bool(b)) => b,
        Some(Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    };
    let track = get("jellyfinId", "jellyfin_id")?.as_str()?.to_owned();
    let deleted = flag("deleted", "deleted");
    Some(Flags {
        content_key: get("contentKey", "content_key")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        not_interested: !deleted && flag("notInterested", "not_interested"),
        dont_count: !deleted && flag("dontCount", "dont_count"),
        updated_at: get("updatedAt", "updated_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        track,
        row,
    })
}

/// Every track `user_id` has a flags row for, each as its latest op wrote it, the latest
/// change first.
///
/// # Errors
///
/// Fails when the database does.
pub async fn all(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Flags>> {
    let payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload FROM aoide_ops WHERE user_id = ? AND entity = 'track_flags' ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let mut latest: Vec<Flags> = Vec::new();
    for flags in payloads.iter().filter_map(|p| read(p)) {
        let key = normalise(&flags.track);
        latest.retain(|f| normalise(&f.track) != key);
        latest.push(flags);
    }
    latest.reverse();
    Ok(latest)
}

/// Sets or clears `flag` on track `track_id` for `user_id`, as a device of theirs named
/// `device` would at `now`: the track's row with that flag changed and stamped (a new row when
/// it has none). Nothing is written when it's already so. False only when setting a flag on a
/// track that isn't in the library.
///
/// # Errors
///
/// A database error, or no randomness for a new row's id.
pub async fn set(
    pool: &SqlitePool,
    user_id: i64,
    device: &str,
    track_id: i64,
    flag: Flag,
    on: bool,
    now: i64,
) -> Result<bool, WriteError> {
    let wanted = track_id.to_string();
    let held = all(pool, user_id)
        .await?
        .into_iter()
        .find(|f| normalise(&f.track) == wanted);
    let current = held.as_ref().is_some_and(|f| match flag {
        Flag::NotInterested => f.not_interested,
        Flag::DontCount => f.dont_count,
    });
    if current == on {
        return Ok(true);
    }
    let row = match held {
        Some(held) => changed(held, flag, on, device, now),
        None => {
            let Some(link) = writing::track_link(pool, track_id).await? else {
                return Ok(false);
            };
            fresh(writing::new_id()?, link, flag, on, device, now)
        }
    };
    let change = Change {
        entity: "track_flags",
        delete: false,
        row,
    };
    writing::append(pool, user_id, device, vec![change], now).await?;
    Ok(true)
}

/// A new row for a track with no flags yet: `flag` set to `on`, the other clear.
fn fresh(
    id: String,
    link: writing::TrackLink,
    flag: Flag,
    on: bool,
    device: &str,
    now: i64,
) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert("id".into(), id.into());
    row.insert("jellyfinId".into(), link.jellyfin_id.into());
    row.insert("contentKey".into(), link.content_key.into());
    row.insert("notInterested".into(), false.into());
    row.insert("dontCount".into(), false.into());
    row.insert(flag.field().into(), on.into());
    row.insert("updatedAt".into(), now.into());
    row.insert("deleted".into(), false.into());
    row.insert("originDevice".into(), device.into());
    let mut stamps = Map::new();
    stamps.insert(flag.field().into(), now.into());
    row.insert("fieldUpdatedAt".into(), stamps.into());
    row
}

/// The held row with `flag` set to `on`, stamped at `now` (after the row's own time, so it
/// wins); a flag it never stamped keeps the row's old time, so this doesn't claim it.
fn changed(held: Flags, flag: Flag, on: bool, device: &str, now: i64) -> Map<String, Value> {
    let at = now.max(held.updated_at.saturating_add(1));
    let mut row = held.row;
    let mut stamps = match row
        .remove("fieldUpdatedAt")
        .or_else(|| row.remove("field_updated_at"))
    {
        Some(Value::Object(stamps)) => stamps,
        _ => Map::new(),
    };
    for other in [Flag::NotInterested, Flag::DontCount] {
        stamps
            .entry(other.field())
            .or_insert_with(|| held.updated_at.into());
    }
    stamps.insert(flag.field().into(), at.into());
    // Snake-case fields from an older writer are written back as the apps write them.
    for (snake, camel) in [
        ("jellyfin_id", "jellyfinId"),
        ("content_key", "contentKey"),
        ("not_interested", "notInterested"),
        ("dont_count", "dontCount"),
        ("updated_at", "updatedAt"),
        ("origin_device", "originDevice"),
    ] {
        if let Some(value) = row.remove(snake) {
            row.entry(camel).or_insert(value);
        }
    }
    row.insert("notInterested".into(), held.not_interested.into());
    row.insert("dontCount".into(), held.dont_count.into());
    row.insert(flag.field().into(), on.into());
    row.insert("updatedAt".into(), at.into());
    row.insert("deleted".into(), false.into());
    row.insert("originDevice".into(), device.into());
    row.insert("fieldUpdatedAt".into(), stamps.into());
    row
}
