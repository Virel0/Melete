//! Watching together: a party of devices playing one film or episode in step. The server keeps
//! the party: which file plays, where and whether it plays, and who's in it. Every device in it
//! learns of each change from the event stream (`watch_party`) and plays its own stream of the
//! file, with its own audio and subtitles, so one can watch dubbed while another has subtitles.
//!
//! A device starts a party and is in it; others join with the party's code. Whoever started it
//! (the host) changes what plays, moving it on to the next episode, say; anyone in it can play,
//! pause and seek. Each change names the version it was made against, and one made against an
//! older version is refused, so two devices acting at once can't undo each other without
//! knowing. Devices are sign-in sessions: one that signs out leaves, and a party with nobody in
//! it, or unchanged for a day, ends.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::db::now_ms;

pub mod guests;
pub mod waits;

/// Bumped at each change to a party, so the event streams send it at once instead of at their
/// next look, up to two seconds later: a play set to start a moment ahead must reach everyone
/// before it does.
static CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<u64>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(0).0);

/// Wakes the event streams: a party changed.
pub fn changed() {
    CHANGES.send_modify(|n| *n = n.wrapping_add(1));
}

/// Told of each change to a party from now on.
pub fn changes() -> tokio::sync::watch::Receiver<u64> {
    CHANGES.subscribe()
}

/// How far ahead a party set playing starts, so every player starts at the same moment rather
/// than as the news reaches each.
pub const START_LEAD_MS: i64 = 400;

/// When a change made `now` takes effect: a moment ahead when it leaves the party playing.
pub(crate) fn starts_at(now: i64, playing: bool) -> i64 {
    if playing { now + START_LEAD_MS } else { now }
}

/// A party unchanged this long ends.
const IDLE_MS: i64 = 24 * 3600 * 1000;
/// Letters and digits that can't be mistaken for one another, as Listen Together's.
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 6;

/// What's playing, as a device sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// The film's or episode's file.
    pub file_id: i64,
    /// How far into it, in milliseconds, when the state was set.
    pub position_ms: i64,
    /// Whether it's playing, not paused.
    pub playing: bool,
}

/// Why a change wasn't made.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// No such party, or the device isn't in it.
    NotFound,
    /// It changed since the version the change was made against.
    Stale,
    /// Only the host may do that.
    NotHost(&'static str),
    Invalid(String),
}

/// Checks a state: a present library file, and a position in it.
async fn check(pool: &SqlitePool, state: &State) -> anyhow::Result<Result<(), Refused>> {
    if state.position_ms < 0 {
        return Ok(Err(Refused::Invalid("position_ms is 0 or more".into())));
    }
    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM media_files WHERE id = ? AND missing_since IS NULL)",
    )
    .bind(state.file_id)
    .fetch_one(pool)
    .await?;
    Ok(if present {
        Ok(())
    } else {
        Err(Refused::Invalid(
            "file_id is a film's or episode's file in the library".into(),
        ))
    })
}

fn random_code() -> anyhow::Result<String> {
    let mut bytes = [0u8; CODE_LENGTH];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no randomness available: {e}"))?;
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect())
}

/// Ends parties nobody is in, and ones unchanged for a day.
async fn tidy(pool: &SqlitePool) -> sqlx::Result<()> {
    let ended: Vec<i64> = sqlx::query_scalar(
        "DELETE FROM watch_parties
         WHERE updated_at < ? OR id NOT IN (SELECT party_id FROM watch_party_members)
         RETURNING id",
    )
    .bind(now_ms() - IDLE_MS)
    .fetch_all(pool)
    .await?;
    for party in ended {
        tracing::info!(
            party,
            "watch party ended: nobody left in it, or unchanged for a day"
        );
    }
    Ok(())
}

