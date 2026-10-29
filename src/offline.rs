//! Offline copies: a converted file a device can download and keep, while an admin has them on
//! (`MELETE_OFFLINE_COPIES`, off by default). Asked for with a transcode's choices, made in the
//! background one at a time, and only while no more than half the live conversion slots are in
//! use, so watching comes first. Each is one MP4 (H.264 or HEVC with AAC, its index at the
//! front) in the transcode folder's `offline/`, with WebVTT files for the subtitles asked for,
//! never in the library. A copy is deleted 7 days after its last download (or after it was
//! made, if never downloaded), as soon as the one who asked deletes it, or a day after failing.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::transcode::{Plan, VideoCodec};

mod making;

pub use making::*;

/// A copy is kept this long after its last download, or after it was made.
pub const KEPT: i64 = 7 * 24 * 3600;
/// A failed copy says why for this long.
pub const FAILED_KEPT: i64 = 24 * 3600;
/// Copies one person may have at once, waiting, being made or ready.
pub const PER_USER: i64 = 20;
/// How long a download link works.
pub const LINK_LIFETIME: i64 = 24 * 3600;
/// Download links' tokens start with this.
pub const LINK_PREFIX: &str = "mlo_";

/// Where copies are kept, inside the transcode folder.
pub fn folder(transcode_dir: &Path) -> PathBuf {
    transcode_dir.join("offline")
}

/// A copy's video file.
pub fn video_path(root: &Path, id: i64) -> PathBuf {
    root.join(format!("{id}.mp4"))
}

/// A copy's subtitle file for a track (`s3`, `x1a2b3c4d`).
pub fn subtitle_path(root: &Path, id: i64, track: &str) -> PathBuf {
    root.join(format!("{id}.{track}.vtt"))
}

/// A copy as its owner sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Copy {
    /// Its id.
    pub id: i64,
    /// The file it's a copy of.
    pub file_id: i64,
    /// queued, making, ready or failed.
    pub status: String,
    /// How far along making it is, from 0 to 1.
    pub progress: f64,
    /// Why it failed, when it did.
    pub error: Option<String>,
    /// About how big it will be, in bytes, worked out before it's made.
    pub estimated_bytes: i64,
    /// How big it is, once ready.
    pub bytes: Option<i64>,
    /// When it was asked for, in Unix seconds.
    pub created_at: i64,
    /// When it was ready.
    pub ready_at: Option<i64>,
    /// When it will be deleted, once ready or failed.
    pub delete_at: Option<i64>,
    /// The subtitle tracks it includes.
    pub subtitles: Vec<String>,
}

/// A copy as stored: its subtitle tracks as JSON.
#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    file_id: i64,
    status: String,
    progress: f64,
    error: Option<String>,
    estimated_bytes: i64,
    bytes: Option<i64>,
    created_at: i64,
    ready_at: Option<i64>,
    delete_at: Option<i64>,
    subtitles: String,
}

impl From<Row> for Copy {
    fn from(r: Row) -> Self {
        Copy {
            id: r.id,
            file_id: r.file_id,
            status: r.status,
            progress: r.progress,
            error: r.error,
            estimated_bytes: r.estimated_bytes,
            bytes: r.bytes,
            created_at: r.created_at,
            ready_at: r.ready_at,
            delete_at: r.delete_at,
            subtitles: serde_json::from_str(&r.subtitles).unwrap_or_default(),
        }
    }
}

const COPY_COLUMNS: &str =
    "id, file_id, status, progress, error, estimated_bytes, bytes, created_at,
     ready_at,
     CASE status
         WHEN 'ready' THEN COALESCE(last_download_at, ready_at) + 604800
         WHEN 'failed' THEN created_at + 86400
     END AS delete_at,
     subtitles";

/// About how big a copy made by `plan` will be: its video bitrate (the file's own when the
/// video is copied) and its sound's, over its length.
pub fn estimate(plan: &Plan, file_bitrate: Option<i64>) -> i64 {
    let video = match plan.codec {
        VideoCodec::Copy => file_bitrate.unwrap_or(plan.bitrate),
        _ => plan.bitrate,
    };
    let audio = match (plan.audio, plan.channels) {
        (None, _) => 0,
        (Some(_), c) if c > 2 => 384_000,
        _ => 192_000,
    };
    ((video + audio) as f64 * plan.duration / 8.0).round() as i64
}

