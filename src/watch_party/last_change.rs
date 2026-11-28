//! What last changed in a watch party, and who changed it: played, paused, jumped (`seek`) or
//! moved on to another file (`next`), by a household member or a guest, so every player can
//! say "Paused by …" rather than going still without a word. A change that leaves the party as
//! it was (the same file, playing or not as before, where it would be anyway) isn't one.
//! Waiting for someone to load, and playing on after, are told by `held` and `waits` instead.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::waits::SOMEONE;

/// How far from where it would be a position must be to count as a jump.
pub const SEEK_MS: i64 = 2000;

/// Who made a change.
#[derive(Debug, Clone, Copy)]
pub enum By<'a> {
    /// A household member, by their account.
    User(i64),
    /// A guest: their id, and the name they gave.
    Guest(i64, &'a str),
}

/// A party as it stood before a change.
#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct Before {
    pub file_id: i64,
    pub position_ms: i64,
    pub position_at: i64,
    pub playing: bool,
}

/// The party `id` as it stands, for a change about to be made.
pub async fn before(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Before>> {
    sqlx::query_as(
        "SELECT file_id, position_ms, position_at, playing FROM watch_parties WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// What setting `(file_id, position_ms, playing)` at `now` changes in a party that was
/// `before`: another file is `next`; playing or pausing is `play` or `pause`; otherwise a
/// position [`SEEK_MS`] or more from where it would be is `seek`, and anything less is nothing.
pub fn kind(
    before: &Before,
    (file_id, position_ms, playing): (i64, i64, bool),
    now: i64,
) -> Option<&'static str> {
    if file_id != before.file_id {
        return Some("next");
    }
    match (before.playing, playing) {
        (false, true) => return Some("play"),
        (true, false) => return Some("pause"),
        _ => {}
    }
    let moved = if before.playing {
        (now - before.position_at).max(0)
    } else {
        0
    };
    let expected = before.position_ms + moved;
    ((position_ms - expected).abs() >= SEEK_MS).then_some("seek")
}

/// Notes `kind`, made by `by` at `now`, as the party's last change.
pub async fn note(
    pool: &SqlitePool,
    id: i64,
    by: By<'_>,
    kind: &str,
    now: i64,
) -> sqlx::Result<()> {
    let (user, guest, name) = match by {
        By::User(user) => (Some(user), None, None),
        By::Guest(guest, name) => (None, Some(guest), Some(name)),
    };
    sqlx::query(
        "UPDATE watch_parties SET last_user_id = ?, last_guest_id = ?, last_guest_name = ?,
             last_kind = ?, last_at = ? WHERE id = ?",
    )
    .bind(user)
    .bind(guest)
    .bind(name)
    .bind(kind)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Who's asking: a household device (its user, and its sign-in), or a guest.
#[derive(Debug, Clone, Copy)]
pub enum Viewer {
    Device { user: i64, token: i64 },
    Guest(i64),
}

#[derive(sqlx::FromRow)]
struct Last {
    user_id: Option<i64>,
    username: Option<String>,
    guest_id: Option<i64>,
    guest_name: Option<String>,
    kind: Option<String>,
    at: Option<i64>,
}

/// The party's last change as `viewer` is shown it: `{by, guest, you, kind, at}`, or null
/// before anyone has changed anything. A guest is told a household member is
/// "someone from the household", never their account's name; a member whose account has gone
/// is that to everyone.
pub async fn view(pool: &SqlitePool, id: i64, viewer: Viewer) -> sqlx::Result<Value> {
    let last: Option<Last> = sqlx::query_as(
        "SELECT p.last_user_id AS user_id, u.username, p.last_guest_id AS guest_id,
             p.last_guest_name AS guest_name,
             p.last_kind AS kind, p.last_at AS at
         FROM watch_parties p LEFT JOIN users u ON u.id = p.last_user_id WHERE p.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(Last {
        user_id,
        username,
        guest_id,
        guest_name,
        kind: Some(kind),
        at: Some(at),
    }) = last
    else {
        return Ok(Value::Null);
    };
    let guest = guest_name.is_some();
    let (by, you) = match (guest_name, viewer) {
        (Some(name), Viewer::Guest(me)) => (name, guest_id == Some(me)),
        (Some(name), Viewer::Device { .. }) => (name, false),
        (None, Viewer::Device { user, .. }) => (
            username.unwrap_or_else(|| SOMEONE.to_owned()),
            user_id == Some(user),
        ),
        (None, Viewer::Guest(_)) => (SOMEONE.to_owned(), false),
    };
    Ok(json!({ "by": by, "guest": guest, "you": you, "kind": kind, "at": at }))
}

#[cfg(test)]
mod tests;
