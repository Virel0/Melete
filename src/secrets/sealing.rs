//! Sealing the secrets a database keeps plain, in place: the live database at the switch, and
//! each backup copy after it. Values already sealed are left as they are, and a table an older
//! copy doesn't have yet is skipped.

use sqlx::SqliteConnection;

use super::startup::TWO_FACTOR_LEN;
use super::{DOWNLOAD_URL, GUID, INDEXER_KEY, Key, is_sealed_text};
use crate::settings;

/// Seals every plain secret in the database `conn` is open on, inside the transaction the
/// caller holds; how many values it sealed.
pub(super) async fn seal_all(conn: &mut SqliteConnection, key: &Key) -> anyhow::Result<usize> {
    Ok(seal_settings(conn, key).await?
        + seal_indexers(conn, key).await?
        + seal_two_factor(conn, key).await?
        + seal_links(conn, key).await?)
}

/// Whether the database has `table`: a copy from an older version may not.
async fn has(conn: &mut SqliteConnection, table: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
    )
    .bind(table)
    .fetch_one(conn)
    .await
}

/// Seals the secret settings kept plain; how many.
async fn seal_settings(conn: &mut SqliteConnection, key: &Key) -> anyhow::Result<usize> {
    if !has(conn, "settings").await? {
        return Ok(0);
    }
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT name, value FROM settings")
        .fetch_all(&mut *conn)
        .await?;
    let mut sealed = 0;
    for (name, value) in rows {
        if settings::is_secret(&name) && !is_sealed_text(&value) {
            sqlx::query("UPDATE settings SET value = ? WHERE name = ?")
                .bind(key.seal_text(&super::setting(&name), &value)?)
                .bind(&name)
                .execute(&mut *conn)
                .await?;
            sealed += 1;
        }
    }
    Ok(sealed)
}

/// Seals the indexers' keys kept plain; how many.
async fn seal_indexers(conn: &mut SqliteConnection, key: &Key) -> anyhow::Result<usize> {
    if !has(conn, "indexers").await? {
        return Ok(0);
    }
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, api_key FROM indexers")
        .fetch_all(&mut *conn)
        .await?;
    let mut sealed = 0;
    for (id, api_key) in rows {
        if !api_key.is_empty() && !is_sealed_text(&api_key) {
            sqlx::query("UPDATE indexers SET api_key = ? WHERE id = ?")
                .bind(key.seal_text(INDEXER_KEY, &api_key)?)
                .bind(id)
                .execute(&mut *conn)
                .await?;
            sealed += 1;
        }
    }
    Ok(sealed)
}

/// Seals the two-factor secrets kept plain; how many.
async fn seal_two_factor(conn: &mut SqliteConnection, key: &Key) -> anyhow::Result<usize> {
    if !has(conn, "two_factor").await? {
        return Ok(0);
    }
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as("SELECT user_id, secret FROM two_factor")
        .fetch_all(&mut *conn)
        .await?;
    let mut sealed = 0;
    for (user_id, secret) in rows.into_iter().filter(|(_, s)| s.len() == TWO_FACTOR_LEN) {
        sqlx::query("UPDATE two_factor SET secret = ? WHERE user_id = ?")
            .bind(key.seal_bytes(&super::two_factor(user_id), &secret)?)
            .bind(user_id)
            .execute(&mut *conn)
            .await?;
        sealed += 1;
    }
    Ok(sealed)
}

/// Seals search results' download links and guids kept plain; how many results.
async fn seal_links(conn: &mut SqliteConnection, key: &Key) -> anyhow::Result<usize> {
    if !has(conn, "release_decisions").await? {
        return Ok(0);
    }
    let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, download_url, guid FROM release_decisions
         WHERE download_url != '' OR guid != ''",
    )
    .fetch_all(&mut *conn)
    .await?;
    let plain = |link: &Option<String>| {
        link.as_deref()
            .is_some_and(|l| !l.is_empty() && !is_sealed_text(l))
    };
    let seal = |what: &str, link: &Option<String>| -> anyhow::Result<Option<String>> {
        Ok(match link {
            Some(l) if plain(link) => Some(key.seal_text(what, l)?),
            other => other.clone(),
        })
    };
    let mut sealed = 0;
    for (id, download_url, guid) in rows.iter().filter(|(_, u, g)| plain(u) || plain(g)) {
        sqlx::query("UPDATE release_decisions SET download_url = ?, guid = ? WHERE id = ?")
            .bind(seal(DOWNLOAD_URL, download_url)?)
            .bind(seal(GUID, guid)?)
            .bind(id)
            .execute(&mut *conn)
            .await?;
        sealed += 1;
    }
    Ok(sealed)
}
