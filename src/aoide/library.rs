//! Playlist artwork, collaborative playlists and handing playback between devices, for the
//! aoide-sidecar's contract. Times are milliseconds since the epoch.

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// The largest image accepted.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// The image types accepted, as MIME types.
pub const IMAGE_TYPES: [&str; 3] = ["image/jpeg", "image/png", "image/webp"];
/// How long an unreferenced image is kept: an upload comes before the op naming it, and a
/// device may be offline in between.
pub const GRACE_DAYS: i64 = 30;
const DAY_MS: i64 = 86_400_000;

/// A lowercase hex SHA-256.
pub fn is_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The lowercase hex SHA-256 of `bytes`, as image hashes are named.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Keeps an image under its hash (again is a no-op).
pub async fn store_image(
    pool: &SqlitePool,
    user_id: i64,
    hash: &str,
    mime: &str,
    bytes: &[u8],
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO aoide_images (user_id, image_hash, mime_type, bytes, size, created_at)
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (user_id, image_hash) DO NOTHING",
    )
    .bind(user_id)
    .bind(hash)
    .bind(mime)
    .bind(bytes)
    .bind(bytes.len() as i64)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// An image the user stored under `hash`, with its MIME type; none when there's no such image.
pub async fn image(
    pool: &SqlitePool,
    user_id: i64,
    hash: &str,
) -> sqlx::Result<Option<(Vec<u8>, String)>> {
    sqlx::query_as("SELECT bytes, mime_type FROM aoide_images WHERE user_id = ? AND image_hash = ?")
        .bind(user_id)
        .bind(hash)
        .fetch_optional(pool)
        .await
}

/// An image the user stored, else one stored by someone they share a playlist with: its owner,
/// another collaborator on it, or someone they shared one of theirs with. A shared
/// playlist's artwork is then seen by everyone on it, whoever uploaded it; nobody else's store
/// is reached.
pub async fn image_seen_by(
    pool: &SqlitePool,
    user_id: i64,
    hash: &str,
) -> sqlx::Result<Option<(Vec<u8>, String)>> {
    sqlx::query_as(
        "WITH theirs AS (
             SELECT playlist_id FROM aoide_playlist_owners WHERE owner_user_id = ?1
             UNION SELECT playlist_id FROM aoide_playlist_shares WHERE grantee_user_id = ?1),
         sharing AS (
             SELECT owner_user_id AS user_id FROM aoide_playlist_owners
              WHERE playlist_id IN (SELECT playlist_id FROM theirs)
             UNION SELECT grantee_user_id FROM aoide_playlist_shares
              WHERE playlist_id IN (SELECT playlist_id FROM theirs))
         SELECT bytes, mime_type FROM aoide_images
          WHERE image_hash = ?2
            AND (user_id = ?1 OR user_id IN (SELECT user_id FROM sharing))
          ORDER BY user_id = ?1 DESC LIMIT 1",
    )
    .bind(user_id)
    .bind(hash)
    .fetch_optional(pool)
    .await
}

fn field<'a>(payload: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| payload.get(*name))
}

/// 0/1, true/false and their strings, as clients have written them.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        Some(Value::String(s)) => s
            .parse::<bool>()
            .unwrap_or_else(|_| s.parse::<i64>().is_ok_and(|n| n != 0)),
        _ => false,
    }
}

