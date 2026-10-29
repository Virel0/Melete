//! What's kept of OpenSubtitles: searches remembered, subtitles fetched (in the data folder,
//! never the library), and when the day's downloads come back.

use std::path::PathBuf;

use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use super::*;
use crate::db::unix_now;

/// A search is sent again after this long; until then, what it found (nothing included) is
/// what's shown.
pub const SEARCH_KEPT: i64 = 7 * 24 * 3600;

/// One version of a file: its id, size and date.
pub type Version = (i64, i64, i64);

/// A subtitle fetched for a file version.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    /// Its language, as OpenSubtitles names it.
    pub language: String,
    /// The release it was made for.
    pub release: Option<String>,
    /// For the hard of hearing.
    pub hearing_impaired: bool,
}

/// A language as OpenSubtitles names it ("en", "pt-br"): two letters, or two and a region, in
/// lower case. None for anything else.
pub fn language_code(language: &str) -> Option<String> {
    let language = language.trim().to_ascii_lowercase();
    let (main, region) = language.split_once('-').unwrap_or((&language, "ab"));
    let letters = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_lowercase());
    (letters(main) && letters(region)).then_some(language)
}

/// A title's TMDB and IMDb ids, and an episode's season and number.
type Ids = (Option<i64>, Option<String>, Option<i64>, Option<i64>);

/// What to ask OpenSubtitles for a file (`hash` worked out from it) in `language`: its title's
/// TMDB and IMDb ids, and an episode's season and number.
pub async fn wanted(
    pool: &SqlitePool,
    file_id: i64,
    hash: Option<String>,
    language: &str,
) -> sqlx::Result<Wanted> {
    let ids: Option<Ids> = sqlx::query_as(
        "SELECT COALESCE(m.match_tmdb_id, m.tmdb_id, s.match_tmdb_id, s.tmdb_id),
             COALESCE(tm.imdb_id, m.imdb_id, ts.imdb_id, s.imdb_id),
             CASE WHEN f.series_id IS NOT NULL THEN f.season END,
             CASE WHEN f.series_id IS NOT NULL THEN f.episode END
         FROM media_files f
         LEFT JOIN movies m ON m.id = f.movie_id
         LEFT JOIN tmdb_movies tm ON tm.tmdb_id = m.match_tmdb_id
         LEFT JOIN series s ON s.id = f.series_id
         LEFT JOIN tmdb_series ts ON ts.tmdb_id = s.match_tmdb_id
         WHERE f.id = ?",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let (tmdb_id, imdb_id, season, episode) = ids.unwrap_or_default();
    Ok(Wanted {
        hash,
        tmdb_id,
        imdb_id,
        episode: season.zip(episode),
        language: language.to_owned(),
    })
}

/// The track id a fetched subtitle has in a file's subtitle list: `o` and 8 hex digits from its
/// language, as there's one per language.
pub fn track_id(language: &str) -> String {
    let digest = Sha256::digest(format!("opensubtitles:{language}").as_bytes());
    let hex: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    format!("o{hex}")
}

/// Where a fetched subtitle is kept, inside the subtitle folder.
pub fn fetched_path(subtitles: &Path, version: Version, language: &str) -> PathBuf {
    let (id, size, modified) = version;
    subtitles
        .join("fetched")
        .join(format!("{id}-{size}-{modified}"))
        .join(format!("{language}.srt"))
}

/// The subtitles fetched for a file as it is now.
pub async fn fetched_for(pool: &SqlitePool, file_id: i64) -> sqlx::Result<Vec<Fetched>> {
    let rows: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT o.language, o.release, o.hearing_impaired FROM opensubtitles_fetched o
         JOIN media_files f ON f.id = o.file_id
         WHERE o.file_id = ? AND o.size = f.size AND o.modified_at = f.modified_at
         ORDER BY o.language",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(language, release, hearing_impaired)| Fetched {
            language,
            release,
            hearing_impaired,
        })
        .collect())
}

