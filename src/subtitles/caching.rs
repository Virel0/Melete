//! Cache keys tied to each file's version, and removing entries nothing will ask for again.

use super::*;

/// Cache key for an embedded track, tied to the file's version.
pub fn embedded_key(file_id: i64, index: i64, size: i64, modified_at: i64) -> String {
    format!("{file_id}-s{index}-{size}-{modified_at}")
}

/// Cache key for a file beside the video, tied to that file's name and version. It ends in
/// "-u" since files that aren't UTF-8 are read in their own encoding: copies cached before,
/// garbled, have keys without it, which the cleanup drops, so they're converted again.
pub fn sidecar_key(file_id: i64, sidecar: &Sidecar) -> String {
    let digest = Sha256::digest(sidecar.file.as_bytes());
    let name_hash: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{file_id}-x{name_hash}-{}-{}-u",
        sidecar.size, sidecar.modified
    )
}

/// How long a failure marker or an abandoned temporary file stays in the cache.
pub(super) const FAILURE_KEPT: Duration = Duration::from_secs(24 * 3600);
pub(super) const PARTIAL_KEPT: Duration = Duration::from_secs(3600);

/// Removes cache entries nothing will ask for again: those of files no longer in the
/// library, of videos and subtitle files that changed (the key holds their size and date),
/// failure markers after a day and abandoned temporary files after an hour; fonts pulled out
/// of videos likewise. Names it doesn't
/// recognise are left alone. Returns how many files it removed.
pub async fn clean_cache(pool: &sqlx::SqlitePool, cache_dir: &Path) -> anyhow::Result<usize> {
    let entries = cache_entries(cache_dir).await?;
    let (mut doomed, judged) = sort_entries(entries);
    let present = present_files(pool, &judged).await?;
    outdated(judged, &present, &mut doomed).await?;
    remove_entries(cache_dir, doomed).await
}

/// The cache's entries by name, with when each was last changed; none when there's no cache.
async fn cache_entries(cache_dir: &Path) -> anyhow::Result<Vec<(String, SystemTime)>> {
    let entries: Vec<(String, SystemTime)> = {
        let dir = cache_dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> std::io::Result<Vec<(String, SystemTime)>> {
            let listing = match std::fs::read_dir(&dir) {
                Ok(listing) => listing,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(err) => return Err(err),
            };
            let mut entries = Vec::new();
            for entry in listing {
                let entry = entry?;
                let modified = entry.metadata()?.modified().unwrap_or(SystemTime::now());
                if let Ok(name) = entry.file_name().into_string() {
                    entries.push((name, modified));
                }
            }
            Ok(entries)
        })
        .await??
    };
    Ok(entries)
}

/// Entries to remove whatever their file (failure markers after a day, abandoned temporary
/// files after an hour), and those to judge against their file, each with its file id and key.
fn sort_entries(entries: Vec<(String, SystemTime)>) -> (Vec<String>, Vec<(String, i64, String)>) {
    let age = |modified: SystemTime| {
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
    };
    // "{file id}-{s|x}...{.vtt|.ass}[.failed]" for each entry worth judging.
    let mut doomed: Vec<String> = Vec::new();
    let mut judged: Vec<(String, i64, String)> = Vec::new();
    for (name, modified) in entries {
        if name.starts_with('.') {
            if name.ends_with(".part") && age(modified) > PARTIAL_KEPT {
                doomed.push(name);
            }
            continue;
        }
        // Fonts pulled out of a video: "fonts-{file id}-{size}-{modified}", a folder, or the
        // same with ".none" for a video without any.
        if let Some(version) = name.strip_prefix("fonts-") {
            let version = version.strip_suffix(".none").unwrap_or(version);
            if let Some(file_id) = version.split('-').next().and_then(|id| id.parse().ok()) {
                judged.push((name.clone(), file_id, format!("fonts-{version}")));
            }
            continue;
        }
        let failed = name.ends_with(".failed");
        let stem = name.trim_end_matches(".failed");
        let Some(key) = stem
            .strip_suffix(".vtt")
            .or_else(|| stem.strip_suffix(".ass"))
        else {
            continue;
        };
        let Some(file_id) = key.split('-').next().and_then(|id| id.parse::<i64>().ok()) else {
            continue;
        };
        if failed && age(modified) > FAILURE_KEPT {
            doomed.push(name);
            continue;
        }
        judged.push((name.clone(), file_id, key.to_owned()));
    }
    (doomed, judged)
}

/// The present files these entries belong to, with where they are.
async fn present_files(
    pool: &sqlx::SqlitePool,
    judged: &[(String, i64, String)],
) -> anyhow::Result<HashMap<i64, (i64, i64, PathBuf)>> {
    let ids: Vec<i64> = judged.iter().map(|(_, id, _)| *id).collect();
    let present: HashMap<i64, (i64, i64, PathBuf)> = sqlx::query_as::<_, (i64, i64, i64, String)>(
        "SELECT f.id, f.size, f.modified_at, l.path || '/' || f.rel_path
         FROM media_files f JOIN libraries l ON l.id = f.library_id
         WHERE f.missing_since IS NULL AND f.id IN (SELECT value FROM json_each(?))",
    )
    .bind(serde_json::to_string(&ids)?)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, size, modified, path)| (id, (size, modified, PathBuf::from(path))))
    .collect();
    Ok(present)
}

/// Adds to `doomed` the judged entries whose file is gone or has changed, or whose subtitle
/// file beside the video is no longer there as it was.
async fn outdated(
    judged: Vec<(String, i64, String)>,
    present: &HashMap<i64, (i64, i64, PathBuf)>,
    doomed: &mut Vec<String>,
) -> anyhow::Result<()> {
    // The subtitle files beside each video now, by key; read once per video.
    let mut sidecar_keys: HashMap<i64, Vec<String>> = HashMap::new();
    for (name, file_id, key) in judged {
        let Some((size, modified, video)) = present.get(&file_id) else {
            doomed.push(name);
            continue;
        };
        let rest = &key[key.find('-').map_or(key.len(), |i| i + 1)..];
        let current = if let Some(version) = key.strip_prefix("fonts-") {
            version == format!("{file_id}-{size}-{modified}")
        } else if rest.starts_with('s') || rest.starts_with('o') {
            // "{id}-s{index}-{size}-{modified}", and "{id}-o{language hash}-…" for a subtitle
            // fetched for it: the video's version must match.
            key.ends_with(&format!("-{size}-{modified}"))
        } else if rest.starts_with('x') {
            let keys = match sidecar_keys.entry(file_id) {
                std::collections::hash_map::Entry::Occupied(known) => known.into_mut(),
                std::collections::hash_map::Entry::Vacant(new) => {
                    let video = video.clone();
                    let found = tokio::task::spawn_blocking(move || sidecars(&video))
                        .await?
                        .unwrap_or_default();
                    new.insert(found.iter().map(|s| sidecar_key(file_id, s)).collect())
                }
            };
            keys.contains(&key)
        } else {
            true
        };
        if !current {
            doomed.push(name);
        }
    }
    Ok(())
}

/// Removes the entries, files or folders, saying how many were removed.
async fn remove_entries(cache_dir: &Path, doomed: Vec<String>) -> anyhow::Result<usize> {
    let mut removed = 0;
    for name in doomed {
        // Fonts are kept in folders, and pulled out into hidden temporary ones.
        let path = cache_dir.join(&name);
        let gone = match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_dir() => tokio::fs::remove_dir_all(&path).await,
            Ok(_) => tokio::fs::remove_file(&path).await,
            Err(err) => Err(err),
        };
        match gone {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(removed)
}
