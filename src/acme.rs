//! HTTPS's certificate, got and renewed by Melete from an ACME CA (Let's Encrypt) when
//! Certificates is on under Outside services; off by default. The DNS challenge proves the
//! domain through deSEC, so the certificate is a wildcard (`*.example.org`, with the bare name)
//! and Certificate Transparency logs list no other name. It's written where HTTPS reads it
//! (`files`), and renewed in the window the CA's renewal information suggests, else two-thirds
//! through its life (`renewal`); a renewal that fails is tried again later, and admins are
//! alerted, saying when the one in use runs out, and told again once it's renewed.
//!
//! What leaves the server: the domain's names, to the CA and to deSEC; the challenge's TXT
//! values, to deSEC (and so to anyone who looks the record up while it's there); the deSEC
//! token, to deSEC alone; the account's public key and signed requests, and an email address
//! only when one is set, to the CA. The account's key is kept sealed (`account`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;

use sqlx::SqlitePool;

use crate::alerts::Alert;
use crate::config::Acme;
use crate::db::unix_now;

pub mod account;
mod client;
pub mod files;
pub mod issue;
mod left;
pub mod renewal;
mod stage;
mod status;
pub mod test;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests;

pub use issue::Reach;
pub use status::{Status, ask, status};

/// Wakes the renewing loop: settings changed, or an admin asked for a certificate now.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
/// An admin asked for a certificate now, whether one is due or not.
static ASKED: AtomicBool = AtomicBool::new(false);
/// Whether an attempt is running.
static WORKING: AtomicBool = AtomicBool::new(false);

/// Has the loop look again at once (its settings were saved).
pub fn wake() {
    WAKE.notify_waiters();
}

/// Has the loop get a certificate now (once [`ask`] allows it).
fn ask_now() {
    ASKED.store(true, Ordering::SeqCst);
    WAKE.notify_waiters();
}

/// Whether an attempt is running now.
pub fn working() -> bool {
    WORKING.load(Ordering::SeqCst)
}

/// How long the loop sleeps at most, and when there's nothing to do.
const LONGEST_SLEEP: i64 = 6 * 60 * 60;
/// How often the CA's renewal information is asked at most and least.
const ASK_AT_MOST: i64 = 60 * 60;
const ASK_AT_LEAST: i64 = 24 * 60 * 60;

/// Gets and renews the certificate for as long as the server runs, as the settings stand each
/// time it looks.
pub async fn run(pool: SqlitePool, data_dir: PathBuf) {
    let folder = crate::config::acme_folder(&data_dir);
    loop {
        let asked = ASKED.swap(false, Ordering::SeqCst);
        let next = match crate::settings::load(&pool).await {
            Ok(config) => match &config.acme {
                Some(acme) => match round(&pool, &folder, acme, &Reach::desec(), asked).await {
                    Ok(next) => next,
                    Err(err) => {
                        tracing::warn!(
                            error = format!("{err:#}"),
                            "renewing HTTPS's certificate stopped short"
                        );
                        unix_now() + 600
                    }
                },
                None => unix_now() + LONGEST_SLEEP,
            },
            Err(err) => {
                tracing::warn!(
                    error = format!("{err:#}"),
                    "couldn't read the certificate's settings"
                );
                unix_now() + 600
            }
        };
        let wait = (next - unix_now()).clamp(30, LONGEST_SLEEP);
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs(wait.unsigned_abs())) => {}
            () = WAKE.notified() => {}
        }
    }
}

/// One look: renews the certificate in `folder` if it's due (or `asked`), else works out when it
/// will be. When to look next.
pub async fn round(
    pool: &SqlitePool,
    folder: &Path,
    acme: &Acme,
    reach: &Reach,
    asked: bool,
) -> anyhow::Result<i64> {
    let now = unix_now();
    if let Err(err) = left::take_out(pool, acme, reach).await {
        tracing::warn!(
            error = format!("{err:#}"),
            "couldn't take out challenge values left set"
        );
    }
    let mut state = renewal::load(pool).await?;
    let attempt_for = renewal::attempt_for(&acme.directory, &acme.domain);
    state.forget_failures_unless_for(&attempt_for);
    let held = files::read(folder).filter(|h| h.not_after > now);
    let fits = held.as_ref().filter(|h| {
        h.is_for(&acme.names()) && state.directory.as_deref() == Some(acme.directory.as_str())
    });
    // Asking now doesn't skip the wait once attempts keep failing.
    if let Some(retry_at) = state
        .retry_at
        .filter(|at| *at > now && (!asked || state.failures >= 2))
    {
        return Ok(retry_at);
    }
    if let (Some(held), false) = (fits, asked) {
        if state.ask_again_at.is_none_or(|at| at <= now) {
            schedule(pool, acme, held, &mut state, now).await;
            renewal::save(pool, &state).await?;
        }
        let renew_at = state
            .renew_at
            .unwrap_or_else(|| renewal::two_thirds(held.not_before, held.not_after));
        if renew_at > now {
            return Ok(renew_at.min(state.ask_again_at.unwrap_or(renew_at)));
        }
    }
    let replaces = fits.and_then(|h| h.id.clone());
    // Asked while this attempt runs is asked for by this attempt.
    ASKED.store(false, Ordering::SeqCst);
    WORKING.store(true, Ordering::SeqCst);
    let got = obtain(pool, folder, acme, reach, replaces).await;
    WORKING.store(false, Ordering::SeqCst);
    match got {
        Ok(()) => {
            succeeded(pool, folder, acme, &mut state).await?;
            Ok(now + 60)
        }
        Err(err) => {
            state.failed_for = Some(attempt_for);
            failed(pool, acme, held.as_ref(), &mut state, &err, now).await?;
            Ok(state.retry_at.unwrap_or(now + 600))
        }
    }
}

