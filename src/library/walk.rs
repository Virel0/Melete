//! Walking a library's folder, all of it or one title folder, for its videos or music. Folders
//! holding an `.ignore` or `.melete-ignore` file are skipped and listed apart, so what's
//! recorded under them stays as it was rather than going missing.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, bail};

use super::extras;
use super::scan::{
    AUDIO_EXTENSIONS, FoundFile, Media, SPATIAL_EXTENSIONS, has_extension, is_video, unix_seconds,
};
use super::title_files;

/// Files whose presence in a folder skips it.
const IGNORE_MARKERS: &[&str] = &[".ignore", ".melete-ignore"];

/// What a walk found.
#[derive(Default)]
pub(super) struct Walked {
    /// The videos or music files.
    pub(super) found: Vec<FoundFile>,
    /// Names left out (see `ScanReport::skipped`).
    pub(super) skipped: usize,
    /// For video, the extras and the `.nfo` files and artwork directly in title folders.
    pub(super) beside: Vec<FoundFile>,
    /// Folders skipped for their ignore marker, by path in the library.
    pub(super) ignored: Vec<String>,
}

/// Whether `folder` names one title folder: one path component, not `.` or `..`.
pub(super) fn is_title_folder(folder: &str) -> bool {
    !folder.is_empty() && !folder.contains('/') && folder != "." && folder != ".."
}

/// Walks the library at `root`, or only its title folder `folder`, raising `folders` for each
/// folder read, for the scan's progress.
///
/// Any unreadable folder fails the whole walk instead of being skipped, because the files
/// under it would otherwise be recorded as missing. An empty root fails too: that is what
/// an unmounted share looks like. A title folder that isn't there finds nothing.
pub(super) fn walk_from(
    root: &Path,
    media: Media,
    folder: Option<&str>,
    folders: &AtomicUsize,
) -> anyhow::Result<Walked> {
    if fs::read_dir(root)?.next().is_none() {
        bail!("the folder is empty; is the share mounted?");
    }
    let canonical_root = fs::canonicalize(root)?;
    let mut walked = Walked::default();
    let mut pending: Vec<(PathBuf, usize, bool)> = match folder {
        None => vec![(root.to_path_buf(), 0, false)],
        Some(folder) if is_title_folder(folder) && root.join(folder).is_dir() => {
            vec![(root.join(folder), 1, false)]
        }
        Some(_) => Vec::new(),
    };
    while let Some((dir, depth, in_extras)) = pending.pop() {
        let entries: Vec<fs::DirEntry> = fs::read_dir(&dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .collect::<Result<_, _>>()
            .with_context(|| format!("reading {}", dir.display()))?;
        folders.fetch_add(1, Ordering::Relaxed);
        if depth > 0
            && entries
                .iter()
                .any(|e| IGNORE_MARKERS.iter().any(|m| e.file_name() == *m))
        {
            // Every component was checked to be UTF-8 on the way down.
            let rel = dir.strip_prefix(root)?.to_str().unwrap_or_default();
            walked.ignored.push(rel.to_owned());
            continue;
        }
        for entry in entries {
            visit(
                root,
                &canonical_root,
                &entry,
                (media, depth, in_extras),
                &mut pending,
                &mut walked,
            )?;
        }
    }
    Ok(walked)
}

/// Looks at one entry of a folder: a folder to walk next, or a file to keep or leave out.
fn visit(
    root: &Path,
    canonical_root: &Path,
    entry: &fs::DirEntry,
    (media, depth, in_extras): (Media, usize, bool),
    pending: &mut Vec<(PathBuf, usize, bool)>,
    walked: &mut Walked,
) -> anyhow::Result<()> {
    let file_name = entry.file_name();
    let Some(name) = file_name.to_str() else {
        walked.skipped += 1;
        return Ok(());
    };
    if name.starts_with('.') {
        return Ok(());
    }
    let path = entry.path();
    let file_type = entry.file_type()?;
    let metadata = if file_type.is_dir() {
        // Depth 0 holds the title folders themselves, which may have any name.
        let extras = media == Media::Video && depth > 0 && extras::is_extras_folder(name);
        pending.push((path, depth + 1, in_extras || extras));
        return Ok(());
    } else if file_type.is_symlink() {
        // Links to files inside the library are followed; links to folders are not, so
        // loops can't happen. Streaming refuses files outside the library, so links
        // there are left out rather than listed and then failing to play.
        match fs::canonicalize(&path) {
            Ok(target) if !target.starts_with(canonical_root) => {
                walked.skipped += 1;
                return Ok(());
            }
            Ok(target) => match fs::metadata(&target) {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => return Ok(()),
            },
            Err(_) => return Ok(()),
        }
    } else if file_type.is_file() {
        entry.metadata()?
    } else {
        return Ok(());
    };
    let wanted = match media {
        Media::Video => is_video(name) && !in_extras && !extras::has_extra_name(name),
        Media::Audio => {
            has_extension(name, AUDIO_EXTENSIONS) || has_extension(name, SPATIAL_EXTENSIONS)
        }
    };
    let title_file = media == Media::Video && depth == 1 && title_files::is_title_file(name);
    let extra = media == Media::Video && depth > 0 && is_video(name) && !wanted;
    if !wanted && !title_file && !extra {
        return Ok(());
    }
    // Every component was checked to be UTF-8 on the way down.
    let rel_path = path
        .strip_prefix(root)?
        .to_str()
        .unwrap_or_default()
        .to_owned();
    let file = FoundFile {
        rel_path,
        size: i64::try_from(metadata.len())?,
        modified: unix_seconds(metadata.modified().ok()),
    };
    if wanted {
        walked.found.push(file);
    } else {
        walked.beside.push(file);
    }
    Ok(())
}

/// SQL true when `column` (a path in the library) is `?n`'s folder or below it, or `?n` is
/// NULL: a scan's scope, all of the library or one title folder.
pub(super) fn in_scope(column: &str, n: usize) -> String {
    format!(
        "(?{n} IS NULL OR {column} = ?{n} OR substr({column}, 1, length(?{n}) + 1) = ?{n} || '/')"
    )
}

/// SQL true when `column` (a path in the library) is one of the folders in the JSON array
/// `?n`, or below one.
pub(super) fn ignored(column: &str, n: usize) -> String {
    format!(
        "EXISTS (SELECT 1 FROM json_each(?{n}) j WHERE {column} = j.value
             OR substr({column}, 1, length(j.value) + 1) = j.value || '/')"
    )
}

