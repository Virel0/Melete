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
    /// The CA directory, domain and challenge those failures were for ([`attempt_for`]).
    pub failed_for: Option<String>,
    /// When admins were last told renewing fails.
    pub alerted_at: Option<i64>,
    /// When an admin last asked for a certificate now.
    pub asked_at: Option<i64>,
}

/// What failures are kept for: the CA directory, the domain, and the challenge when it isn't
/// DNS's (written as before for DNS, so its failures kept from before still count).
pub fn attempt_for(directory: &str, domain: &str, challenge: crate::config::Challenge) -> String {
    match challenge {
        crate::config::Challenge::Dns => format!("{directory} {domain}"),
        crate::config::Challenge::TlsAlpn => format!("{directory} {domain} tls-alpn"),
    }
}

impl State {
    /// Forgets failures made for another CA directory, domain or challenge than `now_for`:
    /// their waits and alerts aren't this one's.
    pub fn forget_failures_unless_for(&mut self, now_for: &str) {
        if self.failures > 0 && self.failed_for.as_deref() != Some(now_for) {
            self.failures = 0;
            self.retry_at = None;
            self.last_error = None;
            self.failed_for = None;
            self.alerted_at = None;
        }
    }
}

const DAY: i64 = 24 * 60 * 60;

/// `at`, kept to what makes sense for a certificate valid from `not_before` to `not_after`:
/// not within a tenth of its life (a day at most) of its start, so a window in the past can't
/// have a new certificate replaced at once again and again, and a third of its life (three days
/// at most) before it runs out at the latest.
pub fn sensible(at: i64, not_before: i64, not_after: i64) -> i64 {
    let life = (not_after - not_before).max(0);
    let latest = not_after - (life / 3).min(3 * DAY);
    let earliest = (not_before + (life / 10).min(DAY)).min(latest);
    at.clamp(earliest, latest)
}

/// How long after asking for a certificate now an admin may ask again, and how young a
/// certificate from the same CA for the same names may be before asking is refused.
pub const ASK_AGAIN_AFTER: i64 = 15 * 60;
pub const YOUNGEST_REPLACED: i64 = DAY;

/// Why asking for a certificate now is refused, if it is: one being got already, one asked for
/// lately, or the one held being from this CA, for these names, and new. Each request asks the
/// CA for a certificate, and its limits on duplicates would then block real renewals.
pub fn ask_refused(
    state: &State,
    held: Option<(&super::files::Held, bool)>,
    working: bool,
    now: i64,
) -> Option<String> {
    if working {
        return Some("Melete is getting a certificate right now.".to_owned());
    }
    if let Some(at) = state.asked_at.filter(|at| now - at < ASK_AGAIN_AFTER) {
        return Some(format!(
            "A certificate was asked for {} minutes ago; ask again in {} minutes.",
            (now - at) / 60,
            (ASK_AGAIN_AFTER - (now - at) + 59) / 60
        ));
    }
    match held {
        Some((held, true)) if now - held.not_before < YOUNGEST_REPLACED => Some(
            "The certificate in use is less than a day old, from this server and for these names: asking again only counts against its limits on duplicates.".to_owned(),
        ),
        _ => None,
    }
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
        "SELECT directory, renew_at, ask_again_at, failures, retry_at, last_error, failed_for,
             alerted_at, asked_at
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
             last_error, failed_for, alerted_at, asked_at, updated_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET directory = excluded.directory,
             renew_at = excluded.renew_at, ask_again_at = excluded.ask_again_at,
             failures = excluded.failures, retry_at = excluded.retry_at,
             last_error = excluded.last_error, failed_for = excluded.failed_for,
             alerted_at = excluded.alerted_at, asked_at = excluded.asked_at,
             updated_at = excluded.updated_at",
    )
    .bind(&state.directory)
    .bind(state.renew_at)
    .bind(state.ask_again_at)
    .bind(state.failures)
    .bind(state.retry_at)
    .bind(error)
    .bind(&state.failed_for)
    .bind(state.alerted_at)
    .bind(state.asked_at)
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
    fn a_renewal_time_stays_within_the_certificates_life() {
        let (start, end) = (1_000_000, 1_000_000 + 90 * DAY);
        assert_eq!(
            sensible(start - 5 * DAY, start, end),
            start + DAY,
            "not at once"
        );
        assert_eq!(
            sensible(end + DAY, start, end),
            end - 3 * DAY,
            "before it runs out"
        );
        assert_eq!(sensible(start + 40 * DAY, start, end), start + 40 * DAY);
        // A six-day certificate: a tenth and a third of its life.
        let short = start + 6 * DAY;
        assert_eq!(sensible(0, start, short), start + 6 * DAY / 10);
        assert_eq!(sensible(i64::MAX, start, short), short - 2 * DAY);
    }

    #[test]
    fn retries_wait_longer_each_time_up_to_half_a_day() {
        assert_eq!(backoff(1), 600);
        assert_eq!(backoff(2), 1800);
        assert!(backoff(3) > backoff(2));
        assert_eq!(backoff(50), 12 * 60 * 60);
    }
}
