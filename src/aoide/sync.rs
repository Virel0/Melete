//! The sync store Aoide's devices share, rewritten from the aoide-sidecar
//! plugin against its contract (docs/client-integration.md there): an append-only log of ops
//! per user, pushed and pulled by cursor. Payloads are stored exactly as sent and never read
//! here; playlists and their items are routed between an owner and the users it's shared with.
//! Times are milliseconds since the epoch, as the contract has them.

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use sqlx::SqlitePool;

/// The entities push accepts, sorted (as `acceptedEntities` lists them).
pub const ENTITIES: [&str; 7] = [
    "folders",
    "likes",
    "play_events",
    "playlist_items",
    "playlists",
    "queue_state",
    "track_flags",
];
/// The most ops one push may carry.
pub const MAX_OPS_PER_PUSH: usize = 1000;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_PAYLOAD_DEPTH: usize = 32;
const MAX_OP_ID: usize = 128;
const MAX_ENTITY_ID: usize = 256;
/// How many ops a pull returns when the device doesn't say.
pub const DEFAULT_PULL: i64 = 500;
/// The most ops one pull returns.
pub const MAX_PULL: i64 = 1000;
/// Play events older than this, which every active device has pulled, may be pruned.
pub const RETENTION_DAYS: i64 = 90;
const DAY_MS: i64 = 86_400_000;

/// An op that passed validation, its payload still as sent.
#[derive(Debug, Clone)]
pub struct Op {
    /// The device's own id for the op, unique per user; a repeat is ignored.
    pub op_id: String,
    /// Which of [`ENTITIES`] it changes.
    pub entity: String,
    /// The id of the row it changes.
    pub entity_id: String,
    /// `upsert` or `delete`.
    pub operation: String,
    /// The full row after the change, as the device sent it.
    pub payload: String,
    /// When the device made it, in milliseconds since the epoch.
    pub created_at: i64,
    /// The playlist it belongs to, for routing to whoever it's shared with; none for ops on
    /// anything else.
    pub playlist_id: Option<String>,
}

#[derive(Deserialize)]
struct RawPayload<'a> {
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
}

