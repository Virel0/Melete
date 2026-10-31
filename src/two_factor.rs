//! Two-factor sign-in: once a user turns it on, a right password alone no longer signs in; it
//! gives a short-lived challenge, finished with a code from their authenticator app (TOTP).
//! Optional for every user, and only for signing in with a password: Quick Connect is approved
//! from a device already signed in, and an identity provider asks for its own second factor.
//!
//! The secret is kept sealed, not hashed (a code can't be checked against a hash; see
//! `secrets`), sent to the user only
//! while setup waits for its first code, and never logged. A code is taken once: never again
//! for its own step or an older one. The way back from a lost device is the server's command
//! line, `melete user reset-password`, which offers to remove it.

use anyhow::anyhow;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::auth::User;
use crate::db::unix_now;
use crate::secrets;

mod challenges;
pub mod qr;
mod record;
pub mod totp;

pub use challenges::*;
pub use record::*;

/// The name an authenticator app shows the account under, before the username.
const ISSUER: &str = "Melete";

/// Whether a user has the second factor on, and since when.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Status {
    /// Sign-in with a password asks for a code.
    pub on: bool,
    /// When setup was confirmed; None while it's off.
    pub on_since: Option<i64>,
}

/// A secret just made, for the user to put in their authenticator app. No `Debug`, so it can't
/// end up in a log by accident.
pub struct Setup {
    /// The secret in base32, as apps take it typed in.
    pub secret: String,
    /// The `otpauth://` link apps take from a QR code or a tap.
    pub uri: String,
}

/// What confirming setup with a first code did.
#[derive(Debug, PartialEq, Eq)]
pub enum Confirmed {
    /// The second factor is on.
    Done,
    /// No setup waits for a code.
    NothingWaiting,
    /// It's on already.
    AlreadyOn,
    /// The code isn't the secret's, now.
    WrongCode,
}

/// Where a change to how a user signs in was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MadeFrom {
    /// By the user, signed in to their account.
    Account,
    /// On the server's command line.
    Server,
}

impl MadeFrom {
    fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Server => "server",
        }
    }
}

/// Where a user's second factor stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// None, not even a setup waiting.
    Off,
    /// Setup waits for its first code.
    Waiting,
    /// Sign-in asks for a code.
    On,
}

/// Where the user's second factor stands.
pub async fn stage(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Stage> {
    let row: Option<Option<i64>> =
        sqlx::query_scalar("SELECT confirmed_at FROM two_factor WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        None => Stage::Off,
        Some(None) => Stage::Waiting,
        Some(Some(_)) => Stage::On,
    })
}

/// Whether sign-in with a password asks this user for a code.
pub async fn is_on(pool: &SqlitePool, user_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM two_factor WHERE user_id = ? AND confirmed_at IS NOT NULL)",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// Whether the user has it on, and since when.
pub async fn status(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Status> {
    let on_since: Option<i64> = sqlx::query_scalar(
        "SELECT confirmed_at FROM two_factor WHERE user_id = ? AND confirmed_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(Status {
        on: on_since.is_some(),
        on_since,
    })
}

/// A new secret for `user`, replacing one still waiting for its first code. None when the
/// second factor is on already: it's turned off before it's set up again.
pub async fn start_setup(pool: &SqlitePool, user: &User) -> anyhow::Result<Option<Setup>> {
    let mut secret = [0u8; 20];
    getrandom::fill(&mut secret).map_err(|e| anyhow!("no randomness available: {e}"))?;
    let sealing = secrets::sealing(pool).await?;
    let stored = secrets::bytes_to_store(sealing, &secrets::two_factor(user.id), &secret)?;
    let made = sqlx::query(
        "INSERT INTO two_factor (user_id, secret, created_at) VALUES (?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE
             SET secret = excluded.secret, created_at = excluded.created_at, last_step = NULL
             WHERE two_factor.confirmed_at IS NULL",
    )
    .bind(user.id)
    .bind(&stored)
    .bind(unix_now())
    .execute(pool)
    .await?
    .rows_affected();
    if made == 0 {
        return Ok(None);
    }
    let encoded = totp::base32(&secret);
    // Usernames are letters, digits, dots, dashes and underscores: nothing to escape.
    let uri = format!(
        "otpauth://totp/{ISSUER}:{name}?secret={encoded}&issuer={ISSUER}&algorithm=SHA1&digits={digits}&period={period}",
        name = user.username,
        digits = totp::DIGITS,
        period = totp::STEP,
    );
    Ok(Some(Setup {
        secret: encoded,
        uri,
    }))
}

