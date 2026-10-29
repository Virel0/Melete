//! Upgrades: a grab records the library files the release would
//! replace. Once the upgrade is in the library, scanned and probed, each replaced file whose
//! successor plays about as long is moved to its library's trash folder, and the trash is
//! emptied of files older than 30 days. Anything unclear keeps the old file: it then stays
//! beside the new one, as before.
//!
//! Only files recorded at grab time are ever moved, only when unchanged since, only regular
//! files inside their library, and never over anything: the move fails rather than replace.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::release::{self, Episodes};

mod retiring;
mod trash;

pub use retiring::*;
pub use trash::*;

/// The trash folder at the top of each library; scans skip names starting with a dot.
pub const TRASH: &str = ".melete-trash";
/// How long a replaced file waits in the trash.
pub const KEEP_SECONDS: i64 = 30 * 24 * 3600;
/// How long an imported upgrade may wait for its scan and probe before the old files are kept.
const WAIT_SECONDS: i64 = 7 * 24 * 3600;

/// A library file a download would replace, as it was when grabbed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct Replaced {
    /// The library file's row id.
    pub file_id: i64,
    /// The library it's in.
    pub library_id: i64,
    /// Its path relative to the library's folder.
    pub rel_path: String,
    /// Its size in bytes when grabbed; a file that has changed since is kept.
    pub size: i64,
    /// When it was last changed, in Unix seconds, when grabbed; a file changed since is kept.
    pub modified_at: i64,
    /// The season of the episode it holds; None for a movie.
    pub season: Option<i64>,
    /// The (first) episode it holds; None for a movie.
    pub episode: Option<i64>,
    /// The last episode of a file holding several; None for a single episode or a movie.
    pub episode_end: Option<i64>,
}

/// The files a release grabbed for a wanted title would replace: the movie's one file in its
/// adopted folder, or the files of the episodes it covers, each wholly inside them. Nothing
/// when it's unclear (several files for a movie, a file reaching past the release's episodes,
/// a season pack), so nothing is ever replaced by guesswork.
pub async fn replaced_by(
    pool: &SqlitePool,
    wanted_movie_id: Option<i64>,
    wanted_series_id: Option<i64>,
    season: Option<i64>,
    title: &str,
) -> anyhow::Result<Vec<Replaced>> {
    const COLUMNS: &str = "SELECT f.id AS file_id, f.library_id, f.rel_path, f.size,
        f.modified_at, f.season, f.episode, f.episode_end FROM media_files f";
    if let Some(id) = wanted_movie_id {
        let files: Vec<Replaced> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{COLUMNS} JOIN wanted_movies w ON w.movie_id = f.movie_id
             WHERE w.id = ? AND f.missing_since IS NULL"
        )))
        .bind(id)
        .fetch_all(pool)
        .await?;
        return Ok(if files.len() == 1 { files } else { Vec::new() });
    }
    let (Some(id), Some(season)) = (wanted_series_id, season) else {
        return Ok(Vec::new());
    };
    let Episodes::Standard {
        season: s,
        episodes,
    } = release::parse(title).episodes
    else {
        return Ok(Vec::new());
    };
    let (Some(&first), Some(&last)) = (episodes.iter().min(), episodes.iter().max()) else {
        return Ok(Vec::new());
    };
    if s != season {
        return Ok(Vec::new());
    }
    let files: Vec<Replaced> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{COLUMNS} JOIN wanted_series w ON w.series_id = f.series_id
         WHERE w.id = ? AND f.season = ? AND f.missing_since IS NULL
           AND f.episode <= ? AND coalesce(f.episode_end, f.episode) >= ?"
    )))
    .bind(id)
    .bind(season)
    .bind(last)
    .bind(first)
    .fetch_all(pool)
    .await?;
    let inside = files.iter().all(|f| {
        f.episode.is_some_and(|e| e >= first)
            && f.episode_end.or(f.episode).is_some_and(|e| e <= last)
    });
    // Two files for one episode (a dub and the original, say): which one goes is unclear.
    let span = |f: &Replaced| {
        (
            f.episode.unwrap_or(0),
            f.episode_end.or(f.episode).unwrap_or(0),
        )
    };
    let overlapping = files.iter().enumerate().any(|(i, a)| {
        files[i + 1..].iter().any(|b| {
            let (a, b) = (span(a), span(b));
            a.0 <= b.1 && b.0 <= a.1
        })
    });
    Ok(if inside && !overlapping {
        files
    } else {
        Vec::new()
    })
}

/// A path inside a library, as stored: relative, with no way out of it.
fn inside(root: &Path, rel_path: &str) -> Option<PathBuf> {
    let rel = Path::new(rel_path);
    let plain = rel.components().all(|c| matches!(c, Component::Normal(_)));
    (plain && !rel_path.is_empty()).then(|| root.join(rel))
}

/// Whether two durations are the same cut: within 2%, or 30 seconds for short episodes.
fn same_length(a: f64, b: f64) -> bool {
    (a - b).abs() <= (a.max(b) * 0.02).max(30.0)
}

#[cfg(test)]
mod tests;
