//! Trailer downloads (`MELETE_TRAILER_DOWNLOADS`, off by default): each library title's YouTube
//! trailers and teasers from TMDB's video list, fetched after its details and again whenever
//! they are, one request a title by the TMDB id it's matched to, only while downloads are on.
//! Off, nothing is asked. Which one a title gets is [`chosen`] from the list.

use serde::Deserialize;

use super::*;
use crate::tmdb::Video;

/// A trailer or teaser kept for a title, as TMDB lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trailer {
    /// The YouTube video id.
    pub key: String,
    /// Its name, as TMDB has it.
    pub name: String,
    /// Its language, ISO 639-1 ("en"); None when TMDB has none.
    pub language: Option<String>,
    /// "Trailer" or "Teaser".
    pub kind: String,
    /// Whether the studio or network published it.
    pub official: bool,
    /// Its height in pixels.
    pub size: i64,
    /// When it was published.
    pub published_at: Option<String>,
}

/// Whether `key` is a YouTube video id: 11 letters, digits, `-` or `_`. Anything else is left
/// out, as it goes into the downloader's address.
fn youtube_id(key: &str) -> bool {
    key.len() == 11
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The trailers and teasers among a title's videos that YouTube hosts.
pub fn kept(videos: Vec<Video>) -> Vec<Trailer> {
    videos
        .into_iter()
        .filter(|v| v.site == "YouTube" && youtube_id(&v.key))
        .filter(|v| v.kind == "Trailer" || v.kind == "Teaser")
        .map(|v| Trailer {
            key: v.key,
            name: v.name,
            language: v.language,
            kind: v.kind,
            official: v.official,
            size: v.size,
            published_at: v.published_at,
        })
        .collect()
}

/// The languages a title's trailer is looked for in, best first: its own (`original`, as TMDB
/// gives it), the metadata language (`metadata`, a TMDB language such as `fr-FR`), English.
pub fn languages<'a>(original: Option<&'a str>, metadata: &'a str) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for code in [
        original,
        Some(crate::tmdb::language::code(metadata)),
        Some("en"),
    ]
    .into_iter()
    .flatten()
    {
        if !code.is_empty() && !out.contains(&code) {
            out.push(code);
        }
    }
    out
}

/// The trailer a title gets: in the first of `languages` it has one in; a trailer before a
/// teaser, an official one before others, then the largest and newest. None when there's none
/// in any of them.
pub fn chosen<'a>(trailers: &'a [Trailer], languages: &[&str]) -> Option<&'a Trailer> {
    let rank = |t: &Trailer| {
        let language = t.language.as_deref()?;
        languages.iter().position(|l| *l == language)
    };
    trailers
        .iter()
        .filter_map(|t| rank(t).map(|r| (r, t)))
        .min_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then((a.kind != "Trailer").cmp(&(b.kind != "Trailer")))
                .then(b.official.cmp(&a.official))
                .then(b.size.cmp(&a.size))
                .then(b.published_at.cmp(&a.published_at))
        })
        .map(|(_, t)| t)
}

/// Fetches the videos of present titles whose details were fetched since their videos were (or
/// that have none yet), while trailer downloads are on. Stops when TMDB refuses the token or
/// asks for a long pause; other failures skip a title until the next run. Returns how many
/// titles got theirs.
pub async fn refresh_trailers(pool: &SqlitePool, tmdb: &tmdb::Client) -> anyhow::Result<usize> {
    if !crate::settings::trailer_downloads(pool).await? {
        return Ok(0);
    }
    let metadata = tmdb.language().to_owned();
    let now = unix_now();
    let mut done = 0;
    for (table, titles, tv) in [
        ("tmdb_movies", "movies", false),
        ("tmdb_series", "series", true),
    ] {
        let due: Vec<(i64, Option<String>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT DISTINCT t.tmdb_id, t.original_language FROM {table} t
             JOIN {titles} x ON x.match_tmdb_id = t.tmdb_id
             WHERE x.missing_since IS NULL
               AND (t.videos_fetched_at IS NULL OR t.videos_fetched_at < t.fetched_at)"
        )))
        .fetch_all(pool)
        .await?;
        for (id, original) in due {
            let wanted = languages(original.as_deref(), &metadata);
            match tmdb.videos(tv, id, &wanted).await {
                Ok(videos) => {
                    let trailers = kept(videos.unwrap_or_default());
                    sqlx::query(sqlx::AssertSqlSafe(format!(
                        "UPDATE {table} SET videos = ?, videos_fetched_at = ? WHERE tmdb_id = ?"
                    )))
                    .bind(serde_json::to_string(&trailers)?)
                    .bind(now)
                    .bind(id)
                    .execute(pool)
                    .await?;
                    done += 1;
                }
                Err(err) => skip(
                    err,
                    "videos",
                    &id.to_string(),
                    &mut RefreshReport::default(),
                )?,
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests;
