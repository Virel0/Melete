//! Users, passwords and bearer tokens.
//!
//! Passwords are hashed with Argon2id. Tokens, both login sessions and API keys, are 32 random
//! bytes shown to the client once; only their SHA-256 is stored.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::db::unix_now;
use anyhow::{Context, anyhow};
use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

mod scope;

pub use scope::*;

/// What every token Melete issues starts with; a token without it is refused before any lookup.
pub const TOKEN_PREFIX: &str = "mlt_";
const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 1024;
/// `last_used_at` is only rewritten when older than this, to spare a write per request.
const LAST_USED_RESOLUTION: i64 = 60;
/// A session (not an API key) unused this long is signed out.
const SESSION_IDLE: i64 = 90 * 24 * 3600;
const MAX_FAILED_LOGINS: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);

/// Each Argon2 run holds about 19 MiB; cap how many run at once so a burst of sign-ins
/// can't exhaust memory. Further requests wait their turn.
static PASSWORD_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

/// A Melete account as requests see it; the password hash stays in the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct User {
    /// The user's row id.
    pub id: i64,
    /// Unique regardless of case; see [`validate_username`].
    pub username: String,
    /// Admins manage the server; read afresh on every request.
    pub is_admin: bool,
}

/// Which kind of token to issue: a session signs out after 90 days unused, an API key never.
#[derive(Debug, Clone, Copy)]
pub enum TokenKind {
    Session,
    ApiKey,
}

impl TokenKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::ApiKey => "api_key",
        }
    }
}

/// `Err` with the rule, for the caller to show, when a username breaks it.
pub fn validate_username(username: &str) -> Result<(), &'static str> {
    let valid_chars = username
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    // Starting with a letter or digit keeps `user add --admin` from making a user "--admin".
    let valid_start = username
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric());
    if username.len() > 64 || !valid_chars || !valid_start {
        return Err(
            "usernames are 1-64 letters, digits, dots, dashes or underscores, starting with a letter or digit",
        );
    }
    Ok(())
}

/// `Err` with the rule, for the caller to show, when a password isn't 8 to 1024 characters.
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    let length = password.chars().count();
    if !(MIN_PASSWORD..=MAX_PASSWORD).contains(&length) {
        return Err("passwords are 8 to 1024 characters");
    }
    Ok(())
}

/// A token or device name: trimmed, 1-100 characters.
pub fn validate_label(label: &str) -> Result<&str, &'static str> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 100 {
        return Err("names are 1 to 100 characters");
    }
    Ok(label)
}

/// A random 20-character password, for new accounts and resets made from the command line.
pub fn generate_password() -> anyhow::Result<String> {
    // No 0/O or 1/l/I, so it can be read off a terminal.
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no randomness available: {e}"))?;
    // 256 % 57 leaves a small bias, irrelevant at 20 characters (~116 bits).
    Ok(bytes
        .iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect())
}

fn generate_token() -> anyhow::Result<String> {
    random_token(TOKEN_PREFIX)
}

/// 32 random bytes as hex after `prefix`. Store only [`token_hash`] of it.
pub(crate) fn random_token(prefix: &str) -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no randomness available: {e}"))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{prefix}{hex}"))
}

pub(crate) fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Hashes off the async runtime: Argon2 is deliberately slow.
pub async fn hash_password(password: String) -> anyhow::Result<String> {
    let _slot = PASSWORD_SLOTS.acquire().await?;
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|e| anyhow!("hashing password: {e}"))
    })
    .await?
}

/// Checked against when a username doesn't exist, so the reply takes as long either way and
/// doesn't reveal which usernames are real. Its salt is fixed, so making it needs no randomness.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    Argon2::default()
        .hash_password_with_salt(b"not a real password", b"a fixed salt, 16")
        .expect("a constant password with a 16-byte salt always hashes")
        .to_string()
});

async fn verify_password(password: String, hash: Option<String>) -> anyhow::Result<bool> {
    let _slot = PASSWORD_SLOTS.acquire().await?;
    tokio::task::spawn_blocking(move || {
        let real = hash.is_some();
        let hash = hash.unwrap_or_else(|| DUMMY_HASH.clone());
        let parsed =
            PasswordHash::new(&hash).map_err(|e| anyhow!("stored hash unreadable: {e}"))?;
        let matches = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(real && matches)
    })
    .await?
}

