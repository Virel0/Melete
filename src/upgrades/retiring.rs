//! Retiring replaced files once their upgrade is in: each moved to its library's trash, or kept
//! when anything is unclear.

use super::*;

#[derive(sqlx::FromRow)]
pub(super) struct Imported {
    pub(super) id: i64,
    pub(super) replaces: String,
    pub(super) placed: String,
    pub(super) updated_at: i64,
}

/// A library file now, with its probed duration if its probe is current.
#[derive(sqlx::FromRow)]
pub(super) struct Current {
    pub(super) id: i64,
    pub(super) movie_id: Option<i64>,
    pub(super) series_id: Option<i64>,
    pub(super) root: String,
    pub(super) rel_path: String,
    pub(super) size: i64,
    pub(super) modified_at: i64,
    pub(super) season: Option<i64>,
    pub(super) episode: Option<i64>,
    pub(super) episode_end: Option<i64>,
    pub(super) missing: bool,
    pub(super) duration: Option<f64>,
    pub(super) probe_failed: bool,
}

pub(super) const CURRENT: &str =
    "SELECT f.id, f.movie_id, f.series_id, l.path AS root, f.rel_path, f.size, f.modified_at,
        f.season, f.episode, f.episode_end, f.missing_since IS NOT NULL AS missing,
        json_extract(p.info, '$.duration_seconds') AS duration,
        coalesce(p.error IS NOT NULL, 0) AS probe_failed
    FROM media_files f JOIN libraries l ON l.id = f.library_id
    LEFT JOIN media_probes p
        ON p.file_id = f.id AND p.size = f.size AND p.modified_at = f.modified_at";

/// What became of an imported upgrade's replaced files.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RetireReport {
    /// Replaced files moved to their library's trash.
    pub trashed: usize,
    /// Replaced files left where they are, as anything unclear keeps them.
    pub kept: usize,
    /// Upgrades still waiting for their scan or probe.
    pub waiting: usize,
}

/// Deals with imported upgrades: moves each replaced file to the trash once its successor is
/// scanned, probed and plays about as long, or keeps it with the reason.
pub async fn retire(pool: &SqlitePool) -> anyhow::Result<RetireReport> {
    let mut report = RetireReport::default();
    let imported: Vec<Imported> = sqlx::query_as(
        "SELECT id, replaces, placed, updated_at FROM downloads
         WHERE state = 'imported' AND retired_at IS NULL AND replaces <> '[]'",
    )
    .fetch_all(pool)
    .await?;
    for download in imported {
        let replaces: Vec<Replaced> = serde_json::from_str(&download.replaces)?;
        let placed: Vec<PathBuf> = serde_json::from_str::<Vec<String>>(&download.placed)?
            .into_iter()
            .map(PathBuf::from)
            .collect();
        // The new files, found by the paths the import put them at.
        let mut new_files = Vec::new();
        let mut missing_new = false;
        for library_id in replaces
            .iter()
            .map(|r| r.library_id)
            .collect::<std::collections::BTreeSet<_>>()
        {
            let files: Vec<Current> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "{CURRENT} WHERE f.library_id = ? AND f.missing_since IS NULL"
            )))
            .bind(library_id)
            .fetch_all(pool)
            .await?;
            new_files.extend(
                files
                    .into_iter()
                    .filter(|f| placed.contains(&Path::new(&f.root).join(&f.rel_path))),
            );
        }
        if new_files.is_empty() {
            missing_new = true;
        }
        let unprobed = new_files
            .iter()
            .any(|f| f.duration.is_none() && !f.probe_failed);
        if missing_new || unprobed {
            if unix_now() - download.updated_at < WAIT_SECONDS {
                report.waiting += 1;
                continue;
            }
            let why = if missing_new {
                "kept the old file: the new one hasn't shown up in a scan"
            } else {
                "kept the old file: the new one hasn't been probed"
            };
            finish(pool, download.id, why).await?;
            report.kept += replaces.len();
            continue;
        }

        let mut notes = Vec::new();
        for old in &replaces {
            match retire_one(pool, download.id, old, &new_files).await? {
                Ok(()) => report.trashed += 1,
                Err(why) => {
                    report.kept += 1;
                    notes.push(format!("kept {}: {why}", old.rel_path));
                }
            }
        }
        let trashed = replaces.len() - notes.len();
        let mut message = Vec::new();
        if trashed > 0 {
            message.push(format!(
                "in the library; the {} it replaced in the trash for 30 days",
                if trashed == 1 {
                    "file".to_owned()
                } else {
                    format!("{trashed} files")
                }
            ));
        }
        message.extend(notes);
        finish(pool, download.id, &message.join("; ")).await?;
    }
    Ok(report)
}

