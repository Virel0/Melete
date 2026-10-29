//! Subtitles fetched for new imports without anyone asking, off unless languages are set
//! (`MELETE_OPENSUBTITLES_IMPORTS`). A film or episode Melete imported in the last
//! [`IMPORTS_KEPT`] gets, for each of those languages it has no text subtitle in (inside it,
//! beside it, or fetched already), one subtitle made for that very file: searched for as by
//! hand (searches remembered the same way) and fetched only when a result's hash matched, never
//! a guess among others. Once per file version and language, and never past the day's
//! downloads.

use std::collections::HashSet;
use std::path::PathBuf;

use sqlx::SqlitePool;

use super::*;
use crate::db::unix_now;
use crate::languages::Language;

/// How long after its import a file is still new.
pub const IMPORTS_KEPT: i64 = 7 * 24 * 3600;

/// What a run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Subtitles fetched.
    pub fetched: usize,
    /// Languages looked for that had nothing made for the file.
    pub none_made: usize,
}

/// A file a download put in the library, as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Imported {
    id: i64,
    size: i64,
    modified: i64,
    root: String,
    rel_path: String,
}

/// Looks for subtitles new imports lack, with OpenSubtitles as the settings stand.
pub async fn fetch_for_imports(pool: &SqlitePool, subtitles: &Path) -> anyhow::Result<Report> {
    fetch_for_imports_at(pool, subtitles, API).await
}

/// The same, with OpenSubtitles' API at `api`.
pub async fn fetch_for_imports_at(
    pool: &SqlitePool,
    subtitles: &Path,
    api: &str,
) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let config = crate::settings::load(pool).await?;
    let Some(settings) = config.opensubtitles.filter(|s| !s.imports.is_empty()) else {
        return Ok(report);
    };
    let client = Client::with_api(&settings, api)?;
    for file in imported(pool).await? {
        for language in lacking(pool, &file, &settings.imports).await? {
            if used_up_until(pool).await?.is_some() {
                return Ok(report);
            }
            match fetch_one(pool, &client, subtitles, &file, &language).await {
                Ok(true) => report.fetched += 1,
                Ok(false) => report.none_made += 1,
                Err(failure) => {
                    tracing::warn!(error = %failure, "stopped fetching subtitles for imports");
                    return Ok(report);
                }
            }
        }
    }
    Ok(report)
}

/// The films' and episodes' files the downloads imported in the last [`IMPORTS_KEPT`] put in
/// the library, as they are now.
async fn imported(pool: &SqlitePool) -> anyhow::Result<Vec<Imported>> {
    let placed: Vec<String> = sqlx::query_scalar(
        "SELECT placed FROM downloads WHERE state = 'imported' AND updated_at > ? ORDER BY id",
    )
    .bind(unix_now() - IMPORTS_KEPT)
    .fetch_all(pool)
    .await?;
    let libraries: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, path FROM libraries WHERE kind IN ('movies', 'shows')")
            .fetch_all(pool)
            .await?;
    let mut files = Vec::new();
    for path in placed
        .iter()
        .flat_map(|p| serde_json::from_str::<Vec<String>>(p).unwrap_or_default())
    {
        for (library, root) in &libraries {
            let Some(rel_path) = path.strip_prefix(&format!("{}/", root.trim_end_matches('/')))
            else {
                continue;
            };
            let found: Option<(i64, i64, i64)> = sqlx::query_as(
                "SELECT id, size, modified_at FROM media_files
                 WHERE library_id = ? AND rel_path = ? AND missing_since IS NULL",
            )
            .bind(library)
            .bind(rel_path)
            .fetch_optional(pool)
            .await?;
            if let Some((id, size, modified)) = found {
                files.push(Imported {
                    id,
                    size,
                    modified,
                    root: root.clone(),
                    rel_path: rel_path.to_owned(),
                });
            }
        }
    }
    let mut seen = HashSet::new();
    files.retain(|f| seen.insert(f.id));
    Ok(files)
}

