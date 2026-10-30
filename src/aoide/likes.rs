//! A listener's liked tracks and albums, read from their sync log and written into it the way
//! an Aoide device does, so the log stays the one place likes are kept and every device sees a
//! like the web set.
//!
//! A track's like is a row of the `likes` entity: its own id, the track (`jellyfinId`,
//! Melete's track id, and `contentKey`), `liked` (false is a dislike), `updatedAt`, `deleted`
//! and `originDevice`. An album's is a row of `album_likes`, the same but for `albumId`, the
//! album's id, in place of `jellyfinId`: a record of its own rather than a flag on a track's,
//! so devices that don't know albums can be liked never see one. Rows merge whole: the later
//! `updatedAt` wins, a tie going to the greater `originDevice`. A track or album has one row:
//! when two devices each made one, the one that wins that comparison stays and the other goes.
//! Withdrawing a like deletes its row (a delete op carrying the row with `deleted` set).

use std::collections::HashMap;

use serde_json::{Map, Value};
use sqlx::SqlitePool;

use super::listening::normalise;
use super::playlists::Row;
use super::writing::{self, Change, WriteError};

/// What a like names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
}

impl Kind {
    /// The entity its rows are kept as.
    pub fn entity(self) -> &'static str {
        match self {
            Self::Track => "likes",
            Self::Album => "album_likes",
        }
    }

    /// The row's field naming the track or album.
    fn id_key(self) -> &'static str {
        match self {
            Self::Track => "jellyfinId",
            Self::Album => "albumId",
        }
    }
}

/// A like row as a device holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Like {
    /// The row's id.
    pub id: String,
    /// The track's or album's id as written.
    pub item_id: String,
    /// The track's or album's content key.
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
    fn read(kind: Kind, entity_id: &str, operation: &str, payload: &str) -> Option<Self> {
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
            item_id: text(kind.id_key())?,
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
    fn to_row(&self, kind: Kind) -> Map<String, Value> {
        let mut row = Map::new();
        row.insert("id".into(), self.id.clone().into());
        row.insert(kind.id_key().into(), self.item_id.clone().into());
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

/// The rows after applying `ops` (entity id, operation, payload) of `kind`'s entity in order,
/// as a device's merge does, by row id.
pub fn fold<'a>(
    kind: Kind,
    ops: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
) -> Vec<Like> {
    let mut rows: HashMap<String, Like> = HashMap::new();
    for (entity_id, operation, payload) in ops {
        let Some(incoming) = Like::read(kind, entity_id, operation, payload) else {
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
            .find(|r| r.item_id == incoming.item_id && r.id != incoming.id)
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

/// Every like row of `kind` `user_id` has, as their devices hold them.
///
/// # Errors
///
/// Fails when the database does.
pub async fn rows(pool: &SqlitePool, kind: Kind, user_id: i64) -> sqlx::Result<Vec<Like>> {
    let ops: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT entity_id, operation, payload FROM aoide_ops
         WHERE user_id = ? AND entity = ? ORDER BY seq",
    )
    .bind(user_id)
    .bind(kind.entity())
    .fetch_all(pool)
    .await?;
    Ok(fold(
        kind,
        ops.iter()
            .map(|(e, o, p)| (e.as_str(), o.as_str(), p.as_str())),
    ))
}

/// The tracks or albums `user_id` likes, newest like first: (Melete's id, or None when the
/// row names something that isn't one; the row).
///
/// # Errors
///
/// Fails when the database does.
pub async fn liked(
    pool: &SqlitePool,
    kind: Kind,
    user_id: i64,
) -> sqlx::Result<Vec<(Option<i64>, Like)>> {
    let mut liked: Vec<Like> = rows(pool, kind, user_id)
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
        .map(|r| (normalise(&r.item_id).parse().ok(), r))
        .collect())
}

/// Likes or unlikes the track or album `id` for `user_id`, as a device of theirs named
/// `device` would at `now`: liking writes its row (its own, if it has one) liked and not
/// deleted; unliking deletes its row when that's a like. Nothing is written when it's already
/// so. False only when liking a track or album that isn't in the library.
///
/// # Errors
///
/// A database error, or no randomness for a new row's id.
pub async fn set(
    pool: &SqlitePool,
    kind: Kind,
    user_id: i64,
    device: &str,
    id: i64,
    like: bool,
    now: i64,
) -> Result<bool, WriteError> {
    let wanted = id.to_string();
    // Its row, as the device that made it holds it; the latest if ids were written two ways.
    let held = rows(pool, kind, user_id)
        .await?
        .into_iter()
        .filter(|r| normalise(&r.item_id) == wanted)
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
            let link = match kind {
                Kind::Track => writing::track_link(pool, id)
                    .await?
                    .map(|l| (l.jellyfin_id, l.content_key)),
                Kind::Album => writing::album_link(pool, id)
                    .await?
                    .map(|l| (l.album_id, l.content_key)),
            };
            let Some((item_id, content_key)) = link else {
                return Ok(false);
            };
            Like {
                id: writing::new_id()?,
                item_id,
                content_key,
                liked: true,
                updated_at: now,
                deleted: false,
                origin_device: device.to_owned(),
            }
        }
    };
    let change = Change {
        entity: kind.entity(),
        delete: row.deleted,
        row: row.to_row(kind),
    };
    writing::append(pool, user_id, device, vec![change], now).await?;
    Ok(true)
}

#[cfg(test)]
mod tests;
