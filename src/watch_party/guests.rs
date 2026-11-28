//! Guests of a watch party: someone with no account, let in to one party and nothing else.
//! Anyone in a party may make an invite link; each lets one guest in once, within a day, and
//! gives them a pass for that party alone. A guest plays, pauses and seeks like anyone in it,
//! never changes what plays, and sees the title, the other guests by the names they gave, and
//! how many from the household are watching: no account's name. A guest's pass ends with the
//! party, when they leave, or when anyone in the party sends them away.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{Refused, host_for};
use crate::auth::{random_token, token_hash};
use crate::db::now_ms;

/// An invite link's secret begins so; a guest's pass so.
pub const INVITE_PREFIX: &str = "mpi_";
pub const PASS_PREFIX: &str = "mpg_";
/// How long an invite stays good, unused.
const INVITE_MS: i64 = 24 * 3600 * 1000;
/// The longest name a guest may give.
const NAME_CHARS: usize = 40;

/// A guest, by their pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guest {
    pub id: i64,
    pub party_id: i64,
    pub name: String,
}

/// Makes an invite to party `id` for the device in it; its secret (shown once) and when it
/// stops being good.
pub async fn invite(
    pool: &SqlitePool,
    (user_id, token_id): (i64, i64),
    id: i64,
) -> anyhow::Result<Result<(String, i64), Refused>> {
    if host_for(pool, token_id, id).await?.is_none() {
        return Ok(Err(Refused::NotFound));
    }
    let secret = random_token(INVITE_PREFIX)?;
    let expires_at = now_ms() + INVITE_MS;
    sqlx::query(
        "INSERT INTO watch_party_invites (party_id, invite_hash, created_by, created_at,
             expires_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(token_hash(&secret))
    .bind(user_id)
    .bind(now_ms())
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(Ok((secret, expires_at)))
}

/// A name as a guest gives it: trimmed, without control characters, 1 to 40 characters.
fn clean_name(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty() && name.chars().count() <= NAME_CHARS).then_some(name)
}

/// Lets a guest in with an invite's secret, once: their pass and who they are. None for an
/// invite that's unknown, used or past its day.
pub async fn accept(
    pool: &SqlitePool,
    secret: &str,
    name: &str,
) -> anyhow::Result<Result<(String, Guest), Refused>> {
    let Some(name) = clean_name(name) else {
        return Ok(Err(Refused::Invalid(format!(
            "a name is 1 to {NAME_CHARS} letters, to show the others"
        ))));
    };
    super::tidy(pool).await?;
    let pass = random_token(PASS_PREFIX)?;
    let mut tx = crate::db::write(pool).await?;
    // Taken and spent at once, so two can't come in on one link.
    let taken: Option<(i64, Option<i64>)> = sqlx::query_as(
        "DELETE FROM watch_party_invites WHERE invite_hash = ? AND expires_at > ?
         RETURNING party_id, created_by",
    )
    .bind(token_hash(secret))
    .bind(now_ms())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((party_id, invited_by)) = taken else {
        return Ok(Err(Refused::NotFound));
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO watch_party_guests (party_id, name, pass_hash, invited_by, joined_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(party_id)
    .bind(&name)
    .bind(token_hash(&pass))
    .bind(invited_by)
    .bind(now_ms())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(party = party_id, guest = id, "a guest joined a watch party");
    Ok(Ok((pass, Guest { id, party_id, name })))
}

/// The guest a pass belongs to, while their party lasts.
pub async fn by_pass(pool: &SqlitePool, pass: &str) -> sqlx::Result<Option<Guest>> {
    if !pass.starts_with(PASS_PREFIX) {
        return Ok(None);
    }
    super::tidy(pool).await?;
    let found: Option<(i64, i64, String)> =
        sqlx::query_as("SELECT id, party_id, name FROM watch_party_guests WHERE pass_hash = ?")
            .bind(token_hash(pass))
            .fetch_optional(pool)
            .await?;
    Ok(found.map(|(id, party_id, name)| Guest { id, party_id, name }))
}

/// Whether the guest is still in their party.
pub async fn still_in(pool: &SqlitePool, guest_id: i64) -> sqlx::Result<bool> {
    super::tidy(pool).await?;
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM watch_party_guests WHERE id = ?)")
        .bind(guest_id)
        .fetch_one(pool)
        .await
}

/// Takes back the unused invite hashed as `invite_hash`: it lets no one in.
pub async fn withdraw(pool: &SqlitePool, invite_hash: &[u8]) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM watch_party_invites WHERE invite_hash = ?")
        .bind(invite_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Whether the invite hashed as `invite_hash` is unused, good, and its party still going.
pub async fn invite_open(pool: &SqlitePool, invite_hash: &[u8]) -> sqlx::Result<bool> {
    super::tidy(pool).await?;
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM watch_party_invites WHERE invite_hash = ? AND expires_at > ?)",
    )
    .bind(invite_hash)
    .bind(now_ms())
    .fetch_one(pool)
    .await
}

