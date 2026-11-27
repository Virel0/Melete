//! When to renew, and where renewing stands. A certificate is renewed at a time picked at
//! random in the window the CA's renewal information (ARI) suggests, which the CA may move
//! earlier (to replace certificates it must revoke, say); when the CA gives none, two-thirds
//! through its life. An attempt that fails is tried again later and later, from 10 minutes to 12
//! hours apart. All of it is kept in the database, so a restart picks up where it was.

use sqlx::SqlitePool;

use crate::db::unix_now;

/// Where renewing stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct State {
    /// The CA directory the certificate in use came from.
    pub directory: Option<String>,
    /// When to renew it.
    pub renew_at: Option<i64>,
    /// When to ask the CA's renewal information again.
    pub ask_again_at: Option<i64>,
    /// Attempts that failed in a row, when to try next, and why the last one failed.
    pub failures: i64,
    pub retry_at: Option<i64>,
    pub last_error: Option<String>,
    /// When admins were last told renewing fails.
    pub alerted_at: Option<i64>,
}

/// Two-thirds through a certificate's life.
pub fn two_thirds(not_before: i64, not_after: i64) -> i64 {
    not_before + (not_after - not_before).max(0) * 2 / 3
}

/// A time in `[start, end]`, `pick` of the way through (`pick` in 0..=u32::MAX).
pub fn in_window(start: i64, end: i64, pick: u32) -> i64 {
    let span = (end - start).max(0);
    let offset = i128::from(span) * i128::from(pick) / i128::from(u32::MAX);
    start + i64::try_from(offset).unwrap_or(span)
}

/// A random point for [`in_window`].
pub fn random_pick() -> u32 {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_or(u32::MAX / 2, |()| u32::from_be_bytes(bytes))
}

/// How long after the `failures`th failure in a row to try again, in seconds.
pub fn backoff(failures: i64) -> i64 {
    let mut wait: i64 = 10 * 60;
    for _ in 1..failures.min(8) {
        wait *= 3;
    }
    wait.min(12 * 60 * 60)
}

/// How long after an alert that renewing fails to say so again while it still fails.
pub const ALERT_AGAIN_AFTER: i64 = 3 * 24 * 60 * 60;

/// Where renewing stands now; nothing yet when it never ran.
pub async fn load(pool: &SqlitePool) -> sqlx::Result<State> {
    Ok(sqlx::query_as(
        "SELECT directory, renew_at, ask_again_at, failures, retry_at, last_error, alerted_at
         FROM acme_renewal WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or_default())
}

/// Keeps `state`.
pub async fn save(pool: &SqlitePool, state: &State) -> sqlx::Result<()> {
    let error = state
        .last_error
        .as_ref()
        .map(|e| e.chars().take(1000).collect::<String>());
    sqlx::query(
        "INSERT INTO acme_renewal (id, directory, renew_at, ask_again_at, failures, retry_at,
             last_error, alerted_at, updated_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET directory = excluded.directory,
             renew_at = excluded.renew_at, ask_again_at = excluded.ask_again_at,
             failures = excluded.failures, retry_at = excluded.retry_at,
             last_error = excluded.last_error, alerted_at = excluded.alerted_at,
             updated_at = excluded.updated_at",
    )
    .bind(&state.directory)
    .bind(state.renew_at)
    .bind(state.ask_again_at)
    .bind(state.failures)
    .bind(state.retry_at)
    .bind(error)
    .bind(state.alerted_at)
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renewing_falls_in_the_window_or_two_thirds_through() {
        assert_eq!(two_thirds(0, 90), 60);
        assert_eq!(in_window(100, 200, 0), 100);
        assert_eq!(in_window(100, 200, u32::MAX), 200);
        let middle = in_window(100, 200, u32::MAX / 2);
        assert!((149..=150).contains(&middle), "{middle}");
        assert_eq!(
            in_window(100, 50, 7),
            100,
            "a window backwards is its start"
        );
    }

    #[test]
    fn retries_wait_longer_each_time_up_to_half_a_day() {
        assert_eq!(backoff(1), 600);
        assert_eq!(backoff(2), 1800);
        assert!(backoff(3) > backoff(2));
        assert_eq!(backoff(50), 12 * 60 * 60);
    }
}
