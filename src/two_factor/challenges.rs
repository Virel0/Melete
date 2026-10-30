//! Sign-ins waiting for their code: a challenge is given for a right password, and finished
//! with a code once.

use super::*;
use crate::auth;

/// How long a sign-in waits for its code, in seconds.
pub const CHALLENGE_LIFETIME: i64 = 5 * 60;
/// What every challenge starts with.
pub const CHALLENGE_PREFIX: &str = "m2f_";

/// A challenge for a sign-in whose password was right, to finish with a code within
/// [`CHALLENGE_LIFETIME`]. Only its hash is kept. Challenges past their time go now.
pub async fn start_challenge(
    pool: &SqlitePool,
    user_id: i64,
    device_name: &str,
    for_web: bool,
) -> anyhow::Result<String> {
    let now = unix_now();
    sqlx::query("DELETE FROM sign_in_challenges WHERE expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    let challenge = auth::random_token(CHALLENGE_PREFIX)?;
    sqlx::query(
        "INSERT INTO sign_in_challenges (challenge_hash, user_id, device_name, for_web, expires_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(auth::token_hash(&challenge))
    .bind(user_id)
    .bind(device_name)
    .bind(for_web)
    .bind(now + CHALLENGE_LIFETIME)
    .execute(pool)
    .await?;
    Ok(challenge)
}

/// The user a challenge still waiting is for, when it was started the same way (`for_web`).
pub async fn challenged(
    pool: &SqlitePool,
    challenge: &str,
    for_web: bool,
    now: i64,
) -> sqlx::Result<Option<i64>> {
    if !challenge.starts_with(CHALLENGE_PREFIX) {
        return Ok(None);
    }
    sqlx::query_scalar(
        "SELECT user_id FROM sign_in_challenges
         WHERE challenge_hash = ? AND for_web = ? AND expires_at > ?",
    )
    .bind(auth::token_hash(challenge))
    .bind(for_web)
    .bind(now)
    .fetch_optional(pool)
    .await
}

/// Uses a challenge up, once its code was right: the device name it was started with, unless
/// it's gone or ran out meanwhile.
pub async fn finish_challenge(
    pool: &SqlitePool,
    challenge: &str,
    now: i64,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "DELETE FROM sign_in_challenges WHERE challenge_hash = ? AND expires_at > ?
         RETURNING device_name",
    )
    .bind(auth::token_hash(challenge))
    .bind(now)
    .fetch_optional(pool)
    .await
}
