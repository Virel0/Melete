//! The blocklist: releases an admin never wants grabbed again, added from a search
//! result or a download. A release is known by its name (case aside), and a torrent also by
//! its info hash once Melete has seen it. Grabbing refuses either, by hand or automatically,
//! and automatic grabs pass over blocklisted results to the next best.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;

/// A blocklisted release.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Entry {
    /// The entry's row id.
    pub id: i64,
    /// The release name, as it was when blocklisted.
    pub title: String,
    /// torrent, or soulseek for a Soulseek peer's folder or file.
    pub source: String,
    /// When it was blocklisted, in Unix seconds.
    pub added_at: i64,
}

/// Where a release to blocklist is taken from.
#[derive(Debug, Clone, Copy)]
pub enum From {
    /// A search result, by its decision's row id.
    Release(i64),
    /// A download, by its row id.
    Download(i64),
}

/// Blocklists a search result's or a download's release; None when there's no such one. One
/// on the list already stays as it was (its info hash added when it lacked one).
pub async fn add(pool: &SqlitePool, from: From) -> anyhow::Result<Option<Entry>> {
    let found: Option<(String, Option<String>, String)> = match from {
        From::Release(id) => {
            sqlx::query_as(
                "SELECT title, infohash,
                        CASE WHEN soulseek IS NULL THEN 'torrent' ELSE 'soulseek' END
                 FROM release_decisions WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        From::Download(id) => {
            sqlx::query_as("SELECT title, infohash, source FROM downloads WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?
        }
    };
    let Some((title, infohash, source)) = found else {
        return Ok(None);
    };
    let infohash = infohash.map(|h| h.to_ascii_lowercase());
    sqlx::query(
        "INSERT INTO blocklist (title, infohash, source, added_at) VALUES (?, ?, ?, ?)
         ON CONFLICT (lower(title)) DO UPDATE SET infohash = coalesce(blocklist.infohash, excluded.infohash)",
    )
    .bind(&title)
    .bind(&infohash)
    .bind(&source)
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(sqlx::query_as(
        "SELECT id, title, source, added_at FROM blocklist WHERE lower(title) = lower(?)",
    )
    .bind(&title)
    .fetch_optional(pool)
    .await?)
}

/// Everything on the blocklist, newest first.
pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<Entry>> {
    sqlx::query_as(
        "SELECT id, title, source, added_at FROM blocklist ORDER BY added_at DESC, id DESC",
    )
    .fetch_all(pool)
    .await
}

/// Takes a release off the blocklist; false when it wasn't on it.
pub async fn remove(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    Ok(sqlx::query("DELETE FROM blocklist WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

/// Whether a release of this name, or (when given) this torrent, is on the blocklist.
pub async fn blocks(pool: &SqlitePool, title: &str, infohash: Option<&str>) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM blocklist
                        WHERE lower(title) = lower(?1) OR (?2 IS NOT NULL AND infohash = lower(?2)))",
    )
    .bind(title)
    .bind(infohash)
    .fetch_one(pool)
    .await
}
