//! Listening together: a session two or more household devices play in step, like
//! SharePlay. The server keeps the session: the queue, which track is playing and where, whether
//! it's playing, and who's in it. Every device in it learns of each change from the event stream
//! (`listening`) and plays its own stream of the track, so nothing goes through anyone's device
//! but Melete.
//!
//! A device starts a session and is in it; others join with the session's code, which those in
//! it can see and share, so nobody's listening shows up anywhere they didn't ask for it. Anyone
//! in a session can change it. Each change names the version it was made against, and one made
//! against an older version is refused, so two devices acting at once can't undo each other
//! without knowing. Devices are sign-in sessions: one that signs out leaves, and a session with
//! nobody in it, or unchanged for a day, ends.

use crate::db::now_ms;
use serde_json::{Value, json};
use sqlx::SqlitePool;

/// Tracks a queue may hold.
pub const MAX_QUEUE: usize = 1000;
/// A session unchanged this long ends.
const IDLE_MS: i64 = 24 * 3600 * 1000;
/// Letters and digits that can't be mistaken for one another, as Quick Connect's.
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 6;

/// What's playing, as a device sets it.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// The tracks to play, in order, by id; at most [`MAX_QUEUE`].
    pub queue: Vec<i64>,
    /// Which track in the queue is playing, from 0; 0 for an empty queue.
    pub index: i64,
    /// How far into that track, in milliseconds, when the state was set.
    pub position_ms: i64,
    /// Whether it's playing, not paused.
    pub playing: bool,
}

/// Why a change wasn't made.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// No such session, or the device isn't in it.
    NotFound,
    /// It changed since the version the change was made against.
    Stale,
    Invalid(String),
}

/// Checks a state: a queue of present tracks, and an index and position inside it.
async fn check(pool: &SqlitePool, state: &State) -> anyhow::Result<Result<(), Refused>> {
    let invalid = |why: &str| Ok(Err(Refused::Invalid(why.to_owned())));
    if state.queue.len() > MAX_QUEUE {
        return invalid("a queue holds at most 1000 tracks");
    }
    if state.position_ms < 0 {
        return invalid("position_ms is 0 or more");
    }
    let in_queue = usize::try_from(state.index).is_ok_and(|i| i < state.queue.len());
    if !in_queue && !(state.queue.is_empty() && state.index == 0) {
        return invalid("index is the place of a track in the queue, from 0");
    }
    let present: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT t.id) FROM music_tracks t
         WHERE t.missing_since IS NULL AND t.id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&state.queue)?)
    .fetch_one(pool)
    .await?;
    let distinct: std::collections::HashSet<i64> = state.queue.iter().copied().collect();
    if usize::try_from(present).ok() != Some(distinct.len()) {
        return invalid("every track in the queue must be in the library");
    }
    Ok(Ok(()))
}

fn random_code() -> anyhow::Result<String> {
    let mut bytes = [0u8; CODE_LENGTH];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect())
}