/// Which of `languages` the file has no text subtitle in and hasn't been looked for in yet.
/// None while the file as it is now hasn't been probed, so what's inside it is known.
async fn lacking(
    pool: &SqlitePool,
    file: &Imported,
    languages: &[String],
) -> anyhow::Result<Vec<String>> {
    let probe: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT info FROM media_probes WHERE file_id = ? AND size = ? AND modified_at = ?",
    )
    .bind(file.id)
    .bind(file.size)
    .bind(file.modified)
    .fetch_optional(pool)
    .await?;
    let Some((info,)) = probe else {
        return Ok(Vec::new());
    };
    let mut has: Vec<String> = info
        .and_then(|i| serde_json::from_str::<crate::probe::MediaInfo>(&i).ok())
        .map(|i| i.subtitles)
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.text)
        .filter_map(|s| s.language)
        .collect();
    if let Some(video) = on_disk(file).await {
        let beside = tokio::task::spawn_blocking(move || crate::subtitles::sidecars(&video))
            .await?
            .unwrap_or_default();
        has.extend(beside.into_iter().filter_map(|s| s.language));
    }
    has.extend(
        fetched_for(pool, file.id)
            .await?
            .into_iter()
            .map(|f| f.language),
    );
    let tried: Vec<String> = sqlx::query_scalar(
        "SELECT language FROM opensubtitles_import_tries
         WHERE file_id = ? AND size = ? AND modified_at = ?",
    )
    .bind(file.id)
    .bind(file.size)
    .bind(file.modified)
    .fetch_all(pool)
    .await?;
    let has: Vec<Language> = has.iter().filter_map(|c| Language::parse(c)).collect();
    Ok(languages
        .iter()
        .filter(|l| !tried.contains(l))
        .filter(|l| {
            Language::parse(l).is_some_and(|wanted| has.iter().all(|h| wanted.answers(h).is_none()))
        })
        .cloned()
        .collect())
}

/// The file on disk, unless it resolves outside its library (a link) or can't be reached.
async fn on_disk(file: &Imported) -> Option<PathBuf> {
    let root = tokio::fs::canonicalize(&file.root).await.ok()?;
    let path = tokio::fs::canonicalize(root.join(&file.rel_path))
        .await
        .ok()?;
    path.starts_with(&root).then_some(path)
}

/// The subtitle made for `file` in `language`, from what a search found (remembered, or asked
/// now). Without the file's hash nothing can be known to be made for it, so nothing is asked.
async fn made_for(
    pool: &SqlitePool,
    client: &Client,
    file: &Imported,
    language: &str,
) -> anyhow::Result<Option<Found>> {
    let version: Version = (file.id, file.size, file.modified);
    let Some(video) = on_disk(file).await else {
        anyhow::bail!("{} can't be read right now", file.rel_path);
    };
    let Some(hash) = tokio::task::spawn_blocking(move || hash(&video)).await?? else {
        return Ok(None);
    };
    let found = match remembered(pool, version, language).await? {
        Some(found) => found,
        None => {
            let asked = wanted(pool, file.id, Some(hash), language).await?;
            let found = client.search(&asked).await?;
            remember(pool, version, language, &found).await?;
            found
        }
    };
    Ok(found.into_iter().find(|f| f.matches_file))
}

/// Looks for a subtitle made for `file` in `language` and fetches it: true when one was. A
/// failure (the day's downloads used up included) stops the run and leaves the language to try
/// again.
async fn fetch_one(
    pool: &SqlitePool,
    client: &Client,
    subtitles: &Path,
    file: &Imported,
    language: &str,
) -> anyhow::Result<bool> {
    let version: Version = (file.id, file.size, file.modified);
    let made = made_for(pool, client, file, language).await?;
    if let Some(mut made) = made.clone() {
        let bytes = match client.download(made.id).await {
            Ok(bytes) => bytes,
            Err(Failure::Quota(reset_at)) => {
                used_up(pool, reset_at).await?;
                return Err(Failure::Quota(reset_at).into());
            }
            Err(other) => return Err(other.into()),
        };
        // Kept under the language asked for, however OpenSubtitles writes it.
        made.language = language.to_owned();
        keep(pool, subtitles, version, &made, &bytes).await?;
    }
    sqlx::query(
        "INSERT INTO opensubtitles_import_tries (file_id, size, modified_at, language, tried_at,
             fetched) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id, language) DO UPDATE SET size = excluded.size,
             modified_at = excluded.modified_at, tried_at = excluded.tried_at,
             fetched = excluded.fetched",
    )
    .bind(file.id)
    .bind(file.size)
    .bind(file.modified)
    .bind(language)
    .bind(unix_now())
    .bind(made.is_some())
    .execute(pool)
    .await?;
    Ok(made.is_some())
}

#[cfg(test)]
mod tests;
