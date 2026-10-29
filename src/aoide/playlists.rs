//! Aoide's playlists as they stand, for apps that don't sync: the web UI,
//! Mneme, anything reading the API. Worked out from the sync log the way an Aoide device works
//! them out when it applies what it pulls, so every reader sees what the apps show: last
//! writer wins on `updatedAt`, ties broken by `originDevice`; playlists and folders merged
//! field by field on their `fieldUpdatedAt` stamps, items row by row. Nothing here writes: a
//! change to a playlist is an op in the sync log, pushed by a device or written by
//! `playlist_edits` as a device would write it, so the log stays the one source of truth.

use std::collections::HashMap;

use serde_json::{Map, Value};
use sqlx::SqlitePool;

/// A record as an Aoide device holds it: fields by their camelCase names.
pub type Row = Map<String, Value>;

/// Fields that carry the merge rather than being merged: the phone's metadata columns.
const METADATA: [&str; 6] = [
    "deleted",
    "deviceId",
    "fieldUpdatedAt",
    "id",
    "originDevice",
    "updatedAt",
];

/// A record and its per-field stamps.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    row: Row,
    stamps: HashMap<String, f64>,
}

/// `start_time` → `startTime`: older ops in a log may use either naming.
fn camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for c in key.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A number however a device wrote it, as JavaScript's `Number(x ?? 0)` reads it.
fn number(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Whether a record is deleted: 1, true or "1".
pub fn deleted(row: &Row) -> bool {
    match row.get("deleted") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true"),
        _ => false,
    }
}

/// An op's payload as a record, or None when it isn't one; a delete marks it deleted.
fn record(operation: &str, payload: &str) -> Option<(Row, HashMap<String, f64>)> {
    let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(payload) else {
        return None;
    };
    let mut row: Row = raw.into_iter().map(|(k, v)| (camel(&k), v)).collect();
    let stamps = match row.get("fieldUpdatedAt") {
        Some(Value::Object(map)) => map
            .iter()
            .filter_map(|(k, v)| Some((camel(k), v.as_f64().filter(|n| n.is_finite())?)))
            .collect(),
        _ => HashMap::new(),
    };
    row.remove("fieldUpdatedAt");
    if operation == "delete" {
        row.insert("deleted".into(), Value::from(1));
    }
    Some((row, stamps))
}

/// Which of two writes wins, considered whole: the later `updatedAt`, then the greater
/// `originDevice`; the same device at the same moment is the same write, and what's held stays.
fn incoming_wins(incoming: &Row, existing: &Row) -> bool {
    let (a, b) = (
        number(incoming.get("updatedAt")),
        number(existing.get("updatedAt")),
    );
    if a != b {
        return a > b;
    }
    let (a, b) = (
        text(incoming.get("originDevice")),
        text(existing.get("originDevice")),
    );
    a != b && a > b
}

/// The apps' merge: row by row, or field by field on stamps. Field by field, the row that
/// wins as a whole gives the metadata and is the starting point; each field then comes from
/// whichever side changed it later (a field without a stamp counts from its row's
/// `updatedAt`), and when that side's row has no such field, it's cleared; a tie leaves the
/// winner's. Only stamps a write gave are kept, at the later of the two.
fn merge(
    per_field: bool,
    existing: Held,
    incoming: Row,
    incoming_stamps: HashMap<String, f64>,
) -> Held {
    let wins = incoming_wins(&incoming, &existing.row);
    if !per_field {
        return if wins {
            Held {
                row: incoming,
                stamps: HashMap::new(),
            }
        } else {
            existing
        };
    }
    let incoming_at = number(incoming.get("updatedAt"));
    let existing_at = number(existing.row.get("updatedAt"));
    let (mut row, mut stamps) = if wins {
        (incoming.clone(), incoming_stamps.clone())
    } else {
        (existing.row.clone(), existing.stamps.clone())
    };
    let fields: std::collections::BTreeSet<&String> = existing
        .row
        .keys()
        .chain(incoming.keys())
        .filter(|k| !METADATA.contains(&k.as_str()))
        .collect();
    for field in fields {
        let incoming_stamp = incoming_stamps.get(field).copied();
        let existing_stamp = existing.stamps.get(field).copied();
        let incoming_time = incoming_stamp.unwrap_or(incoming_at);
        let existing_time = existing_stamp.unwrap_or(existing_at);
        let source = if incoming_time > existing_time {
            Some(&incoming)
        } else if existing_time > incoming_time {
            Some(&existing.row)
        } else {
            None
        };
        if let Some(source) = source {
            match source.get(field) {
                Some(value) => row.insert(field.clone(), value.clone()),
                None => row.remove(field),
            };
        }
        if incoming_stamp.is_some() || existing_stamp.is_some() {
            stamps.insert(field.clone(), incoming_time.max(existing_time));
        }
    }
    Held { row, stamps }
}