/// Starts a party playing `state`, with the device `token_id` (of `user_id`) in it; guests'
/// streams held to `guest_max_bitrate` when given.
pub async fn start(
    pool: &SqlitePool,
    (user_id, token_id): (i64, i64),
    state: &State,
    guest_max_bitrate: Option<i64>,
) -> anyhow::Result<Result<i64, Refused>> {
    if let Err(refused) = check(pool, state).await? {
        return Ok(Err(refused));
    }
    if guest_max_bitrate.is_some_and(|b| b <= 0) {
        return Ok(Err(Refused::Invalid(
            "guest_max_bitrate is bits a second, more than 0".into(),
        )));
    }
    tidy(pool).await?;
    let now = now_ms();
    let mut tx = pool.begin().await?;
    let id: i64 = loop {
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO watch_parties (code, host_user_id, file_id, position_ms, position_at,
                 playing, guest_max_bitrate, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (code) DO NOTHING RETURNING id",
        )
        .bind(random_code()?)
        .bind(user_id)
        .bind(state.file_id)
        .bind(state.position_ms)
        // Nobody else is in it yet: no lead to give.
        .bind(now)
        .bind(state.playing)
        .bind(guest_max_bitrate)
        .bind(now)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(id) = inserted {
            break id;
        }
    };
    sqlx::query("INSERT INTO watch_party_members (party_id, token_id, joined_at) VALUES (?, ?, ?)")
        .bind(id)
        .bind(token_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(
        party = id,
        user = user_id,
        device = token_id,
        file = state.file_id,
        "watch party started"
    );
    Ok(Ok(id))
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

/// The party with `code`, and the file it plays; None for no such code.
pub async fn by_code(pool: &SqlitePool, code: &str) -> anyhow::Result<Option<(i64, i64)>> {
    tidy(pool).await?;
    let Some(code) = normalize_code(code) else {
        return Ok(None);
    };
    Ok(
        sqlx::query_as("SELECT id, file_id FROM watch_parties WHERE code = ?")
            .bind(code)
            .fetch_optional(pool)
            .await?,
    )
}

/// Puts the device in party `id`.
pub async fn join(pool: &SqlitePool, token_id: i64, id: i64) -> sqlx::Result<()> {
    let joined = sqlx::query(
        "INSERT INTO watch_party_members (party_id, token_id, joined_at) VALUES (?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(token_id)
    .bind(now_ms())
    .execute(pool)
    .await?
    .rows_affected();
    if joined == 1 {
        tracing::info!(party = id, device = token_id, "joined a watch party");
    }
    Ok(())
}

/// Takes the device out of the party; false when it wasn't in it. The last to leave ends it.
pub async fn leave(pool: &SqlitePool, token_id: i64, id: i64) -> anyhow::Result<bool> {
    let left = sqlx::query("DELETE FROM watch_party_members WHERE party_id = ? AND token_id = ?")
        .bind(id)
        .bind(token_id)
        .execute(pool)
        .await?
        .rows_affected();
    if left > 0 {
        tracing::info!(party = id, device = token_id, "left a watch party");
    }
    tidy(pool).await?;
    Ok(left > 0)
}

/// The party's host, when the device is in it.
async fn host_for(pool: &SqlitePool, token_id: i64, id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT p.host_user_id FROM watch_parties p
         JOIN watch_party_members m ON m.party_id = p.id AND m.token_id = ?
         WHERE p.id = ?",
    )
    .bind(token_id)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Ends the party for everyone in it, when the device is in it and its user is the host (or
/// an admin).
pub async fn end(
    pool: &SqlitePool,
    (user_id, token_id, is_admin): (i64, i64, bool),
    id: i64,
) -> anyhow::Result<Result<(), Refused>> {
    match host_for(pool, token_id, id).await? {
        None => Ok(Err(Refused::NotFound)),
        Some(host) if host != user_id && !is_admin => Ok(Err(Refused::NotHost(
            "only whoever started it ends it for everyone; leave it instead",
        ))),
        Some(_) => {
            sqlx::query("DELETE FROM watch_parties WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            tracing::info!(
                party = id,
                user = user_id,
                device = token_id,
                "watch party ended"
            );
            Ok(Ok(()))
        }
    }
}

/// Changes what's playing, made against `version`: anyone in it plays, pauses and seeks; only
/// the host changes the file.
pub async fn set(
    pool: &SqlitePool,
    (user_id, token_id): (i64, i64),
    id: i64,
    version: i64,
    state: &State,
) -> anyhow::Result<Result<(), Refused>> {
    let Some(host) = host_for(pool, token_id, id).await? else {
        return Ok(Err(Refused::NotFound));
    };
    let file: i64 = sqlx::query_scalar("SELECT file_id FROM watch_parties WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if file != state.file_id && host != user_id {
        return Ok(Err(Refused::NotHost(
            "only whoever started the party changes what it plays",
        )));
    }
    if let Err(refused) = check(pool, state).await? {
        return Ok(Err(refused));
    }
    let now = now_ms();
    let changed = sqlx::query(
        "UPDATE watch_parties SET file_id = ?, position_ms = ?, position_at = ?, playing = ?,
             held = 0, version = version + 1, updated_at = ?
         WHERE id = ? AND version = ?",
    )
    .bind(state.file_id)
    .bind(state.position_ms)
    .bind(starts_at(now, state.playing))
    .bind(state.playing)
    .bind(now)
    .bind(id)
    .bind(version)
    .execute(pool)
    .await?
    .rows_affected();
    if changed != 1 {
        return Ok(Err(Refused::Stale));
    }
    // Played or paused by hand: that ends any waiting.
    waits::clear(pool, id).await?;
    Ok(Ok(()))
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    code: String,
    host: String,
    file_id: i64,
    movie_id: Option<i64>,
    series_id: Option<i64>,
    season: Option<i64>,
    episode: Option<i64>,
    position_ms: i64,
    position_at: i64,
    playing: bool,
    version: i64,
    guest_max_bitrate: Option<i64>,
    held: bool,
}

/// The parties the device is in, as the API shows them, oldest first.
pub async fn for_device(pool: &SqlitePool, token_id: i64) -> anyhow::Result<Vec<Value>> {
    tidy(pool).await?;
    // Who each waits for, settled first, so what's read next is as it stands.
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT party_id FROM watch_party_members WHERE token_id = ?")
            .bind(token_id)
            .fetch_all(pool)
            .await?;
    let mut waiting = std::collections::HashMap::new();
    for id in ids {
        let names: Vec<String> = waits::names(pool, id)
            .await?
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        waiting.insert(id, names);
    }
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT p.id, p.code, u.username AS host, p.file_id, f.movie_id, f.series_id, f.season,
             f.episode, p.position_ms, p.position_at, p.playing, p.version, p.guest_max_bitrate,
             p.held
         FROM watch_parties p
         JOIN watch_party_members m ON m.party_id = p.id AND m.token_id = ?
         JOIN users u ON u.id = p.host_user_id
         JOIN media_files f ON f.id = p.file_id
         ORDER BY p.id",
    )
    .bind(token_id)
    .fetch_all(pool)
    .await?;
    let pictures = crate::avatars::urls(pool).await?;
    let mut parties = Vec::with_capacity(rows.len());
    for row in rows {
        let members: Vec<(i64, String, String, bool)> = sqlx::query_as(
            "SELECT u.id, u.username, t.name, t.id = ? FROM watch_party_members m
             JOIN tokens t ON t.id = m.token_id JOIN users u ON u.id = t.user_id
             WHERE m.party_id = ? ORDER BY m.joined_at, t.id",
        )
        .bind(token_id)
        .bind(row.id)
        .fetch_all(pool)
        .await?;
        parties.push(json!({
            "id": row.id,
            "code": row.code,
            "host": row.host,
            "file_id": row.file_id,
            "movie_id": row.movie_id,
            "series_id": row.series_id,
            "season": row.season,
            "episode": row.episode,
            "position_ms": row.position_ms,
            "position_at": row.position_at,
            "playing": row.playing,
            "version": row.version,
            "guest_max_bitrate": row.guest_max_bitrate,
            "held": row.held,
            "waiting": waiting.remove(&row.id).unwrap_or_default(),
            "guests": guests::of_party(pool, row.id).await?,
            "members": members.into_iter().map(|(user_id, user, device, you)| json!({
                "username": user, "avatar_url": pictures.get(&user_id), "device": device,
                "you": you,
            })).collect::<Vec<_>>(),
        }));
    }
    Ok(parties)
}

/// One party the device is in, as the API shows it.
pub async fn one(pool: &SqlitePool, token_id: i64, id: i64) -> anyhow::Result<Option<Value>> {
    Ok(for_device(pool, token_id)
        .await?
        .into_iter()
        .find(|p| p["id"] == id))
}

#[cfg(test)]
mod tests;
