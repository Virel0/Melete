//! Who a search or a discovery answer listed each picture of something not in the library for:
//! a discovery poster ([`crate::discover_posters`]), a release group's cover
//! ([`crate::search_covers`]) or an artist's portrait ([`crate::artist_portraits`]). Their routes
//! serve one only to a user it was listed for in the last 30 days, so nobody can tell from them
//! what someone else searched for lately. Each kind keeps at most its own number of pictures:
//! when a listing takes it over, [`PRUNE`] wakes the forgetting at once rather than at the next
//! housekeeping, so scripted searches can't make Melete fetch and keep more meanwhile.

use sqlx::SqlitePool;
use tokio::sync::Notify;

/// Woken when a listing takes a kind over its most kept; the pictures past their time or
/// beyond it are forgotten then, with their files.
pub static PRUNE: Notify = Notify::const_new();

/// Held while pictures are forgotten, and while a route keeps a picture it fetched once it has
/// checked the picture is still remembered: a picture forgotten during its fetch isn't then
/// written where nothing remembers it, to stay on disk for good.
pub static KEEPING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Which pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Poster,
    Cover,
    Portrait,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Poster => "poster",
            Kind::Cover => "cover",
            Kind::Portrait => "portrait",
        }
    }
}

/// Notes that `key` was listed for `user` at `now`.
pub async fn note(
    pool: &SqlitePool,
    kind: Kind,
    key: &str,
    user: i64,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO picture_listings (kind, key, user_id, listed_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (kind, key, user_id) DO UPDATE SET listed_at = excluded.listed_at",
    )
    .bind(kind.name())
    .bind(key)
    .bind(user)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether `key` was listed for `user` since `since`.
pub async fn listed_for(
    pool: &SqlitePool,
    kind: Kind,
    key: &str,
    user: i64,
    since: i64,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM picture_listings
             WHERE kind = ? AND key = ? AND user_id = ? AND listed_at >= ?)",
    )
    .bind(kind.name())
    .bind(key)
    .bind(user)
    .bind(since)
    .fetch_one(pool)
    .await
}

/// Notes that `key` couldn't be had when `user` asked for it.
pub async fn missed(
    pool: &SqlitePool,
    kind: Kind,
    key: &str,
    user: i64,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE picture_listings SET missed_at = ? WHERE kind = ? AND key = ? AND user_id = ?",
    )
    .bind(now)
    .bind(kind.name())
    .bind(key)
    .bind(user)
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether `key` couldn't be had when `user` asked for it, since `since`.
pub async fn missed_for(
    pool: &SqlitePool,
    kind: Kind,
    key: &str,
    user: i64,
    since: i64,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM picture_listings
             WHERE kind = ? AND key = ? AND user_id = ? AND missed_at >= ?)",
    )
    .bind(kind.name())
    .bind(key)
    .bind(user)
    .bind(since)
    .fetch_one(pool)
    .await
}

/// Forgets who `key` was listed for, as its picture is forgotten.
pub async fn forget(pool: &SqlitePool, kind: Kind, key: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM picture_listings WHERE kind = ? AND key = ?")
        .bind(kind.name())
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Forgets listings older than `before`. How many.
pub async fn forget_older(pool: &SqlitePool, before: i64) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM picture_listings WHERE listed_at < ?")
            .bind(before)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// Wakes the forgetting when `kept` pictures of a kind are over its `most`.
pub fn over(kept: i64, most: i64) {
    if kept > most {
        PRUNE.notify_one();
    }
}

#[cfg(test)]
mod tests;