/// Every record of a kind after applying `ops` (operation, payload) in order, by id.
pub fn fold<'a>(
    per_field: bool,
    ops: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HashMap<String, Row> {
    fold_stamped(per_field, ops)
        .into_iter()
        .map(|(id, (row, _))| (id, row))
        .collect()
}

/// [`fold`], each record with when each of its fields last changed (its `fieldUpdatedAt`).
pub fn fold_stamped<'a>(
    per_field: bool,
    ops: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> HashMap<String, (Row, HashMap<String, f64>)> {
    let mut held: HashMap<String, Held> = HashMap::new();
    for (operation, payload) in ops {
        let Some((row, stamps)) = record(operation, payload) else {
            continue;
        };
        let Some(id) = row
            .get("id")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        let next = match held.remove(&id) {
            None => Held { row, stamps },
            Some(existing) => merge(per_field, existing, row, stamps),
        };
        held.insert(id, next);
    }
    held.into_iter()
        .map(|(id, h)| (id, (h.row, h.stamps)))
        .collect()
}

/// A playlist someone can see, as it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct Playlist {
    /// The playlist's id.
    pub id: String,
    /// Its record as it stands, every field merged.
    pub row: Row,
    /// When each of its fields last changed, as its `fieldUpdatedAt` says.
    pub stamps: HashMap<String, f64>,
    /// The user who owns it.
    pub owner_id: i64,
    /// The owner's username.
    pub owner: String,
    /// Whether the viewer may edit it: always for the owner, for others as shared.
    pub can_edit: bool,
    /// Its items as they stand, in order: (item, its record).
    pub items: Vec<Row>,
}