/// Keeps what's recorded under the ignored folders as seen in this scan (`generation`), so it
/// isn't marked missing: `table`'s rows, by their `column` path.
pub(super) async fn keep_ignored(
    tx: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
    (library_id, generation): (i64, i64),
    ignored: &[String],
) -> anyhow::Result<()> {
    if ignored.is_empty() {
        return Ok(());
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET seen_generation = ?1 WHERE library_id = ?2 AND {}",
        self::ignored(column, 3)
    )))
    .bind(generation)
    .bind(library_id)
    .bind(serde_json::to_string(ignored)?)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::library::{Library, LibraryConfig, LibraryKind, scan, scan_folder, sync_config};

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"video").unwrap();
    }

    async fn library(kind: LibraryKind, root: &Path) -> (sqlx::SqlitePool, Library) {
        let pool = crate::db::open_in_memory().await;
        let config = LibraryConfig {
            name: "l".into(),
            kind,
            path: root.into(),
        };
        let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
        (pool, library)
    }

    async fn present(pool: &sqlx::SqlitePool, table: &str) -> Vec<String> {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT rel_path FROM {table} WHERE missing_since IS NULL ORDER BY rel_path"
        )))
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_folder_scan_touches_only_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "A (2000)/A.mkv");
        touch(dir.path(), "B (2001)/B.mkv");
        let (pool, library) = library(LibraryKind::Movies, dir.path()).await;
        scan(&pool, &library).await.unwrap();
        let last_scan: Option<i64> = sqlx::query_scalar("SELECT last_scan_at FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();

        touch(dir.path(), "A (2000)/A.1080p.mkv");
        std::fs::remove_file(dir.path().join("B (2001)/B.mkv")).unwrap();
        touch(dir.path(), "C (2002)/C.mkv");
        let report = scan_folder(&pool, &library, "A (2000)").await.unwrap();
        assert_eq!((report.files, report.added), (2, 1));
        assert_eq!(
            present(&pool, "media_files").await,
            ["A (2000)/A.1080p.mkv", "A (2000)/A.mkv", "B (2001)/B.mkv"],
            "B and C weren't looked at"
        );
        let after: Option<i64> = sqlx::query_scalar("SELECT last_scan_at FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(after, last_scan, "the library wasn't scanned whole");

        scan(&pool, &library).await.unwrap();
        assert_eq!(
            present(&pool, "media_files").await,
            ["A (2000)/A.1080p.mkv", "A (2000)/A.mkv", "C (2002)/C.mkv"]
        );
        assert_eq!(present(&pool, "movies").await, ["A (2000)", "C (2002)"]);

        // A folder gone is marked missing by its own scan; a bad name scans nothing.
        std::fs::remove_dir_all(dir.path().join("C (2002)")).unwrap();
        scan_folder(&pool, &library, "C (2002)").await.unwrap();
        assert_eq!(present(&pool, "movies").await, ["A (2000)"]);
        assert!(!super::is_title_folder("../A (2000)") && !super::is_title_folder(".."));
    }

    #[tokio::test]
    async fn a_folder_marked_to_be_ignored_is_skipped_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Kept (2000)/Kept.mkv");
        touch(dir.path(), "Kept (2000)/Old.mkv");
        touch(dir.path(), "Other (2001)/Other.mkv");
        let (pool, library) = library(LibraryKind::Movies, dir.path()).await;
        scan(&pool, &library).await.unwrap();

        std::fs::write(dir.path().join("Kept (2000)/.melete-ignore"), b"").unwrap();
        std::fs::remove_file(dir.path().join("Kept (2000)/Old.mkv")).unwrap();
        touch(dir.path(), "Kept (2000)/New.mkv");
        touch(dir.path(), "New (2002)/Extras/.ignore");
        touch(dir.path(), "New (2002)/New.mkv");
        let report = scan(&pool, &library).await.unwrap();
        assert_eq!(
            report.newly_missing, 0,
            "nothing under the marked folder goes missing"
        );
        assert_eq!(
            present(&pool, "media_files").await,
            [
                "Kept (2000)/Kept.mkv",
                "Kept (2000)/Old.mkv",
                "New (2002)/New.mkv",
                "Other (2001)/Other.mkv"
            ],
            "and nothing new under it is added"
        );
        assert_eq!(
            present(&pool, "movies").await,
            ["Kept (2000)", "New (2002)", "Other (2001)"]
        );

        std::fs::remove_file(dir.path().join("Kept (2000)/.melete-ignore")).unwrap();
        scan(&pool, &library).await.unwrap();
        assert_eq!(
            present(&pool, "media_files").await[..2],
            ["Kept (2000)/Kept.mkv", "Kept (2000)/New.mkv"],
            "unmarked, it's scanned as before"
        );
    }

    #[tokio::test]
    async fn music_under_a_marked_folder_stays_too() {
        let dir = tempfile::tempdir().unwrap();
        for rel in [
            "Artist/Album/01.flac",
            "Artist/Album/02.flac",
            "Other/Record/01.flac",
        ] {
            touch(dir.path(), rel);
        }
        let (pool, library) = library(LibraryKind::Music, dir.path()).await;
        scan(&pool, &library).await.unwrap();
        std::fs::write(dir.path().join("Artist/.ignore"), b"").unwrap();
        std::fs::remove_file(dir.path().join("Artist/Album/02.flac")).unwrap();
        let report = scan(&pool, &library).await.unwrap();
        assert_eq!(report.newly_missing, 0);
        assert_eq!(present(&pool, "music_tracks").await.len(), 3);
    }

    #[tokio::test]
    async fn a_file_renamed_in_a_folders_scan_keeps_its_watched_state() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "A (2000)/A.mkv");
        touch(dir.path(), "B (2001)/B.mkv");
        let (pool, library) = library(LibraryKind::Movies, dir.path()).await;
        scan(&pool, &library).await.unwrap();
        let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched,
                 play_count, updated_at)
             SELECT ?, id, 0, 1, 1, 1 FROM media_files WHERE rel_path = 'A (2000)/A.mkv'",
        )
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
        // Another folder's scan comes between, then A's file is renamed and its folder scanned.
        scan_folder(&pool, &library, "B (2001)").await.unwrap();
        std::fs::rename(
            dir.path().join("A (2000)/A.mkv"),
            dir.path().join("A (2000)/A.1080p.mkv"),
        )
        .unwrap();
        scan_folder(&pool, &library, "A (2000)").await.unwrap();
        let watched: bool = sqlx::query_scalar(
            "SELECT p.watched FROM playback_progress p JOIN media_files f ON f.id = p.file_id
             WHERE f.rel_path = 'A (2000)/A.1080p.mkv'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(watched);
    }

    #[tokio::test]
    async fn an_import_asks_for_its_own_folder_only() {
        let pool = crate::db::open_in_memory().await;
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path) VALUES
                 (1, 'movies', 'movies', '/media/movies'), (2, 'music', 'music', '/media/music');",
        )
        .execute(&pool)
        .await
        .unwrap();
        let locate = |dir: &'static str| {
            let pool = pool.clone();
            async move { super::super::locate(&pool, Path::new(dir)).await.unwrap() }
        };
        assert_eq!(
            locate("/media/movies/Dune (2021)").await,
            Some((1, Some("Dune (2021)".into())))
        );
        assert_eq!(
            locate("/media/music/Artist/Album (2020)").await,
            Some((2, None))
        );
        assert_eq!(locate("/elsewhere/x").await, None);
    }
}
