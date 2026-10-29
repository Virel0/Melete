//! The user an identity is linked to, and linking one in place of any other at that provider.

use super::*;

pub(super) async fn linked_user(
    pool: &SqlitePool,
    identity: &Identity,
) -> anyhow::Result<Option<User>> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.username, u.is_admin FROM oidc_identities o JOIN users u ON u.id = o.user_id
         WHERE o.issuer = ? AND o.subject = ?",
    )
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .fetch_optional(pool)
    .await?)
}

pub(super) enum Linked {
    Done,
    /// Linked to someone else already.
    Taken,
    /// The user who asked is gone.
    NoUser,
}

/// Links an identity to a user, in place of any other they had at that provider.
pub(super) async fn link_identity(
    pool: &SqlitePool,
    user_id: i64,
    identity: &Identity,
) -> anyhow::Result<Linked> {
    let mut tx = pool.begin().await?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Ok(Linked::NoUser);
    }
    let owner: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM oidc_identities WHERE issuer = ? AND subject = ?")
            .bind(&identity.issuer)
            .bind(&identity.subject)
            .fetch_optional(&mut *tx)
            .await?;
    if owner.is_some_and(|o| o != user_id) {
        return Ok(Linked::Taken);
    }
    sqlx::query("DELETE FROM oidc_identities WHERE user_id = ? AND issuer = ?")
        .bind(user_id)
        .bind(&identity.issuer)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO oidc_identities (issuer, subject, user_id, linked_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&identity.issuer)
    .bind(&identity.subject)
    .bind(user_id)
    .bind(crate::db::unix_now())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Linked::Done)
}
