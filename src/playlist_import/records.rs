//! Imports as they're recorded: created, listed, and looked up with their tracks.

use super::*;

/// An import as listed.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct ImportSummary {
    /// The import's row id.
    pub id: i64,
    /// The user who imported it, for whom its songs are asked.
    pub user_id: i64,
    /// Their username.
    pub username: String,
    /// The playlist's name.
    pub name: String,
    /// queued, running, done or failed.
    pub state: String,
    /// How it ended, or why it failed; none while queued or running.
    pub message: Option<String>,
    /// When it was imported, in seconds since the epoch.
    pub created_at: i64,
    /// When it ended, in seconds since the epoch; none until then.
    pub finished_at: Option<i64>,
    /// How many tracks the playlist has.
    pub tracks: i64,
    /// Asked for by this import.
    pub asked: i64,
    /// Asked for already by the same user.
    pub already: i64,
    /// Tracks MusicBrainz had nothing fitting for.
    pub unmatched: i64,
    /// Of those asked for, the ones still waiting for an admin.
    pub waiting: i64,
}

/// One track of an import, and what became of it.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct ImportTrack {
    /// Its place in the playlist, from 1.
    pub position: i64,
    /// Its title, as the playlist lists it.
    pub title: String,
    /// Its artists, joined with commas; none when the playlist lists none.
    pub artist: Option<String>,
    /// Its album's title, when given.
    pub album: Option<String>,
    /// Its ISRC, when given.
    pub isrc: Option<String>,
    /// pending, asked, already or unmatched.
    pub state: String,
    /// Why it couldn't be asked for.
    pub why: Option<String>,
    /// The song request it made, or the one already there; none when there's neither.
    pub request_id: Option<i64>,
}

/// An import with its tracks, in playlist order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImportView {
    /// The import as listed.
    #[serde(flatten)]
    pub summary: ImportSummary,
    /// Its tracks, in playlist order.
    pub items: Vec<ImportTrack>,
}

pub(super) const SUMMARY: &str = "SELECT i.id, i.user_id, u.username, i.name, i.state, i.message, i.created_at,
        i.finished_at,
        (SELECT count(*) FROM playlist_import_tracks t WHERE t.import_id = i.id) AS tracks,
        (SELECT count(*) FROM playlist_import_tracks t WHERE t.import_id = i.id AND t.state = 'asked') AS asked,
        (SELECT count(*) FROM playlist_import_tracks t WHERE t.import_id = i.id AND t.state = 'already') AS already,
        (SELECT count(*) FROM playlist_import_tracks t WHERE t.import_id = i.id AND t.state = 'unmatched') AS unmatched,
        (SELECT count(*) FROM playlist_import_tracks t JOIN requests r ON r.id = t.request_id
          WHERE t.import_id = i.id AND t.state = 'asked' AND r.state = 'requested') AS waiting
     FROM playlist_imports i JOIN users u ON u.id = i.user_id";

/// Records an import of `tracks` for `user`, to be worked through in the background.
pub async fn create(
    pool: &SqlitePool,
    user: &User,
    name: &str,
    tracks: &[Track],
) -> anyhow::Result<ImportSummary> {
    let mut tx = pool.begin().await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO playlist_imports (user_id, name, state, created_at) VALUES (?, ?, 'queued', ?)
         RETURNING id",
    )
    .bind(user.id)
    .bind(name)
    .bind(unix_now())
    .fetch_one(&mut *tx)
    .await?;
    for (position, track) in tracks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_import_tracks (import_id, position, title, artist, album, isrc,
                 length_ms, state)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')",
        )
        .bind(id)
        .bind(position as i64 + 1)
        .bind(&track.title)
        .bind((!track.artists.is_empty()).then(|| track.artists.join(", ")))
        .bind(&track.album)
        .bind(&track.isrc)
        .bind(track.length_ms)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    summary(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("import {id} vanished"))
}

/// An import as listed; none when there's no such import.
pub async fn summary(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<ImportSummary>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUMMARY} WHERE i.id = ?")))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// An import with its tracks; none when there's no such import.
pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<ImportView>> {
    let Some(summary) = summary(pool, id).await? else {
        return Ok(None);
    };
    let items = sqlx::query_as(
        "SELECT position, title, artist, album, isrc, state, why, request_id
         FROM playlist_import_tracks WHERE import_id = ? ORDER BY position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(Some(ImportView { summary, items }))
}

/// A user's imports, or everyone's; newest first.
pub async fn list(pool: &SqlitePool, user_id: Option<i64>) -> sqlx::Result<Vec<ImportSummary>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SUMMARY} WHERE ?1 IS NULL OR i.user_id = ?1 ORDER BY i.created_at DESC, i.id DESC LIMIT 100"
    )))
    .bind(user_id)
    .fetch_all(pool)
    .await
}
