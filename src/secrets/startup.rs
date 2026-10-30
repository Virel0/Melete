//! The server's start: the key is read (made when there's none and nothing needs one), every
//! stored secret is checked to open with it, and a database not switched yet has its plain
//! secrets sealed, once. A secret that can't be read stops the start, saying which and how to
//! recover; `melete secrets clear` is one way back.

use std::path::Path;

use anyhow::bail;
use sqlx::SqlitePool;

use super::{INDEXER_KEY, Key, Unreadable, key_file, opened_bytes, opened_text, sealing};
use crate::db::unix_now;
use crate::settings;

/// How long a two-factor secret is, plain: 160 bits.
pub const TWO_FACTOR_LEN: usize = 20;

/// Stored secrets that can't be read, by where they're kept. Holds names, never values.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unreadables {
    /// Settings, by variable name, with why.
    pub settings: Vec<(String, Unreadable)>,
    /// Indexers' keys, by indexer id and name, with why.
    pub indexers: Vec<(i64, String, Unreadable)>,
    /// Users whose two-factor secret can't be read, by name, with why.
    pub two_factor: Vec<(String, Unreadable)>,
    /// Search results whose download link or guid can't be read, by id.
    pub links: Vec<i64>,
}

impl Unreadables {
    /// Whether every stored secret can be read.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty()
            && self.indexers.is_empty()
            && self.two_factor.is_empty()
            && self.links.is_empty()
    }
}

/// Reads the key in `data_dir` (made there when there's none and nothing is sealed), checks
/// every stored secret opens with it, and seals the plain ones of a database not switched yet.
/// Fails, saying which secrets and how to recover, when one can't be read.
pub async fn prepare(pool: &SqlitePool, data_dir: &Path) -> anyhow::Result<Key> {
    let path = data_dir.join(super::KEY_FILE);
    let found = key_file::read(&path)?;
    let unreadable = unreadable(pool, found.as_ref()).await?;
    if !unreadable.is_empty() {
        bail!(
            "{}",
            super::recovery::refusal(&path, found.is_none(), &unreadable)
        );
    }
    let key = match found {
        Some(key) => key,
        None => {
            let key = key_file::create(&path)?;
            tracing::info!(
                file = %path.display(),
                "made the key stored secrets are sealed with; back it up apart from the database"
            );
            key
        }
    };
    if let Some(sealed) = switch(pool, &key).await? {
        tracing::info!(
            sealed,
            "stored secrets are sealed from now on; keep secrets.key, and back it up apart from the database"
        );
    }
    Ok(key)
}

/// The stored secrets `key` can't open (all sealed ones, without a key), and in a database that
/// keeps them sealed, any kept plain.
pub async fn unreadable(pool: &SqlitePool, key: Option<&Key>) -> sqlx::Result<Unreadables> {
    let sealing = sealing(pool).await?;
    let mut found = Unreadables::default();
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, value FROM settings")
        .fetch_all(pool)
        .await?;
    for (name, value) in rows.into_iter().filter(|(n, _)| settings::is_secret(n)) {
        if let Err(why) = opened_text(key, sealing, &super::setting(&name), &value) {
            found.settings.push((name, why));
        }
    }
    let rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, name, api_key FROM indexers ORDER BY name, id")
            .fetch_all(pool)
            .await?;
    for (id, name, api_key) in rows {
        if let Err(why) = opened_text(key, sealing, INDEXER_KEY, &api_key) {
            found.indexers.push((id, name, why));
        }
    }
    let rows: Vec<(i64, String, Vec<u8>)> = sqlx::query_as(
        "SELECT t.user_id, u.username, t.secret FROM two_factor t JOIN users u ON u.id = t.user_id
         ORDER BY u.username",
    )
    .fetch_all(pool)
    .await?;
    for (user_id, username, secret) in rows {
        let what = super::two_factor(user_id);
        if let Err(why) = opened_bytes(key, sealing, &what, &secret, TWO_FACTOR_LEN) {
            found.two_factor.push((username, why));
        }
    }
    found.links = unreadable_links(pool, key, sealing).await?;
    Ok(found)
}