/// The image hashes the user's playlists name now: each playlist's winning row (latest
/// `updated_at`, else `createdAt`; then the later op) when it isn't deleted.
pub async fn referenced_images(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<String>> {
    let rows: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT entity_id, operation, payload, created_at, seq FROM aoide_ops
         WHERE user_id = ? AND entity = 'playlists' ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let mut latest: std::collections::HashMap<String, (i64, i64, bool, Option<String>)> =
        std::collections::HashMap::new();
    for (id, operation, payload, created_at, seq) in rows {
        let Ok(payload) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        if !payload.is_object() {
            continue;
        }
        let stamp = field(&payload, &["updated_at", "updatedAt"])
            .and_then(Value::as_i64)
            .unwrap_or(created_at);
        let deleted = operation == "delete" || truthy(payload.get("deleted"));
        let hash = field(&payload, &["image_hash", "imageHash"])
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
        let wins = latest
            .get(&id)
            .is_none_or(|(s, q, _, _)| stamp > *s || (stamp == *s && seq > *q));
        if wins {
            latest.insert(id, (stamp, seq, deleted, hash));
        }
    }
    Ok(latest
        .into_values()
        .filter(|(_, _, deleted, _)| !deleted)
        .filter_map(|(_, _, _, hash)| hash)
        .collect())
}

/// One stored image no playlist names now.
#[derive(Debug, PartialEq)]
pub struct Orphan {
    /// The image's hash.
    pub hash: String,
    /// Its size in bytes.
    pub size: i64,
    /// Whole days since it was stored; never negative.
    pub age_days: i64,
    /// Whether it's past the grace period, so a reclaim deletes it.
    pub reclaimable: bool,
}

/// A user's stored images, those no playlist names, and what a reclaim deleted.
#[derive(Debug, Default)]
pub struct Orphans {
    /// How many images the user has stored.
    pub total_blobs: i64,
    /// Their bytes in all.
    pub total_bytes: i64,
    /// The images no playlist names, oldest first; after a reclaim, only those still in their grace
    /// period.
    pub orphans: Vec<Orphan>,
    /// The bytes those images take.
    pub orphan_bytes: i64,
    /// The grace period used, in days: never fewer than [`GRACE_DAYS`].
    pub grace_days: i64,
    /// How many images the reclaim deleted; 0 when not reclaiming.
    pub reclaimed: i64,
    /// The bytes the reclaim freed.
    pub reclaimed_bytes: i64,
}

/// Reports, and with `reclaim` deletes, images no playlist names that were stored at least
/// `grace_days` ago (never fewer than [`GRACE_DAYS`]). Nothing does this on its own.
pub async fn orphans(
    pool: &SqlitePool,
    user_id: i64,
    grace_days: i64,
    reclaim: bool,
    now: i64,
) -> sqlx::Result<Orphans> {
    let grace_days = grace_days.max(GRACE_DAYS);
    let referenced = referenced_images(pool, user_id).await?;
    let blobs: Vec<(String, i64, i64)> =
        sqlx::query_as("SELECT image_hash, size, created_at FROM aoide_images WHERE user_id = ?")
            .bind(user_id)
            .fetch_all(pool)
            .await?;
    let mut report = Orphans {
        total_blobs: blobs.len() as i64,
        grace_days,
        ..Orphans::default()
    };
    for (hash, size, created_at) in blobs {
        report.total_bytes += size;
        if referenced.contains(&hash) {
            continue;
        }
        // A clock that went backwards gives no negative age: too young is the safe side.
        let age_days = ((now - created_at) / DAY_MS).max(0);
        report.orphan_bytes += size;
        report.orphans.push(Orphan {
            hash,
            size,
            age_days,
            reclaimable: age_days >= grace_days,
        });
    }
    report
        .orphans
        .sort_by_key(|o| std::cmp::Reverse(o.age_days));
    if reclaim {
        let doomed: Vec<&Orphan> = report.orphans.iter().filter(|o| o.reclaimable).collect();
        for orphan in &doomed {
            report.reclaimed +=
                sqlx::query("DELETE FROM aoide_images WHERE user_id = ? AND image_hash = ?")
                    .bind(user_id)
                    .bind(&orphan.hash)
                    .execute(pool)
                    .await?
                    .rows_affected() as i64;
            report.reclaimed_bytes += orphan.size;
        }
        report.orphan_bytes -= report.reclaimed_bytes;
        report.orphans.retain(|o| !o.reclaimable);
    }
    Ok(report)
}

/// A playlist shared with someone, as either side sees it.
#[derive(Debug, sqlx::FromRow)]
pub struct Share {
    /// The shared playlist's id.
    pub playlist_id: String,
    /// The user who owns it.
    pub owner_user_id: i64,
    /// The owner's username.
    pub owner_name: String,
    /// The user it's shared with.
    pub grantee_user_id: i64,
    /// The grantee's username.
    pub grantee_name: String,
    /// Whether the grantee may edit it, not only read it.
    pub can_edit: bool,
    /// When it was shared, in milliseconds since the epoch.
    pub created_at: i64,
}

/// Shares the user owns or was granted, newest first.
pub async fn shares(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Share>> {
    sqlx::query_as(
        "SELECT s.playlist_id, o.owner_user_id, ou.username AS owner_name, s.grantee_user_id,
             gu.username AS grantee_name, s.can_edit, s.created_at
         FROM aoide_playlist_shares s
         JOIN aoide_playlist_owners o ON o.playlist_id = s.playlist_id
         JOIN users ou ON ou.id = o.owner_user_id
         JOIN users gu ON gu.id = s.grantee_user_id
         WHERE o.owner_user_id = ?1 OR s.grantee_user_id = ?1
         ORDER BY s.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// The user who owns a playlist; none when nobody has claimed it yet.
pub async fn owner(pool: &SqlitePool, playlist_id: &str) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT owner_user_id FROM aoide_playlist_owners WHERE playlist_id = ?")
        .bind(playlist_id)
        .fetch_optional(pool)
        .await
}

/// Shares a playlist (or changes whether the grantee may edit it).
pub async fn share(
    pool: &SqlitePool,
    playlist_id: &str,
    grantee: i64,
    can_edit: bool,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO aoide_playlist_shares (playlist_id, grantee_user_id, can_edit, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (playlist_id, grantee_user_id) DO UPDATE SET can_edit = excluded.can_edit",
    )
    .bind(playlist_id)
    .bind(grantee)
    .bind(can_edit)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Ends a share; false when there was none.
pub async fn revoke(pool: &SqlitePool, playlist_id: &str, grantee: i64) -> sqlx::Result<bool> {
    let removed = sqlx::query(
        "DELETE FROM aoide_playlist_shares WHERE playlist_id = ? AND grantee_user_id = ?",
    )
    .bind(playlist_id)
    .bind(grantee)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(removed > 0)
}

/// A device's current queue: its latest `queue_state` row.
#[derive(Debug, sqlx::FromRow)]
pub struct Queue {
    /// The queue's id, one per device.
    pub entity_id: String,
    /// The device that pushed it.
    pub device_id: String,
    /// The queue as the device sent it, unread here.
    pub payload: String,
    /// When the device made it, in milliseconds since the epoch.
    pub created_at: i64,
    /// When Melete received it, in milliseconds since the epoch.
    pub received_at: i64,
}

/// Each device's current queue, most recently pushed first.
pub async fn queues(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Queue>> {
    sqlx::query_as(
        "SELECT q.entity_id, q.device_id, q.payload, q.created_at, q.received_at FROM aoide_ops q
         WHERE q.user_id = ?1 AND q.entity = 'queue_state'
           AND q.seq = (SELECT max(seq) FROM aoide_ops
                        WHERE user_id = ?1 AND entity = 'queue_state' AND entity_id = q.entity_id)
         ORDER BY q.seq DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aoide::sync;

    async fn pool() -> SqlitePool {
        let pool = crate::db::open_in_memory().await;
        crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap();
        pool
    }

    async fn push_playlist(
        pool: &SqlitePool,
        op_id: &str,
        id: &str,
        payload: &str,
        operation: &str,
    ) {
        let json = format!(
            r#"{{"opId":"{op_id}","entity":"playlists","entityId":"{id}","operation":"{operation}","payload":{payload},"createdAt":1}}"#
        );
        let op = sync::validate(&serde_json::value::RawValue::from_string(json).unwrap()).unwrap();
        sync::push(pool, 1, "d", vec![op], 1).await.unwrap();
    }

    #[test]
    fn hashes() {
        let hash = sha256_hex(b"abc");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(is_hash(&hash));
        assert!(!is_hash(&hash.to_uppercase()));
        assert!(!is_hash("abc"));
    }

    #[tokio::test]
    async fn only_unnamed_images_past_their_grace_are_reclaimed() {
        let pool = pool().await;
        let day = DAY_MS;
        let [kept, renamed, deleted, young] =
            ["k", "r", "d", "y"].map(|b| sha256_hex(b.as_bytes()));
        for (hash, at) in [(&kept, 0), (&renamed, 0), (&deleted, 0), (&young, 50 * day)] {
            store_image(&pool, 1, hash, "image/png", b"x", at)
                .await
                .unwrap();
        }
        // p1 names `kept`; p2 named `renamed`, then a newer row moved it on; p3 was deleted.
        push_playlist(
            &pool,
            "a",
            "p1",
            &format!(r#"{{"image_hash":"{kept}","updated_at":5}}"#),
            "upsert",
        )
        .await;
        push_playlist(
            &pool,
            "b",
            "p2",
            &format!(r#"{{"imageHash":"{renamed}","updated_at":5}}"#),
            "upsert",
        )
        .await;
        push_playlist(
            &pool,
            "c",
            "p2",
            r#"{"image_hash":null,"updated_at":9}"#,
            "upsert",
        )
        .await;
        // An older row arriving later doesn't win.
        push_playlist(
            &pool,
            "c2",
            "p2",
            &format!(r#"{{"image_hash":"{renamed}","updated_at":7}}"#),
            "upsert",
        )
        .await;
        push_playlist(
            &pool,
            "d",
            "p3",
            &format!(r#"{{"image_hash":"{deleted}","updated_at":5}}"#),
            "upsert",
        )
        .await;
        push_playlist(
            &pool,
            "e",
            "p3",
            &format!(r#"{{"image_hash":"{deleted}","deleted":1,"updated_at":6}}"#),
            "upsert",
        )
        .await;
        let mut named = referenced_images(&pool, 1).await.unwrap();
        named.sort();
        assert_eq!(named, std::slice::from_ref(&kept));

        let now = 60 * day;
        let report = orphans(&pool, 1, 1, false, now).await.unwrap();
        assert_eq!(report.grace_days, 30, "never shorter than the default");
        assert_eq!((report.total_blobs, report.orphans.len()), (4, 3));
        assert!(
            report
                .orphans
                .iter()
                .find(|o| o.hash == young)
                .is_some_and(|o| !o.reclaimable)
        );
        let done = orphans(&pool, 1, 30, true, now).await.unwrap();
        assert_eq!(
            (done.reclaimed, done.reclaimed_bytes, done.orphans.len()),
            (2, 2, 1)
        );
        assert!(image(&pool, 1, &kept).await.unwrap().is_some());
        assert!(image(&pool, 1, &renamed).await.unwrap().is_none());
        assert!(image(&pool, 1, &young).await.unwrap().is_some());
    }
}