pub(super) async fn finish(pool: &SqlitePool, id: i64, message: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE downloads SET retired_at = ?, message = ? WHERE id = ?")
        .bind(unix_now())
        .bind(message)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Moves one replaced file to the trash, or says why it stays. A move an earlier run finished
/// without recording it (a crash, a failed write) is recognised and recorded.
pub(super) async fn retire_one(
    pool: &SqlitePool,
    download_id: i64,
    old: &Replaced,
    new_files: &[Current],
) -> anyhow::Result<Result<(), String>> {
    let trash_path = Path::new(TRASH)
        .join(download_id.to_string())
        .join(&old.rel_path)
        .to_string_lossy()
        .into_owned();
    if already_trashed(pool, download_id, old).await? {
        return Ok(Ok(()));
    }
    let now: Option<Current> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{CURRENT} WHERE f.id = ?")))
            .bind(old.file_id)
            .fetch_optional(pool)
            .await?;
    let Some(now) = now else {
        return Ok(Err("it's no longer in the library".into()));
    };
    let Some(successor) = successor(old, new_files) else {
        return Ok(Err("no new file for exactly what it holds".into()));
    };
    if successor.id == old.file_id {
        return Ok(Err("the new file is this file".into()));
    }
    if (successor.movie_id, successor.series_id) != (now.movie_id, now.series_id) {
        return Ok(Err("the new file isn't in the same title's folder".into()));
    }
    let root = PathBuf::from(&now.root);
    let (Some(from), Some(to), Some(next)) = (
        inside(&root, &old.rel_path),
        inside(&root, &trash_path),
        inside(&root, &successor.rel_path),
    ) else {
        return Ok(Err("its path isn't a plain one inside the library".into()));
    };

    let unchanged = !now.missing
        && now.size == old.size
        && now.modified_at == old.modified_at
        && now.rel_path == old.rel_path;
    if unchanged && let Err(why) = check_lengths(successor, &now) {
        return Ok(Err(why));
    }
    let (size, modified) = (old.size, old.modified_at);
    let (next_size, next_modified) = (successor.size, successor.modified_at);
    let (from_moved, to_moved) = (from.clone(), to.clone());
    let moved = tokio::task::spawn_blocking(move || {
        move_to_trash(
            from,
            to,
            next,
            unchanged,
            (size, modified),
            (next_size, next_modified),
        )
    })
    .await?;
    if let Err(why) = moved {
        return Ok(Err(why));
    }
    let recorded = record_trashed(pool, download_id, old, &trash_path, successor).await;
    if let Err(err) = recorded {
        // Unrecorded, it would sit in the trash forever: put it back.
        put_back(from_moved, to_moved, &err).await?;
        return Err(err.into());
    }
    Ok(Ok(()))
}

/// Whether this download's move of the file to the trash is already recorded.
async fn already_trashed(
    pool: &SqlitePool,
    download_id: i64,
    old: &Replaced,
) -> anyhow::Result<bool> {
    let recorded: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM trashed_files WHERE download_id = ? AND rel_path = ? AND library_id = ?",
    )
    .bind(download_id)
    .bind(&old.rel_path)
    .bind(old.library_id)
    .fetch_optional(pool)
    .await?;
    Ok(recorded.is_some())
}

/// The new file that takes the replaced file's place, if exactly one does.
fn successor<'a>(old: &Replaced, new_files: &'a [Current]) -> Option<&'a Current> {
    // Its successor: the one new file for a movie, the new file for the same episodes, and
    // of the same title: a loose movie file's upgrade lands beside it as a title of its own,
    // and trashing the old one would leave the wanted movie pointing at nothing.
    if old.season.is_none() {
        match new_files {
            [one] => Some(one),
            _ => None,
        }
    } else {
        new_files.iter().find(|f| {
            (f.season, f.episode, f.episode_end.or(f.episode))
                == (old.season, old.episode, old.episode_end.or(old.episode))
        })
    }
}

