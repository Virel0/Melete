//! The ACME account, one per CA directory: made the first time it's needed (its key from the
//! OS's randomness, through aws-lc-rs), kept sealed in the database, and used again after.
//! Making one agrees to the CA's terms. Its contact is the email set, or none; a change of
//! address is sent to the CA. One kept that can't be opened (the secrets key changed) is made
//! again, as a new account. No credential is logged or put in an error.

use instant_acme::{Account, AccountCredentials, NewAccount};
use sqlx::SqlitePool;

use crate::secrets;

/// What an account's credentials are bound to when sealed: its table and directory, so a row
/// moved to another directory doesn't open.
fn sealed_as(directory: &str) -> String {
    format!("acme_accounts.credentials.{directory}")
}

/// The account's URI for `directory`, when there's one: what a CAA record's accounturi names.
pub async fn uri(pool: &SqlitePool, directory: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT uri FROM acme_accounts WHERE directory = ?")
        .bind(directory)
        .fetch_optional(pool)
        .await
}

/// The contacts an account takes for `email`.
fn contacts(email: Option<&str>) -> Vec<String> {
    email.map(|e| format!("mailto:{e}")).into_iter().collect()
}

/// The account on `directory`, kept or made now, with `email` as its contact.
pub async fn get(
    pool: &SqlitePool,
    directory: &str,
    email: Option<&str>,
) -> anyhow::Result<Account> {
    let sealing = secrets::sealing(pool).await?;
    let kept: Option<(String, String)> =
        sqlx::query_as("SELECT credentials, contact FROM acme_accounts WHERE directory = ?")
            .bind(directory)
            .fetch_optional(pool)
            .await?;
    let wanted = email.unwrap_or("");
    if let Some((stored, contact)) = kept {
        match secrets::from_store(sealing, &sealed_as(directory), &stored)
            .ok()
            .and_then(|json| serde_json::from_str::<AccountCredentials>(&json).ok())
        {
            Some(credentials) => {
                let account = Account::builder_with_http(super::client::Client::boxed()?)
                    .from_credentials(credentials)
                    .await?;
                if contact != wanted {
                    let made = contacts(email);
                    let said: Vec<&str> = made.iter().map(String::as_str).collect();
                    account.update_contacts(&said).await?;
                    sqlx::query("UPDATE acme_accounts SET contact = ? WHERE directory = ?")
                        .bind(wanted)
                        .bind(directory)
                        .execute(pool)
                        .await?;
                }
                return Ok(account);
            }
            None => tracing::warn!(
                "the ACME account kept can't be opened with this secrets key; making a new one"
            ),
        }
    }
    let made = contacts(email);
    let said: Vec<&str> = made.iter().map(String::as_str).collect();
    let (account, credentials) = Account::builder_with_http(super::client::Client::boxed()?)
        .create(
            &NewAccount {
                contact: &said,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory.to_owned(),
            None,
        )
        .await?;
    let json = serde_json::to_string(&credentials)?;
    sqlx::query(
        "INSERT INTO acme_accounts (directory, uri, credentials, contact, created_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (directory) DO UPDATE SET uri = excluded.uri,
             credentials = excluded.credentials, contact = excluded.contact,
             created_at = excluded.created_at",
    )
    .bind(directory)
    .bind(account.id())
    .bind(secrets::to_store(sealing, &sealed_as(directory), &json)?)
    .bind(wanted)
    .bind(crate::db::unix_now())
    .execute(pool)
    .await?;
    tracing::info!(uri = account.id(), "made an ACME account");
    Ok(account)
}