/// What the last search for a file version and language found, when it was sent less than
/// [`SEARCH_KEPT`] ago.
pub async fn remembered(
    pool: &SqlitePool,
    version: Version,
    language: &str,
) -> anyhow::Result<Option<Vec<Found>>> {
    let (id, size, modified) = version;
    let results: Option<String> = sqlx::query_scalar(
        "SELECT results FROM opensubtitles_searches
         WHERE file_id = ? AND size = ? AND modified_at = ? AND language = ? AND searched_at > ?",
    )
    .bind(id)
    .bind(size)
    .bind(modified)
    .bind(language)
    .bind(unix_now() - SEARCH_KEPT)
    .fetch_optional(pool)
    .await?;
    Ok(results.map(|r| serde_json::from_str(&r)).transpose()?)
}

/// Remembers what a search found.
pub async fn remember(
    pool: &SqlitePool,
    version: Version,
    language: &str,
    found: &[Found],
) -> anyhow::Result<()> {
    let (id, size, modified) = version;
    sqlx::query(
        "INSERT INTO opensubtitles_searches (file_id, size, modified_at, language, searched_at, results)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id, language) DO UPDATE SET size = excluded.size,
             modified_at = excluded.modified_at, searched_at = excluded.searched_at,
             results = excluded.results",
    )
    .bind(id)
    .bind(size)
    .bind(modified)
    .bind(language)
    .bind(unix_now())
    .bind(serde_json::to_string(found)?)
    .execute(pool)
    .await?;
    Ok(())
}

/// Keeps a fetched subtitle: its file in the subtitle folder, then its row.
pub async fn keep(
    pool: &SqlitePool,
    subtitles: &Path,
    version: Version,
    found: &Found,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let path = fetched_path(subtitles, version, &found.language);
    let dir = path.parent().expect("the path has a folder");
    tokio::fs::create_dir_all(dir).await?;
    let part = path.with_extension("srt.part");
    tokio::fs::write(&part, bytes).await?;
    tokio::fs::rename(&part, &path).await?;
    let (id, size, modified) = version;
    sqlx::query(
        "INSERT INTO opensubtitles_fetched (file_id, size, modified_at, language, subtitle_file_id,
             release, hearing_impaired, fetched_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id, language) DO UPDATE SET size = excluded.size,
             modified_at = excluded.modified_at, subtitle_file_id = excluded.subtitle_file_id,
             release = excluded.release, hearing_impaired = excluded.hearing_impaired,
             fetched_at = excluded.fetched_at",
    )
    .bind(id)
    .bind(size)
    .bind(modified)
    .bind(&found.language)
    .bind(found.id)
    .bind(&found.release)
    .bind(found.hearing_impaired)
    .bind(unix_now())
    .execute(pool)
    .await?;
    Ok(())
}

/// When the day's downloads come back, while they're used up.
pub async fn used_up_until(pool: &SqlitePool) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT reset_at FROM opensubtitles_quota WHERE id = 1 AND reset_at > ?")
        .bind(unix_now())
        .fetch_optional(pool)
        .await
}

/// Notes that the day's downloads are used up until `reset_at`.
pub async fn used_up(pool: &SqlitePool, reset_at: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO opensubtitles_quota (id, reset_at) VALUES (1, ?)
         ON CONFLICT (id) DO UPDATE SET reset_at = excluded.reset_at",
    )
    .bind(reset_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Removes fetched subtitles of file versions no longer in the library as they were. Returns
/// how many folders it removed.
pub async fn clean_fetched(pool: &SqlitePool, subtitles: &Path) -> anyhow::Result<usize> {
    let mut entries = match tokio::fs::read_dir(subtitles.join("fetched")).await {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let current: std::collections::HashSet<String> = sqlx::query_as::<_, (i64, i64, i64)>(
        "SELECT id, size, modified_at FROM media_files WHERE missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, size, modified)| format!("{id}-{size}-{modified}"))
    .collect();
    let mut removed = 0;
    while let Some(entry) = entries.next_entry().await? {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let ours = name.split('-').count() == 3
            && name
                .split('-')
                .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if ours && !current.contains(&name) {
            tokio::fs::remove_dir_all(entry.path()).await?;
            removed += 1;
        }
    }
    Ok(removed)
}