/// Says why the old file stays when ffprobe can't tell both lengths or the new file plays
/// noticeably longer or shorter.
fn check_lengths(successor: &Current, now: &Current) -> Result<(), String> {
    let (Some(new_length), Some(old_length)) = (successor.duration, now.duration) else {
        return Err("ffprobe couldn't tell both files' lengths".into());
    };
    if !same_length(new_length, old_length) {
        return Err(format!(
            "the new file plays {:.0} min against its {:.0} min, so it may be another cut",
            new_length / 60.0,
            old_length / 60.0
        ));
    }
    Ok(())
}

/// On disk, moves the replaced file into the trash once both files are as scanned, or
/// finishes an earlier run's move; says why when it can't. Blocking.
fn move_to_trash(
    from: PathBuf,
    to: PathBuf,
    next: PathBuf,
    unchanged: bool,
    (size, modified): (i64, i64),
    (next_size, next_modified): (i64, i64),
) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let stat = |p: &Path| std::fs::symlink_metadata(p).ok();
    let seconds = |m: &std::fs::Metadata| {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
    };
    let is = |m: &std::fs::Metadata, len: i64, at: i64| {
        m.file_type().is_file() && m.len() as i64 == len && seconds(m) == Some(at)
    };
    // An earlier run's move: finished (only this download puts files at this name), or
    // stopped between the link and the unlink.
    match (stat(&from), stat(&to)) {
        (None, Some(t)) if t.file_type().is_file() && t.len() as i64 == size => {
            return Ok(());
        }
        (Some(f), Some(t))
            if f.file_type().is_file() && f.ino() == t.ino() && f.dev() == t.dev() =>
        {
            return std::fs::remove_file(&from).map_err(|err| err.to_string());
        }
        _ => {}
    }
    if !unchanged {
        return Err("it changed since the upgrade was grabbed".into());
    }
    // The new file must be there now, as scanned: never leave neither.
    if !stat(&next).is_some_and(|m| is(&m, next_size, next_modified)) {
        return Err("the new file isn't on disk as it was scanned".into());
    }
    if !stat(&from).is_some_and(|m| is(&m, size, modified)) {
        return Err("on disk it isn't the file that was scanned".into());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("making the trash folder: {err}"))?;
    }
    crate::transfer::move_without_replacing(&from, &to)
}

/// Records the file as trashed, marks it gone from the library and gives its successor each
/// user's progress, in one transaction.
async fn record_trashed(
    pool: &SqlitePool,
    download_id: i64,
    old: &Replaced,
    trash_path: &str,
    successor: &Current,
) -> sqlx::Result<()> {
    let at = unix_now();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, download_id, replaced_by,
                 trashed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(old.library_id)
    .bind(&old.rel_path)
    .bind(trash_path)
    .bind(old.size)
    .bind(download_id)
    .bind(&successor.rel_path)
    .bind(at)
    .execute(&mut *tx)
    .await?;
    // Gone from the library now, rather than at the next scan.
    sqlx::query("UPDATE media_files SET missing_since = coalesce(missing_since, ?) WHERE id = ?")
        .bind(at)
        .bind(old.file_id)
        .execute(&mut *tx)
        .await?;
    // Watched before is watched still, and where a viewer was goes on: the two play as long.
    crate::library::carry_progress(&mut tx, old.file_id, successor.id, true).await?;
    tx.commit().await
}

/// Moves a file whose trashing couldn't be recorded back out of the trash, logging when that
/// fails too.
pub(super) async fn put_back(
    from_moved: PathBuf,
    to_moved: PathBuf,
    err: &sqlx::Error,
) -> anyhow::Result<()> {
    let back = tokio::task::spawn_blocking(move || {
        crate::transfer::move_without_replacing(&to_moved, &from_moved)
    })
    .await?;
    if let Err(why) = back {
        tracing::error!(error = %err, why, "a trashed file couldn't be recorded or put back");
    }
    Ok(())
}
