//! Removing an import, and approving every request of one still waiting.

use super::*;

/// Removes an import and its track list, its owner's or an admin's to do; None when
/// there's no such import for them. Its requests still waiting for an admin are taken back,
/// unless another import lists them too; approved ones stay, as they're on their way. One
/// being worked through stops before its next track. Returns how many were taken back.
pub async fn remove(pool: &SqlitePool, id: i64, user: &User) -> anyhow::Result<Option<u64>> {
    let mut tx = pool.begin().await?;
    let owner: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM playlist_imports WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    match owner {
        Some(owner) if owner == user.id || user.is_admin => {}
        _ => return Ok(None),
    }
    let taken_back = sqlx::query(
        "DELETE FROM requests WHERE state = 'requested' AND id IN (
             SELECT t.request_id FROM playlist_import_tracks t
             WHERE t.import_id = ?1 AND t.state = 'asked' AND t.request_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM playlist_import_tracks o
                               WHERE o.request_id = t.request_id AND o.import_id <> ?1))",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM playlist_imports WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(taken_back))
}

/// Approves every request of an import still waiting, then fetches them one after another in
/// the background. Returns how many were approved, and each that couldn't be, with why.
pub async fn approve_all(
    pool: &SqlitePool,
    mb: Option<&musicbrainz::Client>,
    sources: &fulfil::Sources,
    id: i64,
    admin: &User,
) -> anyhow::Result<(Vec<i64>, Vec<(i64, String)>)> {
    let waiting: Vec<i64> = sqlx::query_scalar(
        "SELECT r.id FROM playlist_import_tracks t JOIN requests r ON r.id = t.request_id
         WHERE t.import_id = ? AND t.state = 'asked' AND r.state = 'requested' ORDER BY t.position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let (mut approved, mut refused) = (Vec::new(), Vec::new());
    for request in waiting {
        match requests::approve(pool, None, mb, request, admin, &Choice::default()).await? {
            Ok(_) => approved.push(request),
            Err(why) => refused.push((request, why.to_string())),
        }
    }
    fulfil::start_each(pool, sources, approved.clone());
    Ok((approved, refused))
}