/// The search results whose download link or guid `key` can't open.
async fn unreadable_links(
    pool: &SqlitePool,
    key: Option<&Key>,
    sealing: bool,
) -> sqlx::Result<Vec<i64>> {
    let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, download_url, guid FROM release_decisions
         WHERE download_url != '' OR guid != '' ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let opens = |what: &str, link: &Option<String>| {
        link.as_deref()
            .is_none_or(|link| opened_text(key, sealing, what, link).is_ok())
    };
    Ok(rows
        .into_iter()
        .filter(|(_, url, guid)| !opens(super::DOWNLOAD_URL, url) || !opens(super::GUID, guid))
        .map(|(id, _, _)| id)
        .collect())
}

/// Switches the database to sealed secrets: seals each one kept plain with `key` and marks it
/// switched, together. How many were sealed; None when it was switched already.
pub async fn switch(pool: &SqlitePool, key: &Key) -> anyhow::Result<Option<usize>> {
    let mut tx = pool.begin().await?;
    if sealing(&mut *tx).await? {
        return Ok(None);
    }
    let sealed = seal_settings(&mut tx, key).await?
        + seal_indexers(&mut tx, key).await?
        + seal_two_factor(&mut tx, key).await?
        + seal_links(&mut tx, key).await?;
    sqlx::query("INSERT INTO secrets_sealed (id, sealed_at) VALUES (1, ?)")
        .bind(unix_now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(sealed))
}

/// The open transaction a switch runs in.
type Switching = sqlx::Transaction<'static, sqlx::Sqlite>;

/// Seals the secret settings kept plain; how many.
async fn seal_settings(tx: &mut Switching, key: &Key) -> anyhow::Result<usize> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, value FROM settings")
        .fetch_all(&mut **tx)
        .await?;
    let mut sealed = 0;
    for (name, value) in rows {
        if settings::is_secret(&name) && !super::is_sealed_text(&value) {
            sqlx::query("UPDATE settings SET value = ? WHERE name = ?")
                .bind(key.seal_text(&super::setting(&name), &value)?)
                .bind(&name)
                .execute(&mut **tx)
                .await?;
            sealed += 1;
        }
    }
    Ok(sealed)
}

/// Seals the indexers' keys kept plain; how many.
async fn seal_indexers(tx: &mut Switching, key: &Key) -> anyhow::Result<usize> {
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, api_key FROM indexers")
        .fetch_all(&mut **tx)
        .await?;
    let mut sealed = 0;
    for (id, api_key) in rows {
        if !api_key.is_empty() && !super::is_sealed_text(&api_key) {
            sqlx::query("UPDATE indexers SET api_key = ? WHERE id = ?")
                .bind(key.seal_text(INDEXER_KEY, &api_key)?)
                .bind(id)
                .execute(&mut **tx)
                .await?;
            sealed += 1;
        }
    }
    Ok(sealed)
}

/// Seals the two-factor secrets kept plain; how many.
async fn seal_two_factor(tx: &mut Switching, key: &Key) -> anyhow::Result<usize> {
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT user_id, secret FROM two_factor")
        .fetch_all(&mut **tx)
        .await?;
    let mut sealed = 0;
    for (user_id, secret) in rows.into_iter().filter(|(_, s)| s.len() == TWO_FACTOR_LEN) {
        sqlx::query("UPDATE two_factor SET secret = ? WHERE user_id = ?")
            .bind(key.seal_bytes(&super::two_factor(user_id), &secret)?)
            .bind(user_id)
            .execute(&mut **tx)
            .await?;
        sealed += 1;
    }
    Ok(sealed)
}

/// Seals search results' download links and guids kept plain; how many results.
async fn seal_links(tx: &mut Switching, key: &Key) -> anyhow::Result<usize> {
    let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, download_url, guid FROM release_decisions
         WHERE download_url != '' OR guid != ''",
    )
    .fetch_all(&mut **tx)
    .await?;
    let seal = |what: &str, link: &Option<String>| -> anyhow::Result<Option<String>> {
        Ok(match link {
            Some(link) if !link.is_empty() && !super::is_sealed_text(link) => {
                Some(key.seal_text(what, link)?)
            }
            other => other.clone(),
        })
    };
    for (id, download_url, guid) in &rows {
        sqlx::query("UPDATE release_decisions SET download_url = ?, guid = ? WHERE id = ?")
            .bind(seal(super::DOWNLOAD_URL, download_url)?)
            .bind(seal(super::GUID, guid)?)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(rows.len())
}
