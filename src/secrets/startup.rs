//! The server's start: the key is read (made when there's none and nothing needs one), every
//! stored secret is checked to open with it, and a database not switched yet has its plain
//! secrets sealed, once, then the backup copies made before it too. A secret that can't be read
//! stops the start, saying which and how to recover; `melete secrets clear` is one way back.

use std::path::Path;

use anyhow::bail;
use sqlx::SqlitePool;

use super::key_file::{self, Found};
use super::{INDEXER_KEY, Key, Unreadable, opened_bytes, opened_text, sealing};
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
    /// Users whose two-factor sign-in is on and whose secret can't be read, by name, with why.
    pub two_factor: Vec<(String, Unreadable)>,
    /// Two-factor setups still waiting for their first code whose secret can't be read, by user
    /// id and name: worthless without it, so `melete secrets clear` removes them.
    pub setups: Vec<(i64, String)>,
    /// Search results whose download link or guid can't be read, by id.
    pub links: Vec<i64>,
}

impl Unreadables {
    /// Whether every stored secret can be read.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty()
            && self.indexers.is_empty()
            && self.two_factor.is_empty()
            && self.setups.is_empty()
            && self.links.is_empty()
    }
}

/// Reads the key in `data_dir` (made there when there's none and nothing is sealed), checks
/// every stored secret opens with it, and seals the plain ones of a database not switched yet,
/// then those of the backup copies. Fails, saying which secrets and how to recover, when one
/// can't be read.
pub async fn prepare(pool: &SqlitePool, data_dir: &Path) -> anyhow::Result<Key> {
    let path = data_dir.join(super::KEY_FILE);
    let found = match key_file::read(&path)? {
        Found::Key(key) => Some(key),
        Found::Missing => None,
        Found::Garbled => bail!("{}", super::recovery::garbled(&path, pool).await?),
    };
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
    let in_copies = super::copies::seal_copies(pool, data_dir, &key).await?;
    if in_copies > 0 {
        tracing::info!(
            sealed = in_copies,
            "sealed the secrets the backup copies kept in plain text"
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
    let rows: Vec<(i64, String, Vec<u8>, bool)> = sqlx::query_as(
        "SELECT t.user_id, u.username, t.secret, t.confirmed_at IS NOT NULL
         FROM two_factor t JOIN users u ON u.id = t.user_id ORDER BY u.username",
    )
    .fetch_all(pool)
    .await?;
    for (user_id, username, secret, on) in rows {
        let what = super::two_factor(user_id);
        match opened_bytes(key, sealing, &what, &secret, TWO_FACTOR_LEN) {
            Ok(_) => {}
            Err(why) if on => found.two_factor.push((username, why)),
            Err(_) => found.setups.push((user_id, username)),
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
/// switched, together, with `secure_delete` on so the old bytes are overwritten; then vacuums
/// the database and empties its WAL, so no freed page or old frame keeps them. How many were
/// sealed; None when it was switched already.
pub async fn switch(pool: &SqlitePool, key: &Key) -> anyhow::Result<Option<usize>> {
    let mut conn = pool.acquire().await?;
    sqlx::query("PRAGMA secure_delete = ON")
        .execute(&mut *conn)
        .await?;
    let switched = seal_and_mark(&mut conn, key).await;
    if matches!(switched, Ok(Some(_))) {
        sqlx::query("VACUUM").execute(&mut *conn).await?;
        // An in-memory database has no WAL; the answer says nothing that matters here.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut *conn)
            .await?;
    }
    sqlx::query("PRAGMA secure_delete = OFF")
        .execute(&mut *conn)
        .await?;
    switched
}

/// Seals what's plain and marks the database switched, in one transaction.
async fn seal_and_mark(
    conn: &mut sqlx::SqliteConnection,
    key: &Key,
) -> anyhow::Result<Option<usize>> {
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    if sealing(&mut *tx).await? {
        return Ok(None);
    }
    let sealed = super::sealing::seal_all(&mut tx, key).await?;
    sqlx::query("INSERT INTO secrets_sealed (id, sealed_at) VALUES (1, ?)")
        .bind(unix_now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(sealed))
}
