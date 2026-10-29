//! Play history: every listen with how it ended (finished, or skipped and where), the
//! device and the activity tag, for recommendations, the Home view and the apps.
//!
//! Aoide's devices keep their listens as `play_events` in the sync log; those are read into the
//! history after the fact, in log order, the way a client would read them (`listening`'s
//! rules), and kept after the log's retention trims them. Apps that don't sync send listens
//! through the music API. A listen sent again (the same id from the same source) is the same
//! listen, changed; a play event deleted in the log leaves the history too. A user can delete
//! their own listens; each one's id is remembered, so it stays deleted when sent again.

use serde_json::Value;
use sqlx::SqlitePool;

use crate::aoide::listening::{flag, normalise, number, text};

/// The longest activity tag kept, in characters.
pub const MAX_ACTIVITY: usize = 64;
/// Play events read from the log per query.
const BATCH: i64 = 1000;

/// One reader of the log at a time, so two can't apply the same ops.
static READING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Where a listen came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A play event in Aoide's sync log.
    Sync,
    /// Sent through the music API by an app that doesn't sync.
    Api,
}

impl Source {
    /// Its name, as stored: `sync` or `api`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sync => "sync",
            Self::Api => "api",
        }
    }
}

/// One listen, as it's stored. Times in milliseconds since the epoch.
#[derive(Debug, Clone, PartialEq)]
pub struct Listen {
    /// The listen's id at its source; sent again, it changes the same listen.
    pub event_id: String,
    /// The track it was of: a Melete track id, or another id the import couldn't map, lowercased.
    pub track_key: String,
    /// The device it played on.
    pub device: String,
    /// When it started.
    pub started_at: i64,
    /// When it ended; none while playing or unreported.
    pub ended_at: Option<i64>,
    /// Where it stopped in the track, in milliseconds; none when unreported.
    pub position_ms: Option<i64>,
    /// Whether it played to the end.
    pub completed: bool,
    /// Whether it was skipped.
    pub skipped: bool,
    /// The activity tag, trimmed and at most [`MAX_ACTIVITY`] characters.
    pub activity: Option<String>,
}

/// An activity tag as kept: trimmed, at most [`MAX_ACTIVITY`] characters, none when empty.
pub fn activity(tag: Option<&str>) -> Option<String> {
    let tag = tag?.trim();
    (!tag.is_empty()).then(|| tag.chars().take(MAX_ACTIVITY).collect())
}

/// A play event's payload as a listen; none when it can't be one (not an object, or without
/// a track or a start). Fields in either naming convention: `jellyfinId`, `startedAt`,
/// `endedAt`, `positionMs`, `completed`, `skipped`, `activity`.
pub fn from_payload(event_id: &str, device: &str, payload: &str) -> Option<Listen> {
    let row = serde_json::from_str::<Value>(payload).ok()?;
    if !row.is_object() {
        return None;
    }
    let track = text(&row, "jellyfinId", "jellyfin_id")?;
    let started_at = number(&row, "startedAt", "started_at")?;
    Some(Listen {
        event_id: event_id.to_owned(),
        track_key: normalise(&track),
        device: device.to_owned(),
        started_at,
        ended_at: number(&row, "endedAt", "ended_at"),
        position_ms: number(&row, "positionMs", "position_ms").filter(|p| *p >= 0),
        completed: flag(&row, "completed", "completed"),
        skipped: flag(&row, "skipped", "skipped"),
        activity: activity(text(&row, "activity", "activity").as_deref()),
    })
}

/// Keeps a listen, or changes the one with its id from the same source. Returns its row id.
pub async fn record<'c, E>(
    conn: E,
    user_id: i64,
    source: Source,
    listen: &Listen,
) -> sqlx::Result<i64>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar(
        "INSERT INTO music_plays (user_id, source, event_id, track_key, track_id, device,
             started_at, ended_at, position_ms, completed, skipped, activity)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id, source, event_id) DO UPDATE SET
             track_key = excluded.track_key, track_id = excluded.track_id,
             device = excluded.device, started_at = excluded.started_at,
             ended_at = excluded.ended_at, position_ms = excluded.position_ms,
             completed = excluded.completed, skipped = excluded.skipped,
             activity = excluded.activity
         RETURNING id",
    )
    .bind(user_id)
    .bind(source.as_str())
    .bind(&listen.event_id)
    .bind(&listen.track_key)
    // A Melete track id is a number in its plain form; anything else names no track here.
    .bind(listen.track_key.parse::<i64>().ok())
    .bind(&listen.device)
    .bind(listen.started_at)
    .bind(listen.ended_at)
    .bind(listen.position_ms)
    .bind(listen.completed)
    .bind(listen.skipped)
    .bind(&listen.activity)
    .fetch_one(conn)
    .await
}

