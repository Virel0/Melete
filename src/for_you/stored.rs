//! Lists as the database keeps them.

use super::*;

/// A listener's list as kept.
#[derive(Debug, Default)]
pub(super) struct Kept {
    /// What their plays were when it was made ([`plays_key`]).
    pub plays: String,
    /// Its songs, best first.
    pub items: Vec<Item>,
    /// When it was made; none before it ever was.
    pub made_at: Option<i64>,
    /// When making it was last started.
    pub tried_at: i64,
}

/// `user`'s list as kept.
pub(super) async fn kept(pool: &SqlitePool, user: i64) -> sqlx::Result<Option<Kept>> {
    let row: Option<(String, String, Option<i64>, i64)> =
        sqlx::query_as("SELECT plays, items, made_at, tried_at FROM for_you WHERE user_id = ?")
            .bind(user)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(plays, items, made_at, tried_at)| Kept {
        plays,
        // A list kept by an older Melete that can't be read is no list: it's made again.
        items: serde_json::from_str(&items).unwrap_or_default(),
        made_at,
        tried_at,
    }))
}

/// Notes that making `user`'s list started now, from their plays as `plays`.
pub(super) async fn claim(pool: &SqlitePool, user: i64, plays: &str, now: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO for_you (user_id, plays, items, tried_at) VALUES (?1, ?2, '[]', ?3)
         ON CONFLICT (user_id) DO UPDATE SET tried_at = excluded.tried_at",
    )
    .bind(user)
    .bind(plays)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps `user`'s new list, made from their plays as `plays`.
pub(super) async fn keep(
    pool: &SqlitePool,
    user: i64,
    plays: &str,
    items: &[Item],
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO for_you (user_id, plays, items, made_at, tried_at) VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT (user_id) DO UPDATE SET plays = excluded.plays, items = excluded.items,
             made_at = excluded.made_at, error = NULL",
    )
    .bind(user)
    .bind(plays)
    .bind(serde_json::to_string(items)?)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps why making `user`'s list failed; the list from before stays.
pub(super) async fn fail(pool: &SqlitePool, user: i64, why: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE for_you SET error = ? WHERE user_id = ?")
        .bind(why)
        .bind(user)
        .execute(pool)
        .await?;
    Ok(())
}
