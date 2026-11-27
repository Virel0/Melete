//! Waiting for everyone in a watch party. A player that's loading or buffering says so; a
//! playing party then pauses where it is (`held`) and names who it waits for. Once nobody's
//! waiting it plays on from there, and a wait over [`MOST_MS`] is let go, so one slow
//! connection can't hold everyone: that player catches up when it can. Anyone playing or
//! pausing by hand ends the waiting.

use sqlx::SqlitePool;

use crate::db::now_ms;

/// The longest a party waits for anyone.
pub const MOST_MS: i64 = 30_000;

/// Who waits: a household device by its sign-in, or a guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiter {
    Device(i64),
    Guest(i64),
}

impl Waiter {
    fn key(self) -> String {
        match self {
            Waiter::Device(token) => format!("d{token}"),
            Waiter::Guest(guest) => format!("g{guest}"),
        }
    }
}

/// Says the waiter is waiting (loading or buffering) or ready again; a playing party pauses
/// for a new wait, where it is.
pub async fn set(
    pool: &SqlitePool,
    party: i64,
    (waiter, name): (Waiter, &str),
    waiting: bool,
) -> sqlx::Result<()> {
    let now = now_ms();
    if waiting {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO watch_party_waits (party_id, waiter, name, since) VALUES (?, ?, ?, ?)
             ON CONFLICT DO NOTHING",
        )
        .bind(party)
        .bind(waiter.key())
        .bind(name)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE watch_parties SET position_ms = position_ms + max(0, ?1 - position_at),
                 position_at = ?1, playing = 0, held = 1, version = version + 1, updated_at = ?1
             WHERE id = ?2 AND playing = 1",
        )
        .bind(now)
        .bind(party)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    } else {
        sqlx::query("DELETE FROM watch_party_waits WHERE party_id = ? AND waiter = ?")
            .bind(party)
            .bind(waiter.key())
            .execute(pool)
            .await?;
    }
    settle(pool, party).await
}

/// Lets go of waits over [`MOST_MS`] and of anyone no longer in the party; a party held with
/// nobody left waiting plays on from where it paused.
pub async fn settle(pool: &SqlitePool, party: i64) -> sqlx::Result<()> {
    let now = now_ms();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "DELETE FROM watch_party_waits WHERE party_id = ?1 AND (since < ?2
             OR (waiter GLOB 'd*' AND CAST(substr(waiter, 2) AS INTEGER) NOT IN
                 (SELECT token_id FROM watch_party_members WHERE party_id = ?1))
             OR (waiter GLOB 'g*' AND CAST(substr(waiter, 2) AS INTEGER) NOT IN
                 (SELECT id FROM watch_party_guests WHERE party_id = ?1)))",
    )
    .bind(party)
    .bind(now - MOST_MS)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE watch_parties SET playing = 1, held = 0, position_at = ?3,
             version = version + 1, updated_at = ?1
         WHERE id = ?2 AND held = 1
           AND NOT EXISTS (SELECT 1 FROM watch_party_waits WHERE party_id = ?2)",
    )
    .bind(now)
    .bind(party)
    .bind(super::starts_at(now, true))
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Ends any waiting: someone played or paused by hand.
pub async fn clear(pool: &SqlitePool, party: i64) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM watch_party_waits WHERE party_id = ?")
        .bind(party)
        .execute(pool)
        .await?;
    Ok(())
}

/// What a guest is told of a household device the party waits for: never an account's name.
pub const SOMEONE: &str = "someone from the household";

/// Who the party waits for, by name, first first, each with whether it's a guest; settled
/// first when it's waiting at all, so a wait that's over shows as over. Writes nothing when
/// nobody waits.
pub async fn names(pool: &SqlitePool, party: i64) -> sqlx::Result<Vec<(String, bool)>> {
    let any: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM watch_party_waits WHERE party_id = ?1)
             OR EXISTS (SELECT 1 FROM watch_parties WHERE id = ?1 AND held = 1)",
    )
    .bind(party)
    .fetch_one(pool)
    .await?;
    if !any {
        return Ok(Vec::new());
    }
    settle(pool, party).await?;
    sqlx::query_as(
        "SELECT name, waiter GLOB 'g*' FROM watch_party_waits WHERE party_id = ? ORDER BY since",
    )
    .bind(party)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests;