/// The playlists `user_id` owns or that are shared with them, not deleted, newest change first.
pub async fn visible(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Playlist>> {
    let access: Vec<(String, i64, String, bool)> = sqlx::query_as(
        "SELECT o.playlist_id, o.owner_user_id, u.username,
             o.owner_user_id = ?1 OR coalesce(s.can_edit, 0) = 1
         FROM aoide_playlist_owners o JOIN users u ON u.id = o.owner_user_id
         LEFT JOIN aoide_playlist_shares s ON s.playlist_id = o.playlist_id AND s.grantee_user_id = ?1
         WHERE o.owner_user_id = ?1 OR s.grantee_user_id = ?1",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    if access.is_empty() {
        return Ok(Vec::new());
    }
    let ops: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT entity, operation, payload FROM aoide_ops
         WHERE entity IN ('playlists', 'playlist_items') AND playlist_id IN (
             SELECT playlist_id FROM aoide_playlist_owners WHERE owner_user_id = ?1
             UNION SELECT playlist_id FROM aoide_playlist_shares WHERE grantee_user_id = ?1)
         ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let lists = fold_stamped(
        true,
        ops.iter()
            .filter(|(e, ..)| e == "playlists")
            .map(|(_, o, p)| (o.as_str(), p.as_str())),
    );
    let items = fold(
        false,
        ops.iter()
            .filter(|(e, ..)| e == "playlist_items")
            .map(|(_, o, p)| (o.as_str(), p.as_str())),
    );
    let mut by_playlist: HashMap<String, Vec<Row>> = HashMap::new();
    for item in items.into_values().filter(|i| !deleted(i)) {
        if let Some(playlist) = item.get("playlistId").and_then(Value::as_str) {
            by_playlist
                .entry(playlist.to_owned())
                .or_default()
                .push(item);
        }
    }
    let mut out: Vec<Playlist> = access
        .into_iter()
        .filter_map(|(id, owner_id, owner, can_edit)| {
            let (row, stamps) = lists.get(&id)?.clone();
            if deleted(&row) {
                return None;
            }
            let mut items = by_playlist.remove(&id).unwrap_or_default();
            items.sort_by(|a, b| {
                (text(a.get("position")), text(a.get("id")))
                    .cmp(&(text(b.get("position")), text(b.get("id"))))
            });
            Some(Playlist {
                id,
                row,
                stamps,
                owner_id,
                owner,
                can_edit,
                items,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        number(b.row.get("updatedAt"))
            .total_cmp(&number(a.row.get("updatedAt")))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// The names of `user_id`'s folders as they stand, by id.
pub async fn folders(pool: &SqlitePool, user_id: i64) -> sqlx::Result<HashMap<String, String>> {
    let ops: Vec<(String, String)> = sqlx::query_as(
        "SELECT operation, payload FROM aoide_ops WHERE user_id = ? AND entity = 'folders' ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(
        fold(true, ops.iter().map(|(o, p)| (o.as_str(), p.as_str())))
            .into_iter()
            .filter(|(_, row)| !deleted(row))
            .filter_map(|(id, row)| Some((id, row.get("name")?.as_str()?.to_owned())))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folded(per_field: bool, ops: &[(&str, &str)]) -> HashMap<String, Row> {
        fold(per_field, ops.iter().copied())
    }

    fn get<'a>(rows: &'a HashMap<String, Row>, id: &str, field: &str) -> &'a Value {
        &rows[id][field]
    }

    #[test]
    fn edits_to_different_fields_on_two_devices_both_win() {
        // The phone renames at 200; the desktop, whose clock reads 300, writes the notes.
        let rows = folded(
            true,
            &[
                (
                    "upsert",
                    r#"{"id":"p","name":"Old","notes":"","updatedAt":100,"originDevice":"phone"}"#,
                ),
                (
                    "upsert",
                    r#"{"id":"p","name":"New","notes":"","updatedAt":200,"originDevice":"phone","fieldUpdatedAt":{"name":200}}"#,
                ),
                (
                    "upsert",
                    r#"{"id":"p","name":"Old","notes":"For runs","updatedAt":300,"originDevice":"desk","fieldUpdatedAt":{"name":100,"notes":300}}"#,
                ),
            ],
        );
        assert_eq!(
            (get(&rows, "p", "name"), get(&rows, "p", "notes")),
            (&Value::from("New"), &Value::from("For runs"))
        );
        assert_eq!(get(&rows, "p", "updatedAt"), &Value::from(300));
    }

    #[test]
    fn a_field_a_newer_write_leaves_out_is_cleared() {
        let rows = folded(
            true,
            &[
                (
                    "upsert",
                    r#"{"id":"p","name":"Mix","imageHash":"abc","updatedAt":100,"originDevice":"phone"}"#,
                ),
                // No imageHash at all: the apps clear it, as they would a field set to none.
                (
                    "upsert",
                    r#"{"id":"p","name":"Mix 2","updatedAt":200,"originDevice":"old"}"#,
                ),
            ],
        );
        assert_eq!(get(&rows, "p", "name"), &Value::from("Mix 2"));
        assert!(!rows["p"].contains_key("imageHash"));
    }

    #[test]
    fn notes_cleared_on_a_device_are_cleared_here() {
        // A device clears the notes: its row leaves them out and keeps their old stamp.
        let rows = folded(
            true,
            &[
                (
                    "upsert",
                    r#"{"id":"p","name":"Mix","notes":"n","updatedAt":100,"originDevice":"phone","fieldUpdatedAt":{"name":100,"notes":100}}"#,
                ),
                (
                    "upsert",
                    r#"{"id":"p","name":"Mix","updatedAt":200,"originDevice":"phone","fieldUpdatedAt":{"name":100,"notes":100}}"#,
                ),
                // An older write of the notes, arriving late, doesn't bring them back.
                (
                    "upsert",
                    r#"{"id":"p","name":"Mix","notes":"n","updatedAt":150,"originDevice":"desk","fieldUpdatedAt":{"name":100,"notes":100}}"#,
                ),
            ],
        );
        assert!(!rows["p"].contains_key("notes"), "{:?}", rows["p"]);
    }

    #[test]
    fn a_tie_goes_to_the_greater_device_and_a_replay_changes_nothing() {
        let a = r#"{"id":"p","name":"From a","updatedAt":100,"originDevice":"a"}"#;
        let b = r#"{"id":"p","name":"From b","updatedAt":100,"originDevice":"b"}"#;
        for order in [[a, b], [b, a]] {
            let rows = folded(true, &[("upsert", order[0]), ("upsert", order[1])]);
            assert_eq!(
                get(&rows, "p", "name"),
                &Value::from("From b"),
                "either order"
            );
        }
        let once = folded(true, &[("upsert", a)]);
        assert_eq!(folded(true, &[("upsert", a), ("upsert", a)]), once);
    }

    #[test]
    fn items_go_row_by_row_and_a_delete_holds_until_a_later_write() {
        let rows = folded(
            false,
            &[
                (
                    "upsert",
                    r#"{"id":"i","playlistId":"p","position":"a0","updatedAt":100,"originDevice":"x"}"#,
                ),
                (
                    "delete",
                    r#"{"id":"i","playlistId":"p","position":"a0","updatedAt":200,"originDevice":"x"}"#,
                ),
                // An older write arriving late doesn't bring it back.
                (
                    "upsert",
                    r#"{"id":"i","playlistId":"p","position":"a5","updatedAt":150,"originDevice":"y"}"#,
                ),
            ],
        );
        assert!(deleted(&rows["i"]));
        assert_eq!(get(&rows, "i", "position"), &Value::from("a0"));
        // Older logs wrote snake_case; it reads the same.
        let snake = folded(
            false,
            &[(
                "upsert",
                r#"{"id":"j","playlist_id":"p","content_key":"a|b|c|1"}"#,
            )],
        );
        assert_eq!(get(&snake, "j", "contentKey"), &Value::from("a|b|c|1"));
    }
}