/// Creates a user. `None` if the username is taken (case-insensitively).
pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password: String,
    is_admin: bool,
) -> anyhow::Result<Option<User>> {
    let hash = hash_password(password).await?;
    let user = sqlx::query_as(
        "INSERT INTO users (username, password_hash, is_admin, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (username) DO NOTHING
         RETURNING id, username, is_admin",
    )
    .bind(username)
    .bind(hash)
    .bind(is_admin)
    .bind(unix_now())
    .fetch_optional(pool)
    .await?;
    Ok(user)
}

/// The user with this username, regardless of case; None when there's none.
pub async fn find_user(pool: &SqlitePool, username: &str) -> anyhow::Result<Option<User>> {
    Ok(
        sqlx::query_as("SELECT id, username, is_admin FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(pool)
            .await?,
    )
}

/// The user, if the username exists and the password is right.
pub async fn check_password(
    pool: &SqlitePool,
    username: &str,
    password: String,
) -> anyhow::Result<Option<User>> {
    let row: Option<(i64, String, bool, String)> = sqlx::query_as(
        "SELECT id, username, is_admin, password_hash FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    let (user, hash) = match row {
        Some((id, username, is_admin, hash)) => (
            Some(User {
                id,
                username,
                is_admin,
            }),
            Some(hash),
        ),
        None => (None, None),
    };
    Ok(if verify_password(password, hash).await? {
        user
    } else {
        None
    })
}

/// Sets a new password and signs out every other session and API key of the user, and unlinks
/// any identity provider, which would otherwise outlast the reset. `keep_token` (the caller's
/// own token) stays valid.
pub async fn set_password(
    pool: &SqlitePool,
    user_id: i64,
    password: String,
    keep_token: Option<i64>,
) -> anyhow::Result<()> {
    let hash = hash_password(password).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM tokens WHERE user_id = ? AND id IS NOT ?")
        .bind(user_id)
        .bind(keep_token)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM oidc_identities WHERE user_id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// What deleting a user did.
#[derive(Debug, PartialEq, Eq)]
pub enum Deleted {
    Done,
    NoSuchUser,
    /// The last admin can't be deleted: nobody could manage the server after.
    LastAdmin,
}

/// Deletes a user with their sessions, API keys (and so their stream links) and watch
/// progress. Checked and done in one transaction, so two admins can't both be deleted at once
/// and leave none.
pub async fn delete_user(pool: &SqlitePool, user_id: i64) -> anyhow::Result<Deleted> {
    let mut tx = pool.begin().await?;
    let is_admin: Option<bool> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(is_admin) = is_admin else {
        return Ok(Deleted::NoSuchUser);
    };
    if is_admin {
        let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE is_admin = 1")
            .fetch_one(&mut *tx)
            .await?;
        if admins <= 1 {
            return Ok(Deleted::LastAdmin);
        }
    }
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Deleted::Done)
}

/// What changing a user did.
#[derive(Debug, PartialEq, Eq)]
pub enum Changed {
    Done(User),
    NoSuchUser,
    /// Someone else has that username (case aside).
    Taken,
    /// Taking admin from the last admin would leave nobody to manage the server.
    LastAdmin,
}

/// Renames a user, or makes them an admin or takes it away. Taking admin away is one
/// statement that checks another admin is left, so two admins demoting each other at once
/// can't leave none. Sessions and API keys carry on: who's an admin is read on every request.
pub async fn change_user(
    pool: &SqlitePool,
    user_id: i64,
    username: Option<&str>,
    is_admin: Option<bool>,
) -> anyhow::Result<Changed> {
    let mut tx = pool.begin().await?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = ?)")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    if !exists {
        return Ok(Changed::NoSuchUser);
    }
    if let Some(username) = username {
        let renamed = sqlx::query("UPDATE users SET username = ? WHERE id = ?")
            .bind(username)
            .bind(user_id)
            .execute(&mut *tx)
            .await;
        match renamed {
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Ok(Changed::Taken);
            }
            other => {
                other?;
            }
        }
    }
    match is_admin {
        Some(true) => {
            sqlx::query("UPDATE users SET is_admin = 1 WHERE id = ?")
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        Some(false) => {
            let done = sqlx::query(
                "UPDATE users SET is_admin = 0 WHERE id = ?1
                   AND (is_admin = 0
                        OR EXISTS (SELECT 1 FROM users WHERE is_admin = 1 AND id <> ?1))",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            if done.rows_affected() == 0 {
                return Ok(Changed::LastAdmin);
            }
        }
        None => {}
    }
    let user = sqlx::query_as("SELECT id, username, is_admin FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Changed::Done(user))
}

/// Issues a full token and returns its id and the token itself, which is never stored.
pub async fn issue_token(
    pool: &SqlitePool,
    user_id: i64,
    kind: TokenKind,
    name: &str,
) -> anyhow::Result<(i64, String)> {
    issue_scoped(pool, user_id, kind, name, Scope::Full).await
}

/// The user a token belongs to, with the token's id, and notes the token as used; requests
/// go through [`token_holder`], which says what the token may reach too.
#[cfg(test)]
pub async fn user_for_token(pool: &SqlitePool, token: &str) -> anyhow::Result<Option<(User, i64)>> {
    Ok(token_holder(pool, token)
        .await?
        .map(|holder| (holder.user, holder.token_id)))
}

/// Limits password attempts per key (the caller picks it: username and address for
/// sign-in, the user for password changes).
///
/// Every attempt is counted *before* its password is checked and forgiven only on success,
/// so a burst of parallel requests gets no more tries than a sequence would.
#[derive(Default)]
pub struct LoginLimiter {
    attempts: Mutex<HashMap<String, (u32, Instant)>>,
}

impl LoginLimiter {
    /// Records an attempt. `false` means the key is locked and the password must not be checked.
    pub fn begin(&self, key: &str) -> bool {
        self.begin_at(key, Instant::now())
    }

    fn begin_at(&self, key: &str, now: Instant) -> bool {
        let mut attempts = self.attempts.lock().expect("no holder panics");
        // Bound the map: forget windows that have expired.
        if attempts.len() > 1000 {
            attempts.retain(|_, (_, since)| now.duration_since(*since) < LOCKOUT);
        }
        let (count, since) = attempts.entry(key.to_owned()).or_insert((0, now));
        if now.duration_since(*since) >= LOCKOUT {
            (*count, *since) = (0, now);
        }
        if *count >= MAX_FAILED_LOGINS {
            return false;
        }
        *count += 1;
        if *count == MAX_FAILED_LOGINS {
            // The lock runs for the full window from the attempt that triggered it.
            *since = now;
        }
        true
    }

    /// Forgets the key's attempts after a right password.
    pub fn succeeded(&self, key: &str) {
        self.attempts.lock().expect("no holder panics").remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn username_rules() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("a.b-c_1").is_ok());
        for bad in [
            "",
            "has space",
            "ünïcode",
            "--admin",
            ".hidden",
            &"x".repeat(65),
        ] {
            assert!(validate_username(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password(&"x".repeat(1025)).is_err());
    }

    #[test]
    fn generated_secrets_look_right() {
        let password = generate_password().unwrap();
        assert_eq!(password.len(), 20);
        assert_ne!(password, generate_password().unwrap());
        let token = generate_token().unwrap();
        assert!(token.starts_with(TOKEN_PREFIX));
        assert_eq!(token.len(), TOKEN_PREFIX.len() + 64);
    }

    #[tokio::test]
    async fn password_check_and_case_insensitive_usernames() {
        let pool = db::open_in_memory().await;
        let user = create_user(&pool, "Alice", "correct horse".into(), true)
            .await
            .unwrap()
            .unwrap();
        assert!(user.is_admin);
        assert!(
            create_user(&pool, "alice", "other password".into(), false)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            check_password(&pool, "ALICE", "correct horse".into())
                .await
                .unwrap(),
            Some(user)
        );
        assert_eq!(
            check_password(&pool, "Alice", "wrong".into())
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            check_password(&pool, "nobody", "correct horse".into())
                .await
                .unwrap(),
            None
        );
        let stored: String = sqlx::query_scalar("SELECT password_hash FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(stored.starts_with("$argon2id$"));
    }

    #[tokio::test]
    async fn tokens_are_stored_hashed_and_resolve_to_their_user() {
        let pool = db::open_in_memory().await;
        let user = create_user(&pool, "a", "password1".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (id, token) = issue_token(&pool, user.id, TokenKind::Session, "iPhone")
            .await
            .unwrap();
        assert_eq!(
            user_for_token(&pool, &token).await.unwrap(),
            Some((user, id))
        );
        assert_eq!(user_for_token(&pool, "mlt_0000").await.unwrap(), None);
        assert_eq!(user_for_token(&pool, "not-a-token").await.unwrap(), None);
        let stored: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_ne!(stored, token.as_bytes());
        let used: Option<i64> = sqlx::query_scalar("SELECT last_used_at FROM tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(used.is_some());
    }

    #[tokio::test]
    async fn new_password_signs_out_other_tokens() {
        let pool = db::open_in_memory().await;
        let user = create_user(&pool, "a", "password1".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (keep, kept) = issue_token(&pool, user.id, TokenKind::Session, "phone")
            .await
            .unwrap();
        let (_, other) = issue_token(&pool, user.id, TokenKind::ApiKey, "script")
            .await
            .unwrap();
        set_password(&pool, user.id, "password2".into(), Some(keep))
            .await
            .unwrap();
        assert!(user_for_token(&pool, &kept).await.unwrap().is_some());
        assert!(user_for_token(&pool, &other).await.unwrap().is_none());
        assert!(
            check_password(&pool, "a", "password2".into())
                .await
                .unwrap()
                .is_some()
        );

        set_password(&pool, user.id, "password3".into(), None)
            .await
            .unwrap();
        assert!(user_for_token(&pool, &kept).await.unwrap().is_none());
    }

    #[test]
    fn limiter_counts_attempts_up_front_and_forgives_success() {
        let limiter = LoginLimiter::default();
        for _ in 0..MAX_FAILED_LOGINS {
            assert!(limiter.begin("viewer|10.0.0.2"));
        }
        assert!(!limiter.begin("viewer|10.0.0.2"));
        assert!(
            limiter.begin("viewer|10.0.0.3"),
            "other keys are unaffected"
        );
        limiter.succeeded("viewer|10.0.0.2");
        assert!(limiter.begin("viewer|10.0.0.2"));
    }

    #[test]
    fn lock_lasts_a_full_window_from_when_it_starts() {
        let limiter = LoginLimiter::default();
        let t0 = Instant::now();
        let minute = Duration::from_secs(60);
        assert!(limiter.begin_at("k", t0));
        for _ in 1..MAX_FAILED_LOGINS {
            assert!(limiter.begin_at("k", t0 + 14 * minute));
        }
        assert!(
            !limiter.begin_at("k", t0 + 16 * minute),
            "still locked after the first window"
        );
        assert!(!limiter.begin_at("k", t0 + 28 * minute));
        assert!(
            limiter.begin_at("k", t0 + 29 * minute),
            "unlocked 15 minutes after the lock began"
        );
    }

    #[tokio::test]
    async fn sessions_unused_for_90_days_sign_out_but_api_keys_dont() {
        let pool = db::open_in_memory().await;
        let user = create_user(&pool, "alice", "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        let (old_session, old_token) = issue_token(&pool, user.id, TokenKind::Session, "old phone")
            .await
            .unwrap();
        let (_, recent_token) = issue_token(&pool, user.id, TokenKind::Session, "tv")
            .await
            .unwrap();
        let (old_key, key_token) = issue_token(&pool, user.id, TokenKind::ApiKey, "script")
            .await
            .unwrap();
        let now = unix_now();
        for (id, last_used) in [
            (old_session, now - SESSION_IDLE - 60),
            (old_key, now - SESSION_IDLE - 60),
        ] {
            sqlx::query("UPDATE tokens SET last_used_at = ? WHERE id = ?")
                .bind(last_used)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert!(user_for_token(&pool, &old_token).await.unwrap().is_none());
        let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens WHERE id = ?")
            .bind(old_session)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(gone, 0, "the expired session is removed");
        assert!(
            user_for_token(&pool, &recent_token)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            user_for_token(&pool, &key_token).await.unwrap().is_some(),
            "API keys don't expire"
        );

        // A session never used counts from when it was made; stale ones go at the next sign-in.
        let (never_used, _) = issue_token(&pool, user.id, TokenKind::Session, "tablet")
            .await
            .unwrap();
        sqlx::query("UPDATE tokens SET created_at = ?, last_used_at = NULL WHERE id = ?")
            .bind(now - SESSION_IDLE - 60)
            .bind(never_used)
            .execute(&pool)
            .await
            .unwrap();
        issue_token(&pool, user.id, TokenKind::Session, "new phone")
            .await
            .unwrap();
        // By name: SQLite gives a deleted top id to the next row.
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM tokens WHERE name = 'tablet'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }
}