/// Turns the second factor on when `code` is the waiting secret's, now, and records it.
pub async fn confirm(
    pool: &SqlitePool,
    user_id: i64,
    code: &str,
    now: i64,
) -> sqlx::Result<Confirmed> {
    let mut tx = pool.begin().await?;
    let row: Option<(Vec<u8>, Option<i64>)> =
        sqlx::query_as("SELECT secret, confirmed_at FROM two_factor WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let stored = match row {
        None => return Ok(Confirmed::NothingWaiting),
        Some((_, Some(_))) => return Ok(Confirmed::AlreadyOn),
        Some((stored, None)) => stored,
    };
    let secret = opened(secrets::sealing(&mut *tx).await?, user_id, &stored)?;
    let Some(step) = totp::matching_step(&secret, code, now) else {
        return Ok(Confirmed::WrongCode);
    };
    // Only the secret that code was checked against: a setup started meanwhile replaced it.
    let done = sqlx::query(
        "UPDATE two_factor SET confirmed_at = ?, last_step = ?
         WHERE user_id = ? AND secret = ? AND confirmed_at IS NULL",
    )
    .bind(now)
    .bind(step)
    .bind(user_id)
    .bind(&stored)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if done == 0 {
        return Ok(Confirmed::NothingWaiting);
    }
    record(&mut tx, user_id, "two_factor_on", MadeFrom::Account, now).await?;
    tx.commit().await?;
    Ok(Confirmed::Done)
}

/// Takes `code` from a user with the second factor on: true when it's theirs for a step in the
/// window that's newer than any taken before, which it then becomes, so it's never taken again.
pub async fn take_code(
    pool: &SqlitePool,
    user_id: i64,
    code: &str,
    now: i64,
) -> sqlx::Result<bool> {
    let stored: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT secret FROM two_factor WHERE user_id = ? AND confirmed_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let secret = match stored {
        Some(stored) => Some(opened(secrets::sealing(pool).await?, user_id, &stored)?),
        None => None,
    };
    let Some(step) = secret.and_then(|secret| totp::matching_step(&secret, code, now)) else {
        return Ok(false);
    };
    // One statement, so two requests with the same code can't both take it.
    let taken = sqlx::query(
        "UPDATE two_factor SET last_step = ?1
         WHERE user_id = ?2 AND confirmed_at IS NOT NULL AND (last_step IS NULL OR last_step < ?1)",
    )
    .bind(step)
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(taken == 1)
}

/// The user's secret, from what's stored for it.
fn opened(sealing: bool, user_id: i64, stored: &[u8]) -> sqlx::Result<Vec<u8>> {
    let what = secrets::two_factor(user_id);
    secrets::bytes_from_store(sealing, &what, stored, secrets::startup::TWO_FACTOR_LEN)
}

/// Removes the user's second factor, on or still being set up, with any sign-in waiting for a
/// code. Records the change when it was on; true when it was.
pub async fn remove(pool: &SqlitePool, user_id: i64, from: MadeFrom) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let removed: Option<Option<i64>> =
        sqlx::query_scalar("DELETE FROM two_factor WHERE user_id = ? RETURNING confirmed_at")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    sqlx::query("DELETE FROM sign_in_challenges WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let was_on = matches!(removed, Some(Some(_)));
    if was_on {
        record(&mut tx, user_id, "two_factor_off", from, unix_now()).await?;
    }
    tx.commit().await?;
    Ok(was_on)
}

#[cfg(test)]
mod tests;
