//! The user's own record of changes to how they sign in: turning the second factor on and off,
//! and wrong codes typed at sign-in, so someone whose password got out can tell.

use super::*;

/// Account changes kept in a user's record as it's shown.
const CHANGES_SHOWN: i64 = 20;
/// Wrong codes typed within this long of the last one count together in one entry.
const WRONG_CODES_TOGETHER: i64 = 3600;

/// One change to how a user signs in, for their record.
#[derive(Debug, Serialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Change {
    /// `two_factor_on`, `two_factor_off` or `wrong_sign_in_code`.
    pub change: String,
    /// `account`, `server` or `sign_in`.
    pub made_from: String,
    /// When, Unix seconds; for wrong codes, the latest.
    pub at: i64,
    /// How many: 1, except for wrong codes, counted together within an hour of the last.
    pub count: i64,
}

/// The user's latest changes to how they sign in, newest first.
pub async fn changes(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Change>> {
    sqlx::query_as(
        "SELECT change, made_from, at, count FROM account_changes WHERE user_id = ?
         ORDER BY at DESC, id DESC LIMIT ?",
    )
    .bind(user_id)
    .bind(CHANGES_SHOWN)
    .fetch_all(pool)
    .await
}

/// Adds a change, made `from` the account or the server, inside the transaction that made it.
pub(super) async fn record(
    tx: &mut sqlx::SqliteConnection,
    user_id: i64,
    change: &str,
    from: MadeFrom,
    at: i64,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO account_changes (user_id, change, made_from, at) VALUES (?, ?, ?, ?)")
        .bind(user_id)
        .bind(change)
        .bind(from.as_str())
        .bind(at)
        .execute(tx)
        .await?;
    Ok(())
}

/// Notes a wrong code typed at sign-in, in the user's record: added to the newest entry when
/// that's wrong codes within the hour, else a new one. Where it was typed from isn't kept.
pub async fn record_wrong_code(pool: &SqlitePool, user_id: i64, now: i64) -> sqlx::Result<()> {
    let counted = sqlx::query(
        "UPDATE account_changes SET count = count + 1, at = ?1
         WHERE id = (SELECT id FROM account_changes WHERE user_id = ?2 ORDER BY at DESC, id DESC
                     LIMIT 1)
           AND change = 'wrong_sign_in_code' AND at > ?1 - ?3",
    )
    .bind(now)
    .bind(user_id)
    .bind(WRONG_CODES_TOGETHER)
    .execute(pool)
    .await?
    .rows_affected();
    if counted == 0 {
        sqlx::query(
            "INSERT INTO account_changes (user_id, change, made_from, at)
             VALUES (?, 'wrong_sign_in_code', 'sign_in', ?)",
        )
        .bind(user_id)
        .bind(now)
        .execute(pool)
        .await?;
    }
    Ok(())
}