#[derive(sqlx::FromRow)]
struct LoggedEvent {
    seq: i64,
    user_id: i64,
    device_id: String,
    entity_id: String,
    operation: String,
    payload: String,
}

/// Reads the play events pushed since the last read into the history, in log order. Cheap
/// when there's nothing new. Returns how many ops were read.
pub async fn catch_up(pool: &SqlitePool) -> sqlx::Result<u64> {
    let _reading = READING.lock().await;
    let (read, newest): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT seq FROM music_plays_read WHERE id = 1),
             coalesce((SELECT max(seq) FROM aoide_ops WHERE entity = 'play_events'), 0)",
    )
    .fetch_one(pool)
    .await?;
    if newest <= read {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    // Writing first takes the database's write lock before anything is read.
    sqlx::query("UPDATE music_plays_read SET seq = seq WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    let mut cursor = read;
    let mut count = 0;
    loop {
        let events: Vec<LoggedEvent> = sqlx::query_as(
            "SELECT seq, user_id, device_id, entity_id, operation, payload FROM aoide_ops
             WHERE seq > ? AND entity = 'play_events' ORDER BY seq LIMIT ?",
        )
        .bind(cursor)
        .bind(BATCH)
        .fetch_all(&mut *tx)
        .await?;
        let Some(last) = events.last() else {
            break;
        };
        cursor = last.seq;
        for event in &events {
            count += 1;
            if event.operation == "delete" {
                sqlx::query(
                    "DELETE FROM music_plays
                     WHERE user_id = ? AND source = 'sync' AND event_id = ?",
                )
                .bind(event.user_id)
                .bind(&event.entity_id)
                .execute(&mut *tx)
                .await?;
                continue;
            }
            // An op that can't be read as a listen is skipped, as clients skip it, and so is
            // one the user deleted from their history.
            if let Some(listen) = from_payload(&event.entity_id, &event.device_id, &event.payload)
                && !forgotten(&mut *tx, event.user_id, Source::Sync, &listen.event_id).await?
            {
                record(&mut *tx, event.user_id, Source::Sync, &listen).await?;
            }
        }
    }
    sqlx::query("UPDATE music_plays_read SET seq = ? WHERE id = 1")
        .bind(cursor)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(count)
}

/// Whether a user deleted the listen with this id from this source from their history.
pub async fn forgotten<'c, E>(
    conn: E,
    user_id: i64,
    source: Source,
    event_id: &str,
) -> sqlx::Result<bool>
where
    E: sqlx::Executor<'c, Database = sqlx::Sqlite>,
{
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM music_plays_forgotten
             WHERE user_id = ? AND source = ? AND event_id = ?)",
    )
    .bind(user_id)
    .bind(source.as_str())
    .bind(event_id)
    .fetch_one(conn)
    .await
}

/// Which of a user's listens to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forget {
    /// The listen with this row id.
    One(i64),
    /// Those started from the first time (on or after it) until the second (before it), in
    /// milliseconds since the epoch; an end left open when none. Both open is every listen.
    Between(Option<i64>, Option<i64>),
}

