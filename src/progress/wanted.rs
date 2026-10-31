//! A wanted item as its progress needs it: its title, whether it's on disk, and whether
//! something looks out for it on its own.

use super::*;

/// What one wanted item has in place of a download: its title, whether it's on disk, and
/// whether something looks out for it on its own.
pub(super) struct Wanted {
    /// As a person would say it; for music, with its artist.
    pub(super) title: String,
    /// Whether it's in the library.
    pub(super) on_disk: bool,
    /// Whether RSS syncs and searches look out for it (it's monitored).
    pub(super) looked_for: bool,
}

pub(super) async fn wanted(
    pool: &SqlitePool,
    kind: &str,
    id: i64,
) -> anyhow::Result<Option<Wanted>> {
    let row: Option<(String, bool, bool)> = match kind {
        "movie" => {
            sqlx::query_as(
                "SELECT w.title,
                        EXISTS (SELECT 1 FROM media_files f WHERE f.movie_id = w.movie_id
                                AND f.missing_since IS NULL),
                        w.monitored = 1
                 FROM wanted_movies w WHERE w.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        "series" => {
            sqlx::query_as(
                "SELECT w.title,
                        EXISTS (SELECT 1 FROM media_files f WHERE f.series_id = w.series_id
                                AND f.missing_since IS NULL),
                        w.monitored = 1
                 FROM wanted_series w WHERE w.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        "album" => {
            sqlx::query_as(
                "SELECT w.title || ' by ' || a.name,
                        EXISTS (SELECT 1 FROM music_albums m WHERE m.missing_since IS NULL
                                AND w.mb_release_group_id IN (m.mb_release_group_id, m.match_release_group_id)),
                        w.monitored = 1 AND a.monitored = 1
                 FROM wanted_albums w JOIN wanted_artists a ON a.id = w.wanted_artist_id
                 WHERE w.id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        // A song is in the library once a download of it is; nothing looks for songs on its
        // own, so one waits for a search.
        "song" => {
            sqlx::query_as(
                "SELECT title || ' by ' || artist, 0, 0 FROM wanted_songs WHERE id = ?",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
        }
        _ => None,
    };
    Ok(row.map(|(title, on_disk, looked_for)| Wanted {
        title,
        on_disk,
        looked_for,
    }))
}
