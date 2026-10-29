//! What a listener has heard and flagged, read out of their sync log the way
//! a client would, after the fact: `play_events` (`jellyfinId`, `startedAt`, `endedAt`,
//! `completed`, `skipped`) and `track_flags` (`jellyfinId`, `notInterested`, `deleted`), in
//! either naming convention. The sync path itself stays opaque; an op that can't be read here
//! is skipped, not an error. Listens are read into the play history, and read from
//! there.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::SqlitePool;

use super::taste::PlayEvent;

/// A track id as compared here: a Melete id in its plain form, anything else (a Jellyfin id
/// the import couldn't map) trimmed and lowercased.
pub fn normalise(id: &str) -> String {
    let id = id.trim();
    match id.parse::<i64>() {
        Ok(number) => number.to_string(),
        Err(_) => id.to_lowercase(),
    }
}

fn field<'a>(row: &'a Value, camel: &str, snake: &str) -> Option<&'a Value> {
    row.get(camel).or_else(|| row.get(snake))
}

pub(crate) fn text(row: &Value, camel: &str, snake: &str) -> Option<String> {
    field(row, camel, snake)?.as_str().map(str::to_owned)
}

pub(crate) fn number(row: &Value, camel: &str, snake: &str) -> Option<i64> {
    let value = field(row, camel, snake)?;
    value
        .as_i64()
        .or_else(|| value.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
        .or_else(|| value.as_str()?.parse().ok())
}

/// A boolean however a client wrote it: true, 1 or "1". Absent is false.
pub(crate) fn flag(row: &Value, camel: &str, snake: &str) -> bool {
    match field(row, camel, snake) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => matches!(s.as_str(), "1" | "true" | "True"),
        _ => false,
    }
}

/// Every listen held for a user, from every device and app, in the order each was first heard
/// of: the play history (`crate::plays`), with the sync log's newest play events read
/// into it first. A re-pushed event is the same listen, changed in place.
pub async fn play_events(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<PlayEvent>> {
    crate::plays::catch_up(pool).await?;
    let rows: Vec<(String, i64, Option<i64>, bool, bool)> = sqlx::query_as(
        "SELECT track_key, started_at, ended_at, completed, skipped FROM music_plays
         WHERE user_id = ? ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(track, started_at, ended_at, completed, skipped)| PlayEvent {
                track,
                started_at,
                ended_at,
                completed,
                skipped,
            },
        )
        .collect())
}

/// The tracks a user has flagged not interested: the latest op per track decides, so a flag
/// set and later cleared is no flag.
pub async fn not_interested(pool: &SqlitePool, user_id: i64) -> sqlx::Result<HashSet<String>> {
    let payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload FROM aoide_ops WHERE user_id = ? AND entity = 'track_flags' ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let mut latest: HashMap<String, bool> = HashMap::new();
    for payload in payloads {
        let Ok(row) = serde_json::from_str::<Value>(&payload) else {
            continue;
        };
        let Some(track) = text(&row, "jellyfinId", "jellyfin_id").filter(|_| row.is_object())
        else {
            continue;
        };
        let flagged =
            flag(&row, "notInterested", "not_interested") && !flag(&row, "deleted", "deleted");
        latest.insert(normalise(&track), flagged);
    }
    Ok(latest
        .into_iter()
        .filter(|(_, f)| *f)
        .map(|(t, _)| t)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_listens_and_flags_however_the_clients_wrote_them() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let ops = [
            (
                "e1",
                "play_events",
                r#"{"jellyfinId":"7","startedAt":1000,"completed":true}"#,
            ),
            (
                "e2",
                "play_events",
                r#"{"jellyfin_id":" 0008 ","started_at":"2000","skipped":1}"#,
            ),
            // Re-pushed: the same listen, its latest form.
            (
                "e1",
                "play_events",
                r#"{"jellyfinId":"7","startedAt":1000,"completed":true,"endedAt":1500.0}"#,
            ),
            ("e3", "play_events", r#"{"jellyfinId":"9"}"#),
            ("e4", "play_events", "[1, 2]"),
            (
                "f1",
                "track_flags",
                r#"{"jellyfinId":"7","notInterested":true}"#,
            ),
            (
                "f2",
                "track_flags",
                r#"{"jellyfinId":"ABCDEF","not_interested":"1"}"#,
            ),
            (
                "f3",
                "track_flags",
                r#"{"jellyfinId":"8","notInterested":true}"#,
            ),
            (
                "f3b",
                "track_flags",
                r#"{"jellyfinId":"8","notInterested":true,"deleted":true}"#,
            ),
        ];
        for (seq, (entity_id, entity, payload)) in ops.iter().enumerate() {
            sqlx::query(
                "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                     payload, created_at, received_at)
                 VALUES (?, ?, 'd', ?, ?, 'upsert', ?, 0, 0)",
            )
            .bind(format!("op{seq}"))
            .bind(user.id)
            .bind(entity)
            .bind(entity_id)
            .bind(payload)
            .execute(&pool)
            .await
            .unwrap();
        }
        let events = play_events(&pool, user.id).await.unwrap();
        assert_eq!(
            events,
            [
                PlayEvent {
                    track: "7".into(),
                    started_at: 1000,
                    ended_at: Some(1500),
                    completed: true,
                    skipped: false
                },
                PlayEvent {
                    track: "8".into(),
                    started_at: 2000,
                    ended_at: None,
                    completed: false,
                    skipped: true
                },
            ]
        );
        let flags = not_interested(&pool, user.id).await.unwrap();
        assert_eq!(flags, HashSet::from(["7".to_owned(), "abcdef".to_owned()]));
    }
}