/// Who let a guest in, while they may still play `file_id`: they're in their party, it
/// plays that file, and it's still going (someone of the household is in it, and it changed
/// within the day); checked as is, without waiting for ended parties to be cleared away. Who
/// let them in must still be able to see the file too, which the caller checks.
pub async fn sees(pool: &SqlitePool, guest_id: i64, file_id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT g.invited_by FROM watch_party_guests g
         JOIN watch_parties p ON p.id = g.party_id
         WHERE g.id = ? AND p.file_id = ? AND p.updated_at >= ? AND g.invited_by IS NOT NULL
           AND EXISTS (SELECT 1 FROM watch_party_members m WHERE m.party_id = p.id)",
    )
    .bind(guest_id)
    .bind(file_id)
    .bind(now_ms() - super::IDLE_MS)
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// Plays, pauses or seeks the guest's party, made against `version`; what plays stays.
pub async fn set(
    pool: &SqlitePool,
    guest: &Guest,
    version: i64,
    (position_ms, playing): (i64, bool),
) -> anyhow::Result<Result<(), Refused>> {
    if position_ms < 0 {
        return Ok(Err(Refused::Invalid("position_ms is 0 or more".into())));
    }
    let Some(before) = super::last_change::before(pool, guest.party_id).await? else {
        return Ok(Err(Refused::Stale));
    };
    let now = now_ms();
    let changed = sqlx::query(
        "UPDATE watch_parties SET position_ms = ?, position_at = ?, playing = ?, held = 0,
             version = version + 1, updated_at = ?
         WHERE id = ? AND version = ?",
    )
    .bind(position_ms)
    .bind(super::starts_at(now, playing))
    .bind(playing)
    .bind(now)
    .bind(guest.party_id)
    .bind(version)
    .execute(pool)
    .await?
    .rows_affected();
    if changed != 1 {
        return Ok(Err(Refused::Stale));
    }
    let made = (before.file_id, position_ms, playing);
    if let Some(kind) = super::last_change::kind(&before, made, now) {
        let by = super::last_change::By::Guest(guest.id, &guest.name);
        super::last_change::note(pool, guest.party_id, by, kind, now).await?;
    }
    super::waits::clear(pool, guest.party_id).await?;
    Ok(Ok(()))
}

/// The guest leaves; their pass and stream links end.
pub async fn leave(pool: &SqlitePool, guest_id: i64) -> sqlx::Result<()> {
    let party: Option<i64> =
        sqlx::query_scalar("DELETE FROM watch_party_guests WHERE id = ? RETURNING party_id")
            .bind(guest_id)
            .fetch_optional(pool)
            .await?;
    if let Some(party) = party {
        tracing::info!(party, guest = guest_id, "a guest left a watch party");
    }
    Ok(())
}

/// Sends a guest away from party `id`, for a device in it.
pub async fn send_away(
    pool: &SqlitePool,
    token_id: i64,
    id: i64,
    guest_id: i64,
) -> anyhow::Result<Result<(), Refused>> {
    if host_for(pool, token_id, id).await?.is_none() {
        return Ok(Err(Refused::NotFound));
    }
    let gone = sqlx::query("DELETE FROM watch_party_guests WHERE id = ? AND party_id = ?")
        .bind(guest_id)
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(if gone == 1 {
        tracing::info!(
            party = id,
            guest = guest_id,
            device = token_id,
            "a guest was sent away from a watch party"
        );
        Ok(())
    } else {
        Err(Refused::NotFound)
    })
}

/// The party's guests, for those in it: `{id, name}` each.
pub async fn of_party(pool: &SqlitePool, id: i64) -> sqlx::Result<Vec<Value>> {
    let guests: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM watch_party_guests WHERE party_id = ? ORDER BY joined_at, id",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(guests
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect())
}

#[derive(sqlx::FromRow)]
struct Seen {
    file_id: i64,
    position_ms: i64,
    position_at: i64,
    playing: bool,
    version: i64,
    guest_max_bitrate: Option<i64>,
    movie: Option<String>,
    series: Option<String>,
    season: Option<i64>,
    episode: Option<i64>,
    household: i64,
    held: bool,
}

/// The guest's party as they see it: what plays (its title), where and whether, the other
/// guests by name, and how many of the household's devices are in it.
pub async fn view(pool: &SqlitePool, guest: &Guest) -> sqlx::Result<Option<Value>> {
    let viewer = super::last_change::Viewer::Guest(guest.id);
    let waits = super::waits::waits(pool, guest.party_id).await?;
    let waiting: Vec<&str> = waits
        .iter()
        .map(|w| {
            if w.guest {
                w.name.as_str()
            } else {
                super::waits::SOMEONE
            }
        })
        .collect();
    let seen: Option<Seen> = sqlx::query_as(
        "SELECT p.file_id, p.position_ms, p.position_at, p.playing, p.version,
             p.guest_max_bitrate, m.parsed_title AS movie, s.parsed_title AS series, f.season,
             f.episode, p.held,
             (SELECT count(*) FROM watch_party_members WHERE party_id = p.id) AS household
         FROM watch_parties p JOIN media_files f ON f.id = p.file_id
         LEFT JOIN movies m ON m.id = f.movie_id LEFT JOIN series s ON s.id = f.series_id
         WHERE p.id = ?",
    )
    .bind(guest.party_id)
    .fetch_optional(pool)
    .await?;
    let Some(seen) = seen else { return Ok(None) };
    let title = match (seen.movie, seen.series, seen.season, seen.episode) {
        (Some(movie), ..) => movie,
        (None, Some(series), Some(season), Some(episode)) => {
            format!("{series}, season {season} episode {episode}")
        }
        (None, Some(series), ..) => series,
        _ => "What's playing".to_owned(),
    };
    let guests: Vec<Value> = of_party(pool, guest.party_id)
        .await?
        .into_iter()
        .map(|g| json!({ "name": g["name"], "you": g["id"] == guest.id }))
        .collect();
    Ok(Some(json!({
        "id": guest.party_id,
        "you": guest.name,
        "title": title,
        "file_id": seen.file_id,
        "position_ms": seen.position_ms,
        "position_at": seen.position_at,
        "playing": seen.playing,
        "version": seen.version,
        "max_bitrate": seen.guest_max_bitrate,
        "household": seen.household,
        "held": seen.held,
        "waiting": waiting,
        "waits": super::waits::view(&waits, viewer),
        "last_change": super::last_change::view(pool, guest.party_id, viewer).await?,
        "guests": guests,
    })))
}

#[cfg(test)]
mod tests;
