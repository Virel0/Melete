//! Anime episodes in the library without styled (ASS/SSA) subtitles: no such track in the file
//! and no such file beside it. Most anime releases carry their subtitles as an ASS track, drawn
//! with their signs and styling; one that doesn't (a raw, hardsubs, SRT only) shows plain words
//! or none, so it's noted and a release that has them can be looked for, turning down the
//! group whose copy this is.
//!
//! A series is anime as everywhere else: in a library with the anime category, or wanted as
//! anime. The probe says what's in each file; only files it finds without ASS are looked beside.

use std::path::Path;

use sqlx::SqlitePool;

/// An anime episode whose subtitles aren't styled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unstyled {
    /// The library's series.
    pub series_id: i64,
    /// Its title.
    pub series: String,
    /// The episode's file.
    pub file_id: i64,
    pub season: i64,
    pub episode: i64,
    /// The release group in the file's name, when it has one.
    pub group: Option<String>,
}

/// The anime episodes without styled subtitles, by series, season and episode; or only those of
/// `series`. Specials and files not read as an episode are left out.
pub async fn unstyled(pool: &SqlitePool, series: Option<i64>) -> anyhow::Result<Vec<Unstyled>> {
    let rows: Vec<(i64, String, i64, i64, i64, String, String)> = sqlx::query_as(
        "SELECT s.id, COALESCE(t.name, s.parsed_title), f.id, f.season, f.episode, l.path, f.rel_path
         FROM media_files f
         JOIN series s ON s.id = f.series_id AND s.missing_since IS NULL
         JOIN libraries l ON l.id = f.library_id
         JOIN media_probes p ON p.file_id = f.id AND p.info IS NOT NULL
         LEFT JOIN tmdb_series t ON t.tmdb_id = s.match_tmdb_id
         WHERE f.missing_since IS NULL AND f.season > 0 AND f.episode IS NOT NULL
           AND (?1 IS NULL OR s.id = ?1)
           AND (l.category IS 'anime' OR EXISTS (SELECT 1 FROM wanted_series w
                WHERE w.series_id = s.id AND w.series_type = 'anime'))
           AND NOT EXISTS (SELECT 1 FROM json_each(p.info, '$.subtitles') st
                WHERE json_extract(st.value, '$.codec') IN ('ass', 'ssa'))
         ORDER BY 2, s.id, f.season, f.episode",
    )
    .bind(series)
    .fetch_all(pool)
    .await?;
    let found = tokio::task::spawn_blocking(move || {
        rows.into_iter()
            .filter_map(|(series_id, series, file_id, season, episode, root, rel)| {
                let video = Path::new(&root).join(&rel);
                (!styled_beside(&video)).then(|| Unstyled {
                    series_id,
                    series,
                    file_id,
                    season,
                    episode,
                    group: group_of(&rel),
                })
            })
            .collect()
    })
    .await?;
    Ok(found)
}

/// Whether an ASS/SSA file sits beside the video. A folder that can't be read counts as no.
fn styled_beside(video: &Path) -> bool {
    super::sidecars(video)
        .unwrap_or_default()
        .iter()
        .any(|s| super::is_ass(&s.codec))
}

/// The release group in a file's name, as a release's name is read.
fn group_of(rel_path: &str) -> Option<String> {
    let name = Path::new(rel_path).file_name()?.to_str()?;
    crate::release::parse(name).group
}

#[cfg(test)]
pub(crate) mod tests;