/// Works out when to renew `held`: in the window the CA's renewal information gives, keeping a
/// time already picked in it, else two-thirds through its life.
async fn schedule(
    pool: &SqlitePool,
    acme: &Acme,
    held: &files::Held,
    state: &mut renewal::State,
    now: i64,
) {
    let asked = match &held.id {
        None => Err(anyhow::anyhow!("the certificate names no authority key")),
        Some(id) => match account::get(pool, &acme.directory, acme.email.as_deref()).await {
            Ok(account) => account.renewal_info(id).await.map_err(anyhow::Error::from),
            Err(err) => Err(err),
        },
    };
    match asked {
        Ok((info, ask_after)) => {
            let start = info.suggested_window.start.unix_timestamp();
            let end = info.suggested_window.end.unix_timestamp();
            let kept = state.renew_at.filter(|at| (start..=end).contains(at));
            let picked =
                kept.unwrap_or_else(|| renewal::in_window(start, end, renewal::random_pick()));
            state.renew_at = Some(renewal::sensible(picked, held.not_before, held.not_after));
            let after = i64::try_from(ask_after.as_secs()).unwrap_or(ASK_AT_LEAST);
            state.ask_again_at = Some(now + after.clamp(ASK_AT_MOST, ASK_AT_LEAST));
        }
        Err(err) => {
            tracing::debug!(
                error = format!("{err:#}"),
                "no renewal information from the CA"
            );
            state.renew_at = Some(renewal::two_thirds(held.not_before, held.not_after));
            state.ask_again_at = Some(now + LONGEST_SLEEP);
        }
    }
}

/// Gets a certificate for `acme`'s names and keeps it in `folder`.
async fn obtain(
    pool: &SqlitePool,
    folder: &Path,
    acme: &Acme,
    reach: &Reach,
    replaces: Option<instant_acme::CertificateIdentifier<'static>>,
) -> anyhow::Result<()> {
    let account = account::get(pool, &acme.directory, acme.email.as_deref())
        .await
        .context(stage::Stage::Account)?;
    let issued = issue::issue(pool, &account, acme, reach, replaces).await?;
    files::write(folder, &issued.chain, &issued.key).context(stage::Stage::Keep)?;
    tracing::info!(
        domain = acme.domain,
        staging = acme.staging,
        "got HTTPS's certificate"
    );
    Ok(())
}

/// A day as YYYY-MM-DD, and how many days from `now` it is.
fn when(day: i64, now: i64) -> String {
    format!(
        "{} (in {} days)",
        crate::follow::date(day),
        (day - now).max(0) / 86_400
    )
}

/// After a certificate was got: renewing starts afresh, and admins told of failures hear it's
/// done.
async fn succeeded(
    pool: &SqlitePool,
    folder: &Path,
    acme: &Acme,
    state: &mut renewal::State,
) -> anyhow::Result<()> {
    let was_alerted = state.alerted_at.is_some();
    *state = renewal::State {
        directory: Some(acme.directory.clone()),
        ..renewal::State::default()
    };
    renewal::save(pool, state).await?;
    if was_alerted {
        let until = files::read(folder).map_or(0, |h| h.not_after);
        let alert = Alert {
            title: format!("HTTPS's certificate for {} is renewed", acme.domain),
            message: format!(
                "Melete got a new certificate for {}. It runs out on {}.",
                acme.names().join(" and "),
                crate::follow::date(until)
            ),
        };
        crate::alerts::send(pool, &alert).await;
    }
    Ok(())
}

/// After an attempt failed: tried again later, and admins alerted unless they were lately.
async fn failed(
    pool: &SqlitePool,
    acme: &Acme,
    held: Option<&files::Held>,
    state: &mut renewal::State,
    err: &anyhow::Error,
    now: i64,
) -> anyhow::Result<()> {
    let why = format!("{err:#}");
    tracing::warn!(error = why, "couldn't get HTTPS's certificate");
    state.failures += 1;
    let retry_at = now + renewal::backoff(state.failures);
    state.retry_at = Some(retry_at);
    state.last_error = Some(why);
    let alert = state
        .alerted_at
        .is_none_or(|at| now - at >= renewal::ALERT_AGAIN_AFTER);
    if alert {
        state.alerted_at = Some(now);
    }
    renewal::save(pool, state).await?;
    if alert {
        let next = format!("next in about {} minutes", (retry_at - now) / 60);
        let alert = match held {
            Some(held) => Alert {
                title: format!("HTTPS's certificate for {} isn't renewing", acme.domain),
                message: format!(
                    "Melete couldn't renew the certificate for {}: {}. The one in use runs out on {}. Melete tries again by itself, {next}; Outside services, Certificates says more.",
                    acme.names().join(" and "),
                    stage::said(err),
                    when(held.not_after, now),
                ),
            },
            None => Alert {
                title: format!(
                    "Melete couldn't get HTTPS's certificate for {}",
                    acme.domain
                ),
                message: format!(
                    "Melete couldn't get a certificate for {}: {}. HTTPS has no valid certificate until it does. Melete tries again by itself, {next}; Outside services, Certificates says more.",
                    acme.names().join(" and "),
                    stage::said(err),
                ),
            },
        };
        crate::alerts::send(pool, &alert).await;
    }
    Ok(())
}