/// Deletes a user's own listens, remembering each one's id so the same play event
/// pushed again, or the same listen sent again, doesn't bring it back. The sync log is read in
/// first, so a listen pushed before this was asked for goes too. Returns how many went.
pub async fn forget(pool: &SqlitePool, user_id: i64, which: Forget) -> sqlx::Result<u64> {
    catch_up(pool).await?;
    let (one, from, to) = match which {
        Forget::One(id) => (Some(id), None, None),
        Forget::Between(from, to) => (None, from, to),
    };
    let filter = "FROM music_plays WHERE user_id = ?1 AND (?2 IS NULL OR id = ?2)
         AND (?3 IS NULL OR started_at >= ?3) AND (?4 IS NULL OR started_at < ?4)";
    let mut tx = pool.begin().await?;
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT OR IGNORE INTO music_plays_forgotten (user_id, source, event_id)
         SELECT user_id, source, event_id {filter}"
    )))
    .bind(user_id)
    .bind(one)
    .bind(from)
    .bind(to)
    .execute(&mut *tx)
    .await?;
    // Auto DJ's transitions go with them: the one into a deleted listen, and those
    // started in a deleted range.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "DELETE FROM music_transitions WHERE user_id = ?1 AND (
             play_event_id IN (SELECT event_id {filter})
             OR (?2 IS NULL AND (?3 IS NULL OR started_at >= ?3)
                 AND (?4 IS NULL OR started_at < ?4)))"
    )))
    .bind(user_id)
    .bind(one)
    .bind(from)
    .bind(to)
    .execute(&mut *tx)
    .await?;
    let deleted = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE {filter}")))
        .bind(user_id)
        .bind(one)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(deleted)
}

