//! What the apps are told: where a video made here is, who sees them, where making an album's
//! stands, and how far making them has got.

use super::*;

/// A video named in the database, in the videos folder and nowhere else.
pub fn video_path(videos: &Path, file: &str) -> PathBuf {
    let name = Path::new(file)
        .file_name()
        .map_or_else(|| "none".into(), |n| n.to_owned());
    videos.join(name)
}

/// Whether a user sees motion covers made here: only while they're on, and unless the user
/// turned them off for themselves. Motion covers from an album's own folder are always shown.
pub async fn shown_to(pool: &SqlitePool, user_id: i64) -> sqlx::Result<bool> {
    Ok(crate::settings::motion_covers(pool).await? && chosen(pool, user_id).await?)
}

/// A user's own choice; on until they turn it off.
pub async fn chosen(pool: &SqlitePool, user_id: i64) -> sqlx::Result<bool> {
    let show: Option<bool> =
        sqlx::query_scalar("SELECT show_generated FROM music_motion_choices WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(show.unwrap_or(true))
}

/// Records a user's choice to see motion covers made here, or not.
///
/// # Errors
///
/// Fails only when the database can't be written.
pub async fn choose(pool: &SqlitePool, user_id: i64, show: bool) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO music_motion_choices (user_id, show_generated, changed_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id) DO UPDATE SET show_generated = ?2, changed_at = ?3",
    )
    .bind(user_id)
    .bind(show)
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// The file of an album's motion cover made here, when it's from the cover it has now.
pub async fn current_file(pool: &SqlitePool, album: i64) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT m.file FROM music_motion_covers m
         JOIN music_cover_colours c ON c.album_id = m.album_id
         WHERE m.album_id = ? AND m.state = 'done' AND m.source = c.source",
    )
    .bind(album)
    .fetch_optional(pool)
    .await
    .map(Option::flatten)
}

/// Where making an album's motion cover stands, for the apps: queued, done, skipped (and why) or
/// failed (and why). None when it hasn't been tried for the cover the album has now.
pub async fn status(pool: &SqlitePool, album: i64) -> sqlx::Result<Option<Value>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT m.state, m.reason FROM music_motion_covers m
         LEFT JOIN music_cover_colours c ON c.album_id = m.album_id
         WHERE m.album_id = ? AND (m.state = 'queued' OR m.source = c.source)",
    )
    .bind(album)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(state, reason)| json!({ "state": state, "reason": reason })))
}

/// How far making motion covers has got.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Coverage {
    /// Present albums with a cover and no motion cover of their own.
    pub albums: i64,
    /// Albums whose motion cover was made.
    pub done: i64,
    /// Albums waiting for the renderer.
    pub queued: i64,
    /// Albums the renderer skipped, as for a cover that's mostly text or flat.
    pub skipped: i64,
    /// Albums whose making failed.
    pub failed: i64,
    /// The latest failure's reason.
    pub last_error: Option<String>,
}

/// How far making motion covers has got, counted from the database.
///
/// # Errors
///
/// Fails only when the database can't be read.
pub async fn coverage(pool: &SqlitePool) -> sqlx::Result<Coverage> {
    let (albums, done, queued, skipped, failed): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM music_albums a
                 JOIN music_cover_colours c ON c.album_id = a.id AND c.colours IS NOT NULL
                 WHERE a.missing_since IS NULL AND a.motion_file IS NULL),
                (SELECT count(*) FROM music_motion_covers WHERE state = 'done'),
                (SELECT count(*) FROM music_motion_covers WHERE state = 'queued'),
                (SELECT count(*) FROM music_motion_covers WHERE state = 'skipped'),
                (SELECT count(*) FROM music_motion_covers WHERE state = 'failed')",
    )
    .fetch_one(pool)
    .await?;
    let last_error = sqlx::query_scalar(
        "SELECT reason FROM music_motion_covers WHERE state = 'failed'
         ORDER BY finished_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(Coverage {
        albums,
        done,
        queued,
        skipped,
        failed,
        last_error,
    })
}