/// A typed code: any case, spaces and dashes aside. None when it can't be one.
pub fn normalize_code(code: &str) -> Option<String> {
    let code: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.len() == CODE_LENGTH && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

/// Ends sessions nobody is in, and ones unchanged for a day.
async fn tidy(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        "DELETE FROM listen_sessions
         WHERE updated_at < ? OR id NOT IN (SELECT session_id FROM listen_members)",
    )
    .bind(now_ms() - IDLE_MS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Starts a session playing `state`, with the device `token_id` (of `user_id`) in it.
pub async fn start(
    pool: &SqlitePool,
    user_id: i64,
    token_id: i64,
    state: &State,
) -> anyhow::Result<Result<i64, Refused>> {
    if let Err(refused) = check(pool, state).await? {
        return Ok(Err(refused));
    }
    tidy(pool).await?;
    let now = now_ms();
    let mut tx = pool.begin().await?;
    let id: i64 = loop {
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO listen_sessions (code, host_user_id, queue, queue_index, position_ms,
                 position_at, playing, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (code) DO NOTHING RETURNING id",
        )
        .bind(random_code()?)
        .bind(user_id)
        .bind(serde_json::to_string(&state.queue)?)
        .bind(state.index)
        .bind(state.position_ms)
        .bind(now)
        .bind(state.playing)
        .bind(now)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(id) = inserted {
            break id;
        }
    };
    sqlx::query("INSERT INTO listen_members (session_id, token_id, joined_at) VALUES (?, ?, ?)")
        .bind(id)
        .bind(token_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(id))
}

/// Puts the device in the session with `code`; the session's id, or None for no such code.
pub async fn join(pool: &SqlitePool, token_id: i64, code: &str) -> anyhow::Result<Option<i64>> {
    tidy(pool).await?;
    let Some(code) = normalize_code(code) else {
        return Ok(None);
    };
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM listen_sessions WHERE code = ?")
        .bind(code)
        .fetch_optional(pool)
        .await?;
    let Some(id) = id else { return Ok(None) };
    sqlx::query(
        "INSERT INTO listen_members (session_id, token_id, joined_at) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(token_id)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(Some(id))
}

/// Takes the device out of the session; false when it wasn't in it. The last to leave ends it.
pub async fn leave(pool: &SqlitePool, token_id: i64, id: i64) -> anyhow::Result<bool> {
    let left = sqlx::query("DELETE FROM listen_members WHERE session_id = ? AND token_id = ?")
        .bind(id)
        .bind(token_id)
        .execute(pool)
        .await?
        .rows_affected();
    tidy(pool).await?;
    Ok(left > 0)
}

/// Ends the session for everyone in it, when the device is in it and its user started it (or is
/// an admin).
pub async fn end(
    pool: &SqlitePool,
    token_id: i64,
    user_id: i64,
    is_admin: bool,
    id: i64,
) -> anyhow::Result<Result<(), Refused>> {
    let host: Option<i64> = sqlx::query_scalar(
        "SELECT s.host_user_id FROM listen_sessions s
         JOIN listen_members m ON m.session_id = s.id AND m.token_id = ?
         WHERE s.id = ?",
    )
    .bind(token_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    match host {
        None => Ok(Err(Refused::NotFound)),
        Some(host) if host != user_id && !is_admin => Ok(Err(Refused::Invalid(
            "only whoever started it ends it for everyone; leave it instead".into(),
        ))),
        Some(_) => {
            sqlx::query("DELETE FROM listen_sessions WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            Ok(Ok(()))
        }
    }
}

/// Changes what's playing, made against `version`.
pub async fn set(
    pool: &SqlitePool,
    token_id: i64,
    id: i64,
    version: i64,
    state: &State,
) -> anyhow::Result<Result<(), Refused>> {
    if !member(pool, token_id, id).await? {
        return Ok(Err(Refused::NotFound));
    }
    if let Err(refused) = check(pool, state).await? {
        return Ok(Err(refused));
    }
    let now = now_ms();
    let changed = sqlx::query(
        "UPDATE listen_sessions SET queue = ?, queue_index = ?, position_ms = ?, position_at = ?,
             playing = ?, version = version + 1, updated_at = ?
         WHERE id = ? AND version = ?",
    )
    .bind(serde_json::to_string(&state.queue)?)
    .bind(state.index)
    .bind(state.position_ms)
    .bind(now)
    .bind(state.playing)
    .bind(now)
    .bind(id)
    .bind(version)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(if changed == 1 {
        Ok(())
    } else {
        Err(Refused::Stale)
    })
}

async fn member(pool: &SqlitePool, token_id: i64, id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM listen_members WHERE session_id = ? AND token_id = ?)",
    )
    .bind(id)
    .bind(token_id)
    .fetch_one(pool)
    .await
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    code: String,
    host: String,
    queue: String,
    queue_index: i64,
    position_ms: i64,
    position_at: i64,
    playing: bool,
    version: i64,
}

/// The sessions the device is in, as the API shows them, oldest first.
pub async fn for_device(pool: &SqlitePool, token_id: i64) -> anyhow::Result<Vec<Value>> {
    tidy(pool).await?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT s.id, s.code, u.username AS host, s.queue, s.queue_index, s.position_ms,
             s.position_at, s.playing, s.version
         FROM listen_sessions s
         JOIN listen_members m ON m.session_id = s.id AND m.token_id = ?
         JOIN users u ON u.id = s.host_user_id
         ORDER BY s.id",
    )
    .bind(token_id)
    .fetch_all(pool)
    .await?;
    let mut sessions = Vec::with_capacity(rows.len());
    for row in rows {
        let members: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT u.username, t.name, t.id = ? FROM listen_members m
             JOIN tokens t ON t.id = m.token_id JOIN users u ON u.id = t.user_id
             WHERE m.session_id = ? ORDER BY m.joined_at, t.id",
        )
        .bind(token_id)
        .bind(row.id)
        .fetch_all(pool)
        .await?;
        sessions.push(json!({
            "id": row.id,
            "code": row.code,
            "host": row.host,
            "queue": serde_json::from_str::<Value>(&row.queue)?,
            "index": row.queue_index,
            "position_ms": row.position_ms,
            "position_at": row.position_at,
            "playing": row.playing,
            "version": row.version,
            "members": members.into_iter().map(|(user, device, you)| json!({
                "username": user, "device": device, "you": you,
            })).collect::<Vec<_>>(),
        }));
    }
    Ok(sessions)
}

/// One session the device is in, as the API shows it.
pub async fn one(pool: &SqlitePool, token_id: i64, id: i64) -> anyhow::Result<Option<Value>> {
    Ok(for_device(pool, token_id)
        .await?
        .into_iter()
        .find(|s| s["id"] == id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{self, TokenKind};

    async fn setup() -> (SqlitePool, i64, i64, i64) {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                 seen_generation) VALUES (1, 1, 'k', 'A', 'B', 'a', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres,
                 missing_since) VALUES
                 (1, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'One', '[]', NULL),
                 (2, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Two', '[]', NULL),
                 (3, 1, 'a/3', 1, 1, 1, 1, 1, 1, 'Gone', '[]', 5);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let alice = auth::create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let bob = auth::create_user(&pool, "bob", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (phone, _) = auth::issue_token(&pool, alice.id, TokenKind::Session, "Living room iPad")
            .await
            .unwrap();
        let (laptop, _) = auth::issue_token(&pool, bob.id, TokenKind::Session, "Laptop")
            .await
            .unwrap();
        (pool, alice.id, phone, laptop)
    }

    fn playing(queue: &[i64], index: i64) -> State {
        State {
            queue: queue.to_vec(),
            index,
            position_ms: 0,
            playing: true,
        }
    }

    #[tokio::test]
    async fn devices_join_by_code_and_follow_one_state() {
        let (pool, alice, ipad, laptop) = setup().await;
        let id = start(&pool, alice, ipad, &playing(&[1, 2], 0))
            .await
            .unwrap()
            .unwrap();
        assert!(
            for_device(&pool, laptop).await.unwrap().is_empty(),
            "not until it joins"
        );
        let session = one(&pool, ipad, id).await.unwrap().unwrap();
        let code = session["code"].as_str().unwrap().to_owned();
        assert_eq!(session["host"], "alice");
        assert_eq!(session["queue"], json!([1, 2]));
        assert_eq!(session["version"], 1);
        assert_eq!(join(&pool, laptop, "NOPE00").await.unwrap(), None);
        let typed = format!(" {}-{} ", code[..3].to_lowercase(), &code[3..]);
        assert_eq!(join(&pool, laptop, &typed).await.unwrap(), Some(id));
        let seen = one(&pool, laptop, id).await.unwrap().unwrap();
        assert_eq!(
            seen["members"],
            json!([
                { "username": "alice", "device": "Living room iPad", "you": false },
                { "username": "bob", "device": "Laptop", "you": true },
            ])
        );

        // Anyone in it can change it, against the version they saw.
        let paused = State {
            queue: vec![1, 2],
            index: 1,
            position_ms: 30_000,
            playing: false,
        };
        assert_eq!(set(&pool, laptop, id, 1, &paused).await.unwrap(), Ok(()));
        assert_eq!(
            set(&pool, ipad, id, 1, &playing(&[1, 2], 0)).await.unwrap(),
            Err(Refused::Stale),
            "made against an older version"
        );
        let now = one(&pool, ipad, id).await.unwrap().unwrap();
        assert_eq!(
            (
                &now["index"],
                &now["position_ms"],
                &now["playing"],
                &now["version"]
            ),
            (&json!(1), &json!(30_000), &json!(false), &json!(2))
        );
    }

    #[tokio::test]
    async fn queues_hold_only_tracks_here_and_places_in_them() {
        let (pool, alice, ipad, laptop) = setup().await;
        for (state, why) in [
            (playing(&[1, 3], 0), "in the library"),
            (playing(&[1, 99], 0), "in the library"),
            (playing(&[1, 2], 2), "index"),
            (playing(&[], 1), "index"),
        ] {
            match start(&pool, alice, ipad, &state).await.unwrap() {
                Err(Refused::Invalid(message)) => assert!(message.contains(why), "{message}"),
                other => panic!("{state:?}: {other:?}"),
            }
        }
        let id = start(&pool, alice, ipad, &playing(&[], 0))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            set(&pool, laptop, id, 1, &playing(&[1], 0)).await.unwrap(),
            Err(Refused::NotFound),
            "only those in it change it"
        );
    }

    #[tokio::test]
    async fn the_last_to_leave_ends_it_and_signing_out_leaves() {
        let (pool, alice, ipad, laptop) = setup().await;
        let id = start(&pool, alice, ipad, &playing(&[1], 0))
            .await
            .unwrap()
            .unwrap();
        let code = one(&pool, ipad, id).await.unwrap().unwrap()["code"]
            .as_str()
            .unwrap()
            .to_owned();
        join(&pool, laptop, &code).await.unwrap();
        // Only whoever started it ends it for everyone.
        assert!(matches!(
            end(&pool, laptop, 0, false, id).await.unwrap(),
            Err(Refused::Invalid(_))
        ));
        assert!(leave(&pool, laptop, id).await.unwrap());
        assert!(!leave(&pool, laptop, id).await.unwrap());
        // The iPad signs out: nobody's left, so the session ends.
        sqlx::query("DELETE FROM tokens WHERE id = ?")
            .bind(ipad)
            .execute(&pool)
            .await
            .unwrap();
        tidy(&pool).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM listen_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);

        let id = start(&pool, alice, laptop, &playing(&[1], 0))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(end(&pool, laptop, alice, false, id).await.unwrap(), Ok(()));
        assert!(for_device(&pool, laptop).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_session_unchanged_for_a_day_ends() {
        let (pool, alice, ipad, _) = setup().await;
        start(&pool, alice, ipad, &playing(&[1], 0))
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE listen_sessions SET updated_at = ?")
            .bind(now_ms() - IDLE_MS - 1)
            .execute(&pool)
            .await
            .unwrap();
        assert!(for_device(&pool, ipad).await.unwrap().is_empty());
    }
}