/// [`catch_up`], logging a failure instead of returning it: for callers whose own work has
/// already succeeded, so the next read picks the ops up.
pub async fn catch_up_or_log(pool: &SqlitePool) {
    if let Err(err) = catch_up(pool).await {
        tracing::error!(error = %err, "couldn't read play events into the play history");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn log(pool: &SqlitePool, user: i64, ops: &[(&str, &str, &str)]) {
        for (entity_id, operation, payload) in ops {
            sqlx::query(
                "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
                     payload, created_at, received_at)
                 VALUES (?, ?, 'phone', 'play_events', ?, ?, ?, 1, 1)",
            )
            .bind(format!("op-{}", crate::auth::random_token("").unwrap()))
            .bind(user)
            .bind(entity_id)
            .bind(operation)
            .bind(payload)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    type Row = (
        String,
        String,
        Option<i64>,
        String,
        i64,
        Option<i64>,
        bool,
        bool,
        Option<String>,
    );

    async fn rows(pool: &SqlitePool) -> Vec<Row> {
        sqlx::query_as(
            "SELECT event_id, track_key, track_id, device, started_at, position_ms, completed,
                 skipped, activity FROM music_plays ORDER BY id",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[test]
    fn reads_a_play_event_in_either_naming_convention() {
        let listen = from_payload(
            "e",
            "d",
            r#"{"jellyfin_id":" 012 ","started_at":"5","ended_at":9.0,"position_ms":4000,
                "completed":1,"activity":"  Morning run  "}"#,
        )
        .unwrap();
        assert_eq!(
            listen,
            Listen {
                event_id: "e".into(),
                track_key: "12".into(),
                device: "d".into(),
                started_at: 5,
                ended_at: Some(9),
                position_ms: Some(4000),
                completed: true,
                skipped: false,
                activity: Some("Morning run".into()),
            }
        );
        assert_eq!(from_payload("e", "d", "[1]"), None);
        assert_eq!(from_payload("e", "d", r#"{"jellyfinId":"1"}"#), None);
        assert_eq!(
            from_payload(
                "e",
                "d",
                r#"{"jellyfinId":"1","startedAt":1,"positionMs":-3}"#
            )
            .unwrap()
            .position_ms,
            None,
            "a negative position isn't one"
        );
        assert_eq!(activity(Some("   ")), None);
        assert_eq!(
            activity(Some(&"x".repeat(100))).unwrap().len(),
            MAX_ACTIVITY
        );
    }

    #[tokio::test]
    async fn reads_the_log_into_the_history_once_in_order() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        log(
            &pool,
            user.id,
            &[
                (
                    "e1",
                    "upsert",
                    r#"{"jellyfinId":"7","startedAt":1000,"activity":"focus"}"#,
                ),
                (
                    "e2",
                    "upsert",
                    r#"{"jellyfinId":"ABC","startedAt":2000,"skipped":true,"positionMs":31000}"#,
                ),
                ("e3", "upsert", r#"{"jellyfinId":"9"}"#),
                // The first listen, finished later: changed in place.
                (
                    "e1",
                    "upsert",
                    r#"{"jellyfinId":"7","startedAt":1000,"completed":true,"activity":"focus"}"#,
                ),
            ],
        )
        .await;
        assert_eq!(catch_up(&pool).await.unwrap(), 4);
        assert_eq!(
            rows(&pool).await,
            [
                (
                    "e1".into(),
                    "7".into(),
                    Some(7),
                    "phone".into(),
                    1000,
                    None,
                    true,
                    false,
                    Some("focus".into())
                ),
                (
                    "e2".into(),
                    "abc".into(),
                    None,
                    "phone".into(),
                    2000,
                    Some(31000),
                    false,
                    true,
                    None
                ),
            ]
        );
        assert_eq!(catch_up(&pool).await.unwrap(), 0, "nothing new to read");

        // A deleted play event leaves the history; the history outlives the log's pruning.
        log(
            &pool,
            user.id,
            &[("e2", "delete", r#"{"jellyfinId":"ABC"}"#)],
        )
        .await;
        assert_eq!(catch_up(&pool).await.unwrap(), 1);
        sqlx::query("DELETE FROM aoide_ops")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(catch_up(&pool).await.unwrap(), 0);
        let kept: Vec<String> = rows(&pool).await.into_iter().map(|r| r.0).collect();
        assert_eq!(kept, ["e1"]);
    }

    #[tokio::test]
    async fn a_deleted_listen_stays_deleted_when_the_log_sends_it_again() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "alice", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let bob = crate::auth::create_user(&pool, "bob", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let started = |at: i64| format!(r#"{{"jellyfinId":"7","startedAt":{at}}}"#);
        log(
            &pool,
            user.id,
            &[
                ("e1", "upsert", started(1000).as_str()),
                ("e2", "upsert", started(5000).as_str()),
            ],
        )
        .await;
        // Bob's listen in the same time is his, and stays.
        log(&pool, bob.id, &[("e1", "upsert", started(1000).as_str())]).await;
        // Not yet read into the history when the delete is asked for: it's read in first.
        assert_eq!(
            forget(&pool, user.id, Forget::Between(Some(500), Some(2000)))
                .await
                .unwrap(),
            1
        );
        // The device pushes the same play event again, finished now.
        log(
            &pool,
            user.id,
            &[(
                "e1",
                "upsert",
                r#"{"jellyfinId":"7","startedAt":1000,"completed":true}"#,
            )],
        )
        .await;
        assert_eq!(catch_up(&pool).await.unwrap(), 1);
        let kept: Vec<(i64, String)> =
            sqlx::query_as("SELECT user_id, event_id FROM music_plays ORDER BY user_id, id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(kept, [(user.id, "e2".into()), (bob.id, "e1".into())]);

        // Reading the whole log again (as after a restore) brings none of it back.
        sqlx::query("UPDATE music_plays_read SET seq = 0")
            .execute(&pool)
            .await
            .unwrap();
        catch_up(&pool).await.unwrap();
        let mine: i64 = sqlx::query_scalar("SELECT count(*) FROM music_plays WHERE user_id = ?")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mine, 1);

        // Everything, by its id or all at once; never someone else's.
        let id: i64 = sqlx::query_scalar("SELECT id FROM music_plays WHERE user_id = ?")
            .bind(bob.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(forget(&pool, user.id, Forget::One(id)).await.unwrap(), 0);
        assert_eq!(
            forget(&pool, user.id, Forget::Between(None, None))
                .await
                .unwrap(),
            1
        );
        assert_eq!(forget(&pool, bob.id, Forget::One(id)).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_listen_sent_again_is_the_same_listen() {
        let pool = crate::db::open_in_memory().await;
        let user = crate::auth::create_user(&pool, "u", "a-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let mut listen = Listen {
            event_id: "x".into(),
            track_key: "3".into(),
            device: "Living room iPad".into(),
            started_at: 10,
            ended_at: None,
            position_ms: None,
            completed: false,
            skipped: false,
            activity: None,
        };
        let first = record(&pool, user.id, Source::Api, &listen).await.unwrap();
        listen.skipped = true;
        listen.position_ms = Some(12_000);
        let again = record(&pool, user.id, Source::Api, &listen).await.unwrap();
        assert_eq!(first, again);
        // The same id from the sync log is a different listen.
        let synced = record(&pool, user.id, Source::Sync, &listen).await.unwrap();
        assert_ne!(first, synced);
        let stored = rows(&pool).await;
        assert_eq!(stored.len(), 2);
        assert_eq!((stored[0].5, stored[0].7), (Some(12_000), true));
    }
}
