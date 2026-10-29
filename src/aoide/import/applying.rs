//! Writing the plan into Melete's sync store, in one transaction.

use super::*;

/// Writes the plan in one transaction, into a sync store nobody has used: the ops keep their
/// sequence numbers, which mustn't collide with or fall behind any Melete has handed out.
pub async fn apply(melete: &SqlitePool, plan: &Plan) -> anyhow::Result<()> {
    if let Some(user) = plan.users.iter().find(|u| u.mapping == Mapping::Unmapped) {
        bail!(
            "the Jellyfin user {} isn't mapped: add --user {}=<name> or --skip-user {}",
            user.jellyfin_id,
            user.jellyfin_id,
            user.jellyfin_id
        );
    }
    let mut tx = melete.begin().await?;
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM aoide_ops)
             OR EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'aoide_ops' AND seq > 0)",
    )
    .fetch_one(&mut *tx)
    .await?;
    if used {
        bail!(
            "Melete's sync store has already been used, so the sidecar's sequence numbers can't be kept; the import runs once, before anyone syncs to Melete"
        );
    }
    for (op, user) in &plan.ops {
        sqlx::query(
            "INSERT INTO aoide_ops (seq, op_id, user_id, device_id, entity, entity_id, operation,
                 payload, created_at, received_at, playlist_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(op.seq)
        .bind(&op.op_id)
        .bind(user)
        .bind(&op.device_id)
        .bind(&op.entity)
        .bind(&op.entity_id)
        .bind(&op.operation)
        .bind(&op.payload)
        .bind(op.created_at)
        .bind(op.received_at)
        .bind(&op.playlist_id)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("writing op {}", op.seq))?;
    }
    for (playlist, owner, at) in &plan.owners {
        sqlx::query(
            "INSERT INTO aoide_playlist_owners (playlist_id, owner_user_id, created_at)
             VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(playlist)
        .bind(owner)
        .bind(at)
        .execute(&mut *tx)
        .await?;
    }
    for (playlist, grantee, can_edit, at) in &plan.shares {
        sqlx::query(
            "INSERT INTO aoide_playlist_shares (playlist_id, grantee_user_id, can_edit, created_at)
             VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(playlist)
        .bind(grantee)
        .bind(i64::from(*can_edit != 0))
        .bind(at)
        .execute(&mut *tx)
        .await?;
    }
    for (user, image) in &plan.images {
        sqlx::query(
            "INSERT INTO aoide_images (user_id, image_hash, mime_type, bytes, size, created_at)
             VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(user)
        .bind(&image.image_hash)
        .bind(&image.mime_type)
        .bind(&image.bytes)
        .bind(image.bytes.len() as i64)
        .bind(image.created_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