/// How deep a JSON value nests: an object or array is one level more than its deepest child.
fn depth(value: &Value) -> usize {
    match value {
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Checks one op against the contract's rules; on refusal, its id (when it has one) and why.
pub fn validate(raw: &RawValue) -> Result<Op, (Option<String>, String)> {
    let value: Value = serde_json::from_str(raw.get()).map_err(|err| (None, err.to_string()))?;
    let Value::Object(op) = &value else {
        let reason = if value.is_null() {
            "Op was null."
        } else {
            "An op must be a JSON object."
        };
        return Err((None, reason.into()));
    };
    let text = |name: &str| op.get(name).and_then(Value::as_str).map(str::to_owned);
    let op_id = text("opId");
    let fail = |reason: String| Err((op_id.clone(), reason));
    let id = match &op_id {
        Some(id) if !id.trim().is_empty() && id.chars().count() <= MAX_OP_ID => id.clone(),
        _ => {
            return fail(format!(
                "opId must be a non-empty string of at most {MAX_OP_ID} characters."
            ));
        }
    };
    let entity = text("entity").unwrap_or_default();
    if entity == "tracks" {
        return fail(
            "The tracks table is a per-device cache and is never synced. Rebuild it from this device's own library connection."
                .into(),
        );
    }
    if !ENTITIES.contains(&entity.as_str()) {
        return fail(format!("Unknown entity '{entity}'."));
    }
    let operation = text("operation").unwrap_or_default();
    if operation != "upsert" && operation != "delete" {
        return fail(format!(
            "Unknown operation '{operation}'; expected 'upsert' or 'delete'."
        ));
    }
    let entity_id = match text("entityId") {
        Some(e) if !e.trim().is_empty() && e.chars().count() <= MAX_ENTITY_ID => e,
        _ => {
            return fail(format!(
                "entityId must be a non-empty string of at most {MAX_ENTITY_ID} characters."
            ));
        }
    };
    let payload = match op.get("payload") {
        Some(payload @ Value::Object(_)) => payload,
        _ => {
            return fail(
                "payload must be a JSON object holding the full row after the change.".into(),
            );
        }
    };
    // A stored payload goes to every device on pull, and a decoder refuses an over-nested
    // document whole: one such row would stall every device. It can only be stopped here.
    let nesting = depth(payload);
    if nesting > MAX_PAYLOAD_DEPTH {
        return fail(format!(
            "payload nests {nesting} levels deep, over the {MAX_PAYLOAD_DEPTH} level limit."
        ));
    }
    let raw_payload = serde_json::from_str::<RawPayload>(raw.get())
        .ok()
        .and_then(|r| r.payload)
        .map(|p| p.get().to_owned())
        .unwrap_or_else(|| payload.to_string());
    if raw_payload.len() > MAX_PAYLOAD_BYTES {
        return fail(format!(
            "payload is {} bytes, over the {MAX_PAYLOAD_BYTES} byte limit.",
            raw_payload.len()
        ));
    }
    let created_at = match op.get("createdAt").and_then(Value::as_i64) {
        Some(t) if t > 0 => t,
        _ => return fail("createdAt must be a positive millisecond timestamp.".into()),
    };
    let playlist_id = playlist_id_of(&entity, &entity_id, payload);
    Ok(Op {
        op_id: id,
        entity,
        entity_id,
        operation,
        payload: raw_payload,
        created_at,
        playlist_id,
    })
}

/// The playlist an op belongs to: a playlist's own id, or an item's `playlist_id`
/// (`playlistId` in the other naming convention).
fn playlist_id_of(entity: &str, entity_id: &str, payload: &Value) -> Option<String> {
    match entity {
        "playlists" => Some(entity_id.to_owned()),
        "playlist_items" => ["playlist_id", "playlistId"]
            .iter()
            .find_map(|name| payload.get(*name).and_then(Value::as_str))
            .map(str::to_owned),
        _ => None,
    }
}

/// What a push stored: ops refused because their playlist isn't the user's to edit, and the
/// user's head sequence.
pub struct Pushed {
    /// `(op id, why)` for each op refused because its playlist isn't the user's to edit.
    pub denied: Vec<(String, String)>,
    /// The user's head sequence after the push.
    pub cursor: i64,
}

/// Appends valid ops. Re-pushed ops (same op id) are accepted and ignored; ops on a playlist
/// owned by someone else, not shared with this user for editing, are refused; playlists
/// nobody owns yet become this user's. A device's queue state keeps only its latest row.
pub async fn push(
    pool: &SqlitePool,
    user_id: i64,
    device_id: &str,
    ops: Vec<Op>,
    now: i64,
) -> sqlx::Result<Pushed> {
    let mut tx = pool.begin().await?;
    let mut denied = Vec::new();
    let mut kept = Vec::with_capacity(ops.len());
    for op in ops {
        if let Some(playlist) = &op.playlist_id {
            let forbidden: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM aoide_playlist_owners o
                     WHERE o.playlist_id = ?1 AND o.owner_user_id <> ?2
                       AND NOT EXISTS (SELECT 1 FROM aoide_playlist_shares s
                           WHERE s.playlist_id = ?1 AND s.grantee_user_id = ?2 AND s.can_edit = 1))",
            )
            .bind(playlist)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
            if forbidden {
                denied.push((
                    op.op_id.clone(),
                    format!(
                        "Playlist '{playlist}' belongs to another user and is not shared with you for editing."
                    ),
                ));
                continue;
            }
            sqlx::query(
                "INSERT INTO aoide_playlist_owners (playlist_id, owner_user_id, created_at)
                 VALUES (?, ?, ?) ON CONFLICT (playlist_id) DO NOTHING",
            )
            .bind(playlist)
            .bind(user_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        kept.push(op);
    }
    for op in &kept {
        sqlx::query(
            "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation, payload,
                 created_at, received_at, playlist_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id, op_id) DO NOTHING",
        )
        .bind(&op.op_id)
        .bind(user_id)
        .bind(device_id)
        .bind(&op.entity)
        .bind(&op.entity_id)
        .bind(&op.operation)
        .bind(&op.payload)
        .bind(op.created_at)
        .bind(now)
        .bind(&op.playlist_id)
        .execute(&mut *tx)
        .await?;
    }
    // A queue state is one row per device, replaced whole whenever playback moves; older
    // rows are overwritten values, not history.
    let mut queues: Vec<&str> = kept
        .iter()
        .filter(|op| op.entity == "queue_state")
        .map(|op| op.entity_id.as_str())
        .collect();
    queues.sort_unstable();
    queues.dedup();
    for entity_id in queues {
        sqlx::query(
            "DELETE FROM aoide_ops WHERE user_id = ?1 AND entity = 'queue_state' AND entity_id = ?2
               AND seq < (SELECT max(seq) FROM aoide_ops
                          WHERE user_id = ?1 AND entity = 'queue_state' AND entity_id = ?2)",
        )
        .bind(user_id)
        .bind(entity_id)
        .execute(&mut *tx)
        .await?;
    }
    let cursor = head_in(&mut tx, user_id).await?;
    tx.commit().await?;
    Ok(Pushed { denied, cursor })
}

async fn head_in(conn: &mut sqlx::SqliteConnection, user_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT coalesce(max(seq), 0) FROM aoide_ops WHERE user_id = ?")
        .bind(user_id)
        .fetch_one(conn)
        .await
}

/// The user's head sequence: the highest of their own ops.
pub async fn head(pool: &SqlitePool, user_id: i64) -> sqlx::Result<i64> {
    let mut conn = pool.acquire().await?;
    head_in(&mut conn, user_id).await
}

/// An op as pulled, with what the server added.
#[derive(Debug, sqlx::FromRow)]
pub struct Pulled {
    /// Its place in the log, which a device's cursor counts.
    pub seq: i64,
    /// The device's own id for the op.
    pub op_id: String,
    /// The device that pushed it.
    pub device_id: String,
    /// Which of [`ENTITIES`] it changes.
    pub entity: String,
    /// The id of the row it changes.
    pub entity_id: String,
    /// `upsert` or `delete`.
    pub operation: String,
    /// The full row after the change, as the device sent it.
    pub payload: String,
    /// When the device made it, in milliseconds since the epoch.
    pub created_at: i64,
    /// When Melete received it, in milliseconds since the epoch.
    pub received_at: i64,
    /// The user who pushed it: the puller, or someone else editing a playlist the puller owns or
    /// was shared.
    pub user_id: i64,
}

/// Ops past `since`, in sequence order: the user's own and those on playlists they own or
/// that are shared with them. Reads one past the limit so `has_more` is exact.
pub async fn pull(
    pool: &SqlitePool,
    user_id: i64,
    since: i64,
    limit: i64,
) -> sqlx::Result<(Vec<Pulled>, i64, bool)> {
    let mut ops: Vec<Pulled> = sqlx::query_as(
        "SELECT seq, op_id, device_id, entity, entity_id, operation, payload, created_at,
             received_at, user_id
         FROM aoide_ops
         WHERE seq > ?1 AND (
                 user_id = ?2
              OR playlist_id IN (SELECT playlist_id FROM aoide_playlist_owners WHERE owner_user_id = ?2)
              OR playlist_id IN (SELECT playlist_id FROM aoide_playlist_shares WHERE grantee_user_id = ?2))
         ORDER BY seq LIMIT ?3",
    )
    .bind(since)
    .bind(user_id)
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;
    let has_more = ops.len() as i64 > limit;
    ops.truncate(usize::try_from(limit).unwrap_or(0));
    let cursor = ops.last().map_or(since, |op| op.seq);
    Ok((ops, cursor, has_more))
}

/// Notes how far a device has pulled; only ever forward (re-pulling to replay isn't having
/// seen less).
pub async fn record_cursor(
    pool: &SqlitePool,
    user_id: i64,
    device_id: &str,
    cursor: i64,
    now: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO aoide_device_cursors (user_id, device_id, cursor, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (user_id, device_id) DO UPDATE SET
             cursor = max(cursor, excluded.cursor), updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(device_id)
    .bind(cursor)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// One device's place in the log, for retention.
#[derive(Debug, PartialEq)]
pub struct DeviceCursor {
    /// The device's id.
    pub device_id: String,
    /// How far it has pulled: the highest sequence it has seen.
    pub cursor: i64,
    /// Whole days since it last pulled.
    pub last_seen_days: i64,
    /// Whether it pulled within the retention window, so its cursor holds pruning back.
    pub active: bool,
}

/// What retention would trim, or did.
#[derive(Debug)]
pub struct Retention {
    /// How many ops the user has, by entity.
    pub ops_by_entity: Vec<(String, i64)>,
    /// The user's head sequence.
    pub cursor: i64,
    /// The devices that have pulled, most recently seen first.
    pub devices: Vec<DeviceCursor>,
    /// The lowest cursor of the active devices: nothing past it is pruned. 0 when no device is
    /// active, and then nothing is.
    pub safe_cursor: i64,
    /// The window used, in days: never shorter than [`RETENTION_DAYS`].
    pub retention_days: i64,
    /// Play events that could be pruned (after a prune, those left).
    pub prunable_play_events: i64,
    /// Play events deleted; 0 without `prune`.
    pub pruned: i64,
}

/// Surveys, and with `prune` deletes, play events older than `days` that every device seen in
/// that time has already pulled. Only play events (independent facts; everything else is
/// current state), and never the user's newest op, so their head never moves backwards.
/// `days` is never shorter than [`RETENTION_DAYS`].
pub async fn retention(
    pool: &SqlitePool,
    user_id: i64,
    days: i64,
    prune: bool,
    now: i64,
) -> sqlx::Result<Retention> {
    let days = days.max(RETENTION_DAYS);
    if prune {
        // The play history keeps what's pruned here: read it in before it goes.
        crate::plays::catch_up(pool).await?;
    }
    let ops_by_entity: Vec<(String, i64)> = sqlx::query_as(
        "SELECT entity, count(*) FROM aoide_ops WHERE user_id = ? GROUP BY entity ORDER BY entity",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let cursors: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT device_id, cursor, updated_at FROM aoide_device_cursors
         WHERE user_id = ? ORDER BY updated_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let devices: Vec<DeviceCursor> = cursors
        .into_iter()
        .map(|(device_id, cursor, updated_at)| {
            let last_seen_days = ((now - updated_at) / DAY_MS).max(0);
            DeviceCursor {
                device_id,
                cursor,
                last_seen_days,
                active: last_seen_days <= days,
            }
        })
        .collect();
    // No device has said how far it read: nothing is known to be safe.
    let safe_cursor = devices
        .iter()
        .filter(|d| d.active)
        .map(|d| d.cursor)
        .min()
        .unwrap_or(0);
    let before = now - days * DAY_MS;
    let matching = "FROM aoide_ops
         WHERE user_id = ?1 AND entity = 'play_events' AND seq <= ?2 AND received_at < ?3
           AND seq < (SELECT max(seq) FROM aoide_ops WHERE user_id = ?1)";
    let mut prunable: i64 = if safe_cursor == 0 {
        0
    } else {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) {matching}")))
            .bind(user_id)
            .bind(safe_cursor)
            .bind(before)
            .fetch_one(pool)
            .await?
    };
    let mut pruned = 0;
    if prune && prunable > 0 {
        pruned = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE {matching}")))
            .bind(user_id)
            .bind(safe_cursor)
            .bind(before)
            .execute(pool)
            .await?
            .rows_affected() as i64;
        prunable -= pruned;
        tracing::info!(user_id, pruned, days, "pruned old play events");
    }
    Ok(Retention {
        ops_by_entity,
        cursor: head(pool, user_id).await?,
        devices,
        safe_cursor,
        retention_days: days,
        prunable_play_events: prunable,
        pruned,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(json: &str) -> Box<RawValue> {
        RawValue::from_string(json.to_owned()).unwrap()
    }

    fn op(id: &str, entity: &str, entity_id: &str, payload: &str) -> Op {
        let json = format!(
            r#"{{"opId":"{id}","entity":"{entity}","entityId":"{entity_id}","operation":"upsert","payload":{payload},"createdAt":1}}"#
        );
        validate(&raw(&json)).unwrap()
    }

    async fn users() -> SqlitePool {
        let pool = crate::db::open_in_memory().await;
        for name in ["owner", "friend", "stranger"] {
            crate::auth::create_user(&pool, name, "a-password".into(), false)
                .await
                .unwrap();
        }
        pool
    }

    #[test]
    fn validates_each_rule_and_keeps_the_payload_as_sent() {
        let good = op(
            "o1",
            "playlists",
            "p1",
            r#"{"name":"Late Night","n":12345678901234567890}"#,
        );
        assert_eq!(
            good.payload,
            r#"{"name":"Late Night","n":12345678901234567890}"#
        );
        assert_eq!(good.playlist_id.as_deref(), Some("p1"));
        let item = op("o2", "playlist_items", "i1", r#"{"playlistId":"p1"}"#);
        assert_eq!(item.playlist_id.as_deref(), Some("p1"));

        let deep = format!("{}1{}", r#"{"a":"#.repeat(33), "}".repeat(33));
        let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(MAX_PAYLOAD_BYTES));
        for (json, reason) in [
            ("null", "Op was null."),
            (r#"{"entity":"likes"}"#, "opId must be"),
            (r#"{"opId":"a","entity":"tracks"}"#, "per-device cache"),
            (r#"{"opId":"a","entity":"songs"}"#, "Unknown entity 'songs'"),
            (
                r#"{"opId":"a","entity":"likes","operation":"UPSERT"}"#,
                "Unknown operation 'UPSERT'",
            ),
            (
                r#"{"opId":"a","entity":"likes","operation":"upsert","entityId":""}"#,
                "entityId must be",
            ),
            (
                r#"{"opId":"a","entity":"likes","operation":"upsert","entityId":"e","payload":[1]}"#,
                "payload must be a JSON object",
            ),
            (
                &format!(
                    r#"{{"opId":"a","entity":"likes","operation":"upsert","entityId":"e","payload":{deep},"createdAt":1}}"#
                ),
                "nests 33 levels",
            ),
            (
                &format!(
                    r#"{{"opId":"a","entity":"likes","operation":"upsert","entityId":"e","payload":{big},"createdAt":1}}"#
                ),
                "byte limit",
            ),
            (
                r#"{"opId":"a","entity":"likes","operation":"upsert","entityId":"e","payload":{},"createdAt":0}"#,
                "createdAt must be",
            ),
        ] {
            let (_, why) = validate(&raw(json)).unwrap_err();
            assert!(why.contains(reason), "{json}: {why}");
        }
        let (id, _) = validate(&raw(r#"{"opId":"keep-me","entity":"songs"}"#)).unwrap_err();
        assert_eq!(id.as_deref(), Some("keep-me"), "a refusal names the op");
        // 32 levels is allowed.
        let ok = format!("{}1{}", r#"{"a":"#.repeat(31), "}".repeat(31));
        op("o3", "likes", "e", &ok);
    }

    #[tokio::test]
    async fn pushes_are_idempotent_and_pulls_page_exactly() {
        let pool = users().await;
        let ops = vec![
            op("a", "likes", "t1", "{}"),
            op("b", "likes", "t2", "{}"),
            op("c", "play_events", "e1", "{}"),
        ];
        let first = push(&pool, 1, "phone", ops.clone(), 10).await.unwrap();
        let again = push(&pool, 1, "phone", ops, 20).await.unwrap();
        assert_eq!(
            (first.cursor, again.cursor),
            (3, 3),
            "re-pushing adds nothing"
        );
        let (page, cursor, more) = pull(&pool, 1, 0, 2).await.unwrap();
        assert_eq!((page.len(), cursor, more), (2, 2, true));
        let (page, cursor, more) = pull(&pool, 1, cursor, 2).await.unwrap();
        assert_eq!((page.len(), cursor, more), (1, 3, false));
        assert_eq!(page[0].received_at, 10, "received when first pushed");
        let (none, cursor, more) = pull(&pool, 1, 3, 2).await.unwrap();
        assert_eq!((none.len(), cursor, more), (0, 3, false));
        // Another user sees none of it.
        assert!(pull(&pool, 2, 0, 10).await.unwrap().0.is_empty());
    }

    #[tokio::test]
    async fn shared_playlists_reach_collaborators_and_others_are_refused() {
        let pool = users().await;
        push(&pool, 1, "phone", vec![op("p", "playlists", "p1", "{}")], 1)
            .await
            .unwrap();
        // The stranger can't edit it.
        let refused = push(
            &pool,
            3,
            "x",
            vec![op("s", "playlist_items", "i1", r#"{"playlist_id":"p1"}"#)],
            2,
        )
        .await
        .unwrap();
        assert_eq!(refused.denied.len(), 1);
        assert!(refused.denied[0].1.contains("belongs to another user"));
        // Shared for editing, the friend can, and the owner gets the edit.
        sqlx::query("INSERT INTO aoide_playlist_shares (playlist_id, grantee_user_id, created_at) VALUES ('p1', 2, 0)")
            .execute(&pool)
            .await
            .unwrap();
        let edit = push(
            &pool,
            2,
            "tablet",
            vec![op("f", "playlist_items", "i2", r#"{"playlist_id":"p1"}"#)],
            3,
        )
        .await
        .unwrap();
        assert!(edit.denied.is_empty());
        let owner: Vec<(String, i64)> = pull(&pool, 1, 0, 10)
            .await
            .unwrap()
            .0
            .into_iter()
            .map(|o| (o.op_id, o.user_id))
            .collect();
        assert_eq!(owner, [("p".into(), 1), ("f".into(), 2)]);
        let friend = pull(&pool, 2, 0, 10).await.unwrap().0;
        assert_eq!(
            friend.len(),
            2,
            "the friend sees the playlist and its items"
        );
        assert!(pull(&pool, 3, 0, 10).await.unwrap().0.is_empty());
        // Read-only sharing doesn't let them write.
        sqlx::query("UPDATE aoide_playlist_shares SET can_edit = 0")
            .execute(&pool)
            .await
            .unwrap();
        let read_only = push(
            &pool,
            2,
            "tablet",
            vec![op("g", "playlists", "p1", "{}")],
            4,
        )
        .await
        .unwrap();
        assert_eq!(read_only.denied.len(), 1);
    }

    #[tokio::test]
    async fn queue_state_keeps_one_row_per_device() {
        let pool = users().await;
        for (i, id) in ["q1", "q2", "q3"].iter().enumerate() {
            push(
                &pool,
                1,
                "phone",
                vec![op(id, "queue_state", "phone", &format!(r#"{{"at":{i}}}"#))],
                1,
            )
            .await
            .unwrap();
        }
        push(
            &pool,
            1,
            "mac",
            vec![op("m1", "queue_state", "mac", "{}")],
            1,
        )
        .await
        .unwrap();
        let ops: Vec<String> = pull(&pool, 1, 0, 10)
            .await
            .unwrap()
            .0
            .into_iter()
            .map(|o| o.op_id)
            .collect();
        assert_eq!(ops, ["q3", "m1"]);
    }

    #[tokio::test]
    async fn retention_prunes_only_old_play_events_every_device_has_read() {
        let pool = users().await;
        let old = 1_000;
        push(
            &pool,
            1,
            "phone",
            vec![
                op("e1", "play_events", "e1", "{}"),
                op("l1", "likes", "t", "{}"),
            ],
            old,
        )
        .await
        .unwrap();
        push(
            &pool,
            1,
            "phone",
            vec![op("e2", "play_events", "e2", "{}")],
            old,
        )
        .await
        .unwrap();
        let now = old + 100 * DAY_MS;
        // No device has reported a cursor: nothing is safe.
        assert_eq!(retention(&pool, 1, 90, true, now).await.unwrap().pruned, 0);
        record_cursor(&pool, 1, "token-1", 3, now).await.unwrap();
        record_cursor(&pool, 1, "token-1", 1, now).await.unwrap();
        let survey = retention(&pool, 1, 30, false, now).await.unwrap();
        assert_eq!(survey.retention_days, 90, "never shorter than the default");
        assert_eq!(survey.safe_cursor, 3, "cursors only move forward");
        assert_eq!(survey.prunable_play_events, 1, "the newest op is kept");
        let done = retention(&pool, 1, 90, true, now).await.unwrap();
        assert_eq!(
            (done.pruned, done.prunable_play_events, done.cursor),
            (1, 0, 3)
        );
        let left: Vec<String> = pull(&pool, 1, 0, 10)
            .await
            .unwrap()
            .0
            .into_iter()
            .map(|o| o.op_id)
            .collect();
        assert_eq!(left, ["l1", "e2"]);
        // Devices hold back everything they haven't read.
        push(
            &pool,
            1,
            "phone",
            vec![
                op("e3", "play_events", "e3", "{}"),
                op("e4", "play_events", "e4", "{}"),
            ],
            old,
        )
        .await
        .unwrap();
        record_cursor(&pool, 1, "token-2", 3, now).await.unwrap();
        // e2 is no longer the newest and both devices have read it; e3 and e4 they haven't.
        let survey = retention(&pool, 1, 90, true, now).await.unwrap();
        assert_eq!((survey.safe_cursor, survey.pruned), (3, 1));
        let left: Vec<String> = pull(&pool, 1, 0, 10)
            .await
            .unwrap()
            .0
            .into_iter()
            .map(|o| o.op_id)
            .collect();
        assert_eq!(left, ["l1", "e3", "e4"]);
    }
}