/// Asks for a copy; its id.
pub async fn create(
    pool: &SqlitePool,
    user_id: i64,
    file_id: i64,
    settings: &str,
    subtitles: &[String],
    estimated_bytes: i64,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO offline_copies (user_id, file_id, settings, subtitles, status, estimated_bytes,
             created_at) VALUES (?, ?, ?, ?, 'queued', ?, ?) RETURNING id",
    )
    .bind(user_id)
    .bind(file_id)
    .bind(settings)
    .bind(serde_json::to_string(subtitles)?)
    .bind(estimated_bytes)
    .bind(unix_now())
    .fetch_one(pool)
    .await?)
}

/// How many copies someone has waiting, being made or ready.
pub async fn held(pool: &SqlitePool, user_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM offline_copies WHERE user_id = ? AND status <> 'failed'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// Someone's copies, newest first, of one file or of every file.
pub async fn of_user(
    pool: &SqlitePool,
    user_id: i64,
    file_id: Option<i64>,
) -> sqlx::Result<Vec<Copy>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COPY_COLUMNS} FROM offline_copies
         WHERE user_id = ? AND (?2 IS NULL OR file_id = ?2) ORDER BY id DESC"
    )))
    .bind(user_id)
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Copy::from).collect())
}

/// One of someone's copies.
pub async fn get(pool: &SqlitePool, user_id: i64, id: i64) -> sqlx::Result<Option<Copy>> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COPY_COLUMNS} FROM offline_copies WHERE user_id = ? AND id = ?"
    )))
    .bind(user_id)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Copy::from))
}

/// Deletes a copy and its files. A copy being made stops being made: its run sees it's gone.
pub async fn delete(pool: &SqlitePool, root: &Path, id: i64) -> anyhow::Result<()> {
    let subtitles: Option<String> =
        sqlx::query_scalar("SELECT subtitles FROM offline_copies WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    sqlx::query("DELETE FROM offline_copies WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let tracks: Vec<String> = subtitles
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    let mut paths = vec![video_path(root, id), root.join(format!("{id}.mp4.part"))];
    paths.extend(tracks.iter().map(|t| subtitle_path(root, id, t)));
    for path in paths {
        match tokio::fs::remove_file(&path).await {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
            _ => {}
        }
    }
    Ok(())
}

/// A download link for a ready copy, tied to the session asking; the token and its expiry.
pub async fn link(pool: &SqlitePool, copy_id: i64, token_id: i64) -> anyhow::Result<(String, i64)> {
    let token = crate::auth::random_token(LINK_PREFIX)?;
    let expires_at = unix_now() + LINK_LIFETIME;
    sqlx::query("DELETE FROM offline_links WHERE expires_at <= ?")
        .bind(unix_now())
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO offline_links (token_hash, copy_id, token_id, expires_at) VALUES (?, ?, ?, ?)",
    )
    .bind(crate::auth::token_hash(&token))
    .bind(copy_id)
    .bind(token_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok((token, expires_at))
}

/// What a download link opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    /// The copy.
    pub copy: i64,
    /// The file it's a copy of.
    pub file: i64,
    /// The session or key the link was given to.
    pub holder: i64,
    /// The copy's subtitle tracks.
    pub subtitles: Vec<String>,
}

/// The ready copy a download link opens; None for an unknown or expired link, or a copy not
/// ready.
pub async fn opened_by(pool: &SqlitePool, token: &str) -> sqlx::Result<Option<Opened>> {
    let found: Option<(i64, i64, i64, String)> = sqlx::query_as(
        "SELECT c.id, c.file_id, l.token_id, c.subtitles FROM offline_links l
         JOIN offline_copies c ON c.id = l.copy_id
         WHERE l.token_hash = ? AND l.expires_at > ? AND c.status = 'ready'",
    )
    .bind(crate::auth::token_hash(token))
    .bind(unix_now())
    .fetch_optional(pool)
    .await?;
    Ok(found.map(|(copy, file, holder, subtitles)| Opened {
        copy,
        file,
        holder,
        subtitles: serde_json::from_str(&subtitles).unwrap_or_default(),
    }))
}

/// Notes a copy was downloaded, which keeps it another 7 days.
pub async fn downloaded(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE offline_copies SET last_download_at = ? WHERE id = ?")
        .bind(unix_now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes copies whose time is up: 7 days after their last download (or after they were
/// made), a day after failing. Returns how many.
pub async fn sweep(pool: &SqlitePool, root: &Path) -> anyhow::Result<usize> {
    let now = unix_now();
    let due: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM offline_copies
         WHERE (status = 'ready' AND COALESCE(last_download_at, ready_at) <= ?)
            OR (status = 'failed' AND created_at <= ?)",
    )
    .bind(now - KEPT)
    .bind(now - FAILED_KEPT)
    .fetch_all(pool)
    .await?;
    for id in &due {
        delete(pool, root, *id).await?;
    }
    Ok(due.len())
}

#[cfg(test)]
pub(crate) mod tests;
