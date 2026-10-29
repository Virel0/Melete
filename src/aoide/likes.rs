//! A listener's liked tracks, read from their sync log and written into it the way an Aoide
//! device does, so the log stays the one place likes are kept and every device sees a like
//! the web set.
//!
//! A like is a row of the `likes` entity: its own id, the track (`jellyfinId`, Melete's
//! track id, and `contentKey`), `liked` (false is a dislike), `updatedAt`, `deleted` and
//! `originDevice`. Rows merge whole: the later `updatedAt` wins, a tie going to the greater
//! `originDevice`. A track has one row: when two devices each made one, the one that wins
//! that comparison stays and the other goes. Withdrawing a like deletes its row (a delete op
//! carrying the row with `deleted` set).

use std::collections::HashMap;

use serde_json::{Map, Value};
use sqlx::SqlitePool;

use super::listening::normalise;
use super::playlists::Row;
use super::writing::{self, Change, WriteError};

/// A like row as a device holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Like {
    /// The row's id.
    pub id: String,
    /// The track's id as written.
    pub jellyfin_id: String,
    /// The track's content key.
    pub content_key: String,
    /// True for a like, false for a dislike.
    pub liked: bool,
    /// When it last changed, in milliseconds since the epoch.
    pub updated_at: i64,
    /// Whether it's been withdrawn.
    pub deleted: bool,
    /// The device that changed it last.
    pub origin_device: String,
}

impl Like {
    /// A row from an op's payload, or None when a device couldn't read it: a field missing or
    /// of the wrong type, or an id that isn't the op's.
    fn read(entity_id: &str, operation: &str, payload: &str) -> Option<Self> {
        let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(payload) else {
            return None;
        };
        let row: Row = raw.into_iter().map(|(k, v)| (camel(&k), v)).collect();
        let text = |key: &str| row.get(key)?.as_str().map(str::to_owned);
        let flag = |key: &str| match row.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => n.as_i64().map(|n| n != 0),
            _ => None,
        };
        let like = Self {
            id: text("id")?,
            jellyfin_id: text("jellyfinId")?,
            content_key: text("contentKey")?,
            liked: flag("liked")?,
            updated_at: row.get("updatedAt")?.as_i64()?,
            deleted: flag("deleted")?,
            origin_device: text("originDevice")?,
        };
        // A delete has to say so in the row; one that doesn't is refused by every device.
        let consistent = like.id == entity_id && (operation != "delete" || like.deleted);
        consistent.then_some(like)
    }

    /// Whether this write beats `other`: the later change, a tie to the greater device.
    fn beats(&self, other: &Self) -> bool {
        if self.updated_at == other.updated_at {
            self.origin_device > other.origin_device
        } else {
            self.updated_at > other.updated_at
        }
    }

    /// The row as its payload.
    fn to_row(&self) -> Map<String, Value> {
        let mut row = Map::new();
        row.insert("id".into(), self.id.clone().into());
        row.insert("jellyfinId".into(), self.jellyfin_id.clone().into());
        row.insert("contentKey".into(), self.content_key.clone().into());
        row.insert("liked".into(), self.liked.into());
        row.insert("updatedAt".into(), self.updated_at.into());
        row.insert("deleted".into(), self.deleted.into());
        row.insert("originDevice".into(), self.origin_device.clone().into());
        row
    }
}

/// `content_key` → `contentKey`: logs may hold either naming.
fn camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for c in key.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The rows after applying `ops` (entity id, operation, payload) in order, as a device's
/// merge does, by row id.
pub fn fold<'a>(ops: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>) -> Vec<Like> {
    let mut rows: HashMap<String, Like> = HashMap::new();
    for (entity_id, operation, payload) in ops {
        let Some(incoming) = Like::read(entity_id, operation, payload) else {
            continue;
        };
        if rows
            .get(&incoming.id)
            .is_some_and(|held| !incoming.beats(held))
        {
            continue;
        }
        let clash = rows
            .values()
            .find(|r| r.jellyfin_id == incoming.jellyfin_id && r.id != incoming.id)
            .cloned();
        if let Some(clash) = clash {
            if !incoming.beats(&clash) {
                continue;
            }
            rows.remove(&clash.id);
        }
        rows.insert(incoming.id.clone(), incoming);
    }
    rows.into_values().collect()
}

/// Every like row `user_id` has, as their devices hold them.
///
/// # Errors
///
/// Fails when the database does.
pub async fn rows(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Like>> {
    let ops: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT entity_id, operation, payload FROM aoide_ops
         WHERE user_id = ? AND entity = 'likes' ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(fold(
        ops.iter()
            .map(|(e, o, p)| (e.as_str(), o.as_str(), p.as_str())),
    ))
}

/// The tracks `user_id` likes, newest like first: (Melete's track id, or None when the row
/// names something that isn't one; the row).
///
/// # Errors
///
/// Fails when the database does.
pub async fn liked(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<(Option<i64>, Like)>> {
    let mut liked: Vec<Like> = rows(pool, user_id)
        .await?
        .into_iter()
        .filter(|r| r.liked && !r.deleted)
        .collect();
    liked.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(liked
        .into_iter()
        .map(|r| (normalise(&r.jellyfin_id).parse().ok(), r))
        .collect())
}

/// Likes or unlikes `track_id` for `user_id`, as a device of theirs named `device` would at
/// `now`: liking writes the track's row (its own, if it has one) liked and not deleted;
/// unliking deletes its row when that's a like. Nothing is written when it's already so.
/// False only when liking a track that isn't in the library.
///
/// # Errors
///
/// A database error, or no randomness for a new row's id.
pub async fn set(
    pool: &SqlitePool,
    user_id: i64,
    device: &str,
    track_id: i64,
    like: bool,
    now: i64,
) -> Result<bool, WriteError> {
    let wanted = track_id.to_string();
    // The track's row, as the device that made it holds it; the latest if ids were written
    // two ways.
    let held = rows(pool, user_id)
        .await?
        .into_iter()
        .filter(|r| normalise(&r.jellyfin_id) == wanted)
        .reduce(|a, b| if b.beats(&a) { b } else { a });
    let row = match (held, like) {
        (Some(held), true) if held.liked && !held.deleted => return Ok(true),
        // Only a like is withdrawn here; a dislike stays.
        (Some(held), false) if held.deleted || !held.liked => return Ok(true),
        (None, false) => return Ok(true),
        (Some(held), _) => Like {
            liked: like || held.liked,
            deleted: !like,
            // After the row it changes: two changes in one millisecond would tie, and a tie
            // from one device is the same write, so the second would be lost.
            updated_at: now.max(held.updated_at.saturating_add(1)),
            origin_device: device.to_owned(),
            ..held
        },
        (None, true) => {
            let Some(link) = writing::track_link(pool, track_id).await? else {
                return Ok(false);
            };
            Like {
                id: writing::new_id()?,
                jellyfin_id: link.jellyfin_id,
                content_key: link.content_key,
                liked: true,
                updated_at: now,
                deleted: false,
                origin_device: device.to_owned(),
            }
        }
    };
    let change = Change {
        entity: "likes",
        delete: row.deleted,
        row: row.to_row(),
    };
    writing::append(pool, user_id, device, vec![change], now).await?;
    Ok(true)
}

#[cfg(test)]
mod tests;
