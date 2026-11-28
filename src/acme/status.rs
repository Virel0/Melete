//! How getting HTTPS's certificate stands, for the Certificates page: the CA, the account's URI
//! (not a secret: it's what a CAA record's accounturi names, so only this account may have
//! certificates issued for the domain), the certificate in use, and renewing. Never a key or the
//! deSEC token.

use serde::Serialize;
use sqlx::SqlitePool;

use super::{account, files, renewal};
use crate::config::Config;

/// The certificate in use.
#[derive(Debug, Serialize)]
pub struct Held {
    /// The names it's for.
    pub names: Vec<String>,
    /// When it became valid and when it runs out, Unix seconds.
    pub not_before: i64,
    pub not_after: i64,
    /// Whether it came from the CA now set (not the staging server's, say, after a switch).
    pub from_this_ca: bool,
}

/// How it stands.
#[derive(Debug, Serialize)]
pub struct Status {
    /// Whether Certificates is on.
    pub on: bool,
    /// The names asked for; empty while it's off.
    pub names: Vec<String>,
    /// Whether the CA's staging server is used, whose certificates no browser trusts.
    pub staging: bool,
    /// The account's URI on that CA, once there's one.
    pub account_uri: Option<String>,
    /// CAA records that let only this account have certificates issued for the domain.
    pub caa: Vec<String>,
    /// The certificate in use, when there's one.
    pub certificate: Option<Held>,
    /// When it's renewed, when an attempt that failed is tried again, how many failed in a
    /// row, and why the last one did.
    pub renew_at: Option<i64>,
    pub retry_at: Option<i64>,
    pub failures: i64,
    pub last_error: Option<String>,
    /// Whether an attempt is running now.
    pub working: bool,
    /// Why getting a certificate now is refused, when it is: one being got, one asked for in the
    /// last 15 minutes, or the one in use new and from this server.
    pub ask_refused: Option<String>,
    /// Whether HTTPS is set to listen (`MELETE_TLS_BIND`), serving this certificate.
    pub https: bool,
}

/// How it stands with `config`.
pub async fn status(pool: &SqlitePool, config: &Config) -> anyhow::Result<Status> {
    let state = renewal::load(pool).await?;
    let Some(acme) = &config.acme else {
        return Ok(Status {
            on: false,
            names: Vec::new(),
            staging: true,
            account_uri: None,
            caa: Vec::new(),
            certificate: None,
            renew_at: None,
            retry_at: None,
            failures: 0,
            last_error: None,
            working: false,
            ask_refused: Some("Certificates isn't set up.".to_owned()),
            https: config.tls.is_some(),
        });
    };
    let account_uri = account::uri(pool, &acme.directory).await?;
    let caa = account_uri
        .iter()
        .flat_map(|uri| {
            ["issue", "issuewild"].map(|tag| {
                format!(
                    "{}. CAA 0 {tag} \"letsencrypt.org; accounturi={uri}\"",
                    acme.domain
                )
            })
        })
        .collect();
    let from_this_ca = state.directory.as_deref() == Some(acme.directory.as_str());
    let held = files::read(&crate::config::acme_folder(&config.data_dir));
    let ask_refused = refused(&state, held.as_ref(), acme, from_this_ca);
    let certificate = held.map(|h| Held {
        names: h.names,
        not_before: h.not_before,
        not_after: h.not_after,
        from_this_ca,
    });
    Ok(Status {
        on: true,
        names: acme.names(),
        staging: acme.staging,
        account_uri,
        caa,
        certificate,
        renew_at: state.renew_at,
        retry_at: state.retry_at.filter(|_| state.failures > 0),
        failures: state.failures,
        last_error: state.last_error,
        working: super::working(),
        ask_refused,
        https: config.tls.is_some(),
    })
}

/// Why getting a certificate now is refused as things stand.
fn refused(
    state: &renewal::State,
    held: Option<&files::Held>,
    acme: &crate::config::Acme,
    from_this_ca: bool,
) -> Option<String> {
    let held = held.map(|h| (h, from_this_ca && h.is_for(&acme.names())));
    renewal::ask_refused(state, held, super::working(), crate::db::unix_now())
}

/// Has a certificate got now, unless that's refused: then why, in plain words.
pub async fn ask(pool: &SqlitePool, config: &Config) -> anyhow::Result<Result<(), String>> {
    let Some(acme) = &config.acme else {
        return Ok(Err(
            "Set Certificates up under Outside services first.".to_owned()
        ));
    };
    let mut state = renewal::load(pool).await?;
    let from_this_ca = state.directory.as_deref() == Some(acme.directory.as_str());
    let held = files::read(&crate::config::acme_folder(&config.data_dir));
    if let Some(why) = refused(&state, held.as_ref(), acme, from_this_ca) {
        return Ok(Err(why));
    }
    state.asked_at = Some(crate::db::unix_now());
    renewal::save(pool, &state).await?;
    super::ask_now();
    Ok(Ok(()))
}
