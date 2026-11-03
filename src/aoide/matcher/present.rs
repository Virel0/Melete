//! The library as matching sees it: every present track, with its own artist and its album's.

use sqlx::SqlitePool;

use super::LibraryTrack;

#[derive(sqlx::FromRow)]
struct TrackRow {
    id: i64,
    title: String,
    artist: Option<String>,
    album_artist: String,
    album: String,
    duration_seconds: Option<f64>,
    isrc: Option<String>,
}

/// Every present track, with its own artist and its album's (each once).
///
/// # Errors
///
/// Fails when the database does.
pub async fn present_tracks(pool: &SqlitePool) -> sqlx::Result<Vec<LibraryTrack>> {
    let rows: Vec<TrackRow> = sqlx::query_as(
        "SELECT t.id, t.title, t.artist, a.artist_name AS album_artist, a.title AS album,
             t.duration_seconds, t.isrc
         FROM music_tracks t JOIN music_albums a ON a.id = t.album_id
         WHERE t.missing_since IS NULL AND a.missing_since IS NULL
         ORDER BY t.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut artists: Vec<String> = Vec::new();
            for name in row.artist.into_iter().chain([row.album_artist]) {
                if !name.trim().is_empty() && !artists.iter().any(|a| a.eq_ignore_ascii_case(&name))
                {
                    artists.push(name);
                }
            }
            LibraryTrack {
                id: row.id.to_string(),
                title: Some(row.title),
                artists,
                album: Some(row.album),
                duration_ms: row.duration_seconds.map(|s| (s * 1000.0).round() as i64),
                isrc: row.isrc,
            }
        })
        .collect())
}
