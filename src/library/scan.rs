//! Walks a library folder and records what it finds. Never changes anything on disk. Music
//! libraries are recorded by `music.rs`, from the same walk.

use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use serde::Serialize;
use sqlx::{SqliteConnection, SqlitePool};

use super::parse::{parse_episode, parse_title};
use super::progress::{Progress, TICK};
use super::{Library, LibraryKind, extras, title_files, walk};

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "ts", "m2ts", "mov", "wmv", "webm", "mpg", "mpeg",
];

/// Music files a music library adopts (audiobooks, .m4b, are left out).
pub(crate) const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv", "ape", "mpc",
];

/// Dolby and Matroska audio a music library adopts too: Dolby Atmos and multichannel
/// music comes as raw E-AC-3, AC-3 or TrueHD, or in Matroska. The tag reader can't open these;
/// ffprobe says what they are (`music_spatial`). Kept out of `AUDIO_EXTENSIONS`, which says what
/// downloads and searches count as tracks.
pub(super) const SPATIAL_EXTENSIONS: &[&str] = &["ec3", "eac3", "ac3", "thd", "mlp", "mka"];

/// What a walk looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Media {
    /// Video files, with extras (trailers, featurettes) apart and samples left out.
    Video,
    Audio,
}

/// What scanning one library found.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    /// Video files found.
    pub files: usize,
    /// Of those, files no earlier scan had seen.
    pub added: usize,
    /// Files known from earlier scans that this scan did not find.
    pub newly_missing: u64,
    /// Episode files whose name gave no season and episode.
    pub unparsed: usize,
    /// Loose files at the root of a shows library, names that aren't UTF-8, and links to files
    /// outside the library (which could never be played).
    pub skipped: usize,
}

pub(super) struct FoundFile {
    /// The file's path relative to the library's folder.
    pub rel_path: String,
    /// Its size in bytes.
    pub size: i64,
    /// When it was last changed, in Unix seconds.
    pub modified: i64,
}

/// Scans one library and records the result in a single transaction.
pub async fn scan(pool: &SqlitePool, library: &Library) -> anyhow::Result<ScanReport> {
    let _one_at_a_time = SCANNING.lock().await;
    if library.kind == LibraryKind::Music {
        return super::music::scan(pool, library).await;
    }
    let progress = Progress::start(pool, library.id, true).await;
    let scanned = scan_video(pool, library, None, &progress).await;
    progress.finish().await;
    scanned
}

/// Scans one title folder of a movies or shows library, `folder` (its path in the library),
/// and records the result: what's under it found or gone, nothing else touched. A music
/// library, or a title that is a loose file rather than a folder, is scanned whole.
pub async fn scan_folder(
    pool: &SqlitePool,
    library: &Library,
    folder: &str,
) -> anyhow::Result<ScanReport> {
    let loose = !library.path.join(folder).is_dir() && library.path.join(folder).exists();
    if library.kind == LibraryKind::Music || loose || !walk::is_title_folder(folder) {
        return scan(pool, library).await;
    }
    let _one_at_a_time = SCANNING.lock().await;
    let progress = Progress::start(pool, library.id, false).await;
    let scanned = scan_video(pool, library, Some(folder), &progress).await;
    progress.finish().await;
    scanned
}

/// One scan at a time, whole or of a folder, so two never record the same files at once.
static SCANNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn scan_video(
    pool: &SqlitePool,
    library: &Library,
    folder: Option<&str>,
    progress: &Progress,
) -> anyhow::Result<ScanReport> {
    let root = library.path.clone();
    let scope = folder.map(str::to_owned);
    let folders = progress.folders();
    let walk = tokio::task::spawn_blocking(move || {
        walk::walk_from(&root, Media::Video, scope.as_deref(), &folders)
    });
    let walked = progress
        .walking(TICK, walk)
        .await?
        .with_context(|| format!("scanning {}", library.path.display()))?;
    let (extras, beside): (Vec<FoundFile>, Vec<FoundFile>) = walked
        .beside
        .into_iter()
        .partition(|f| extras::classify(&f.rel_path).is_some());
    let beside = title_files::by_title(&beside, &walked.found, library.kind == LibraryKind::Shows);
    let nfo_ids =
        title_files::read_changed_nfo(pool, library, &beside, (folder, &walked.ignored)).await?;
    let found = Found {
        videos: &walked.found,
        extras: &extras,
        skipped: walked.skipped,
        scope: folder,
        ignored: &walked.ignored,
    };
    store(pool, library, found, (&beside, &nfo_ids)).await
}

/// What a walk of a video library found.
struct Found<'a> {
    /// The titles' own videos.
    videos: &'a [FoundFile],
    /// Trailers, featurettes and other extras.
    extras: &'a [FoundFile],
    /// Names left out (see [`ScanReport::skipped`]).
    skipped: usize,
    /// The one title folder scanned, when not the whole library.
    scope: Option<&'a str>,
    /// Folders skipped for their ignore marker.
    ignored: &'a [String],
}

/// Lists video or audio files under `root`, with a count of skipped names.
#[cfg(test)]
pub(super) fn walk(root: &Path, media: Media) -> anyhow::Result<(Vec<FoundFile>, usize)> {
    walk::walk_from(root, media, None, &Default::default()).map(|w| (w.found, w.skipped))
}

async fn store(
    pool: &SqlitePool,
    library: &Library,
    Found {
        videos: found,
        extras,
        skipped,
        scope,
        ignored,
    }: Found<'_>,
    (beside, nfo_ids): (&title_files::ByTitle<'_>, &title_files::ReadIds),
) -> anyhow::Result<ScanReport> {
    let now = unix_seconds(Some(SystemTime::now()));
    let mut tx = pool.begin().await?;
    // A folder's scan isn't the library's: when it was last scanned whole stays as it was.
    let generation: i64 = sqlx::query_scalar(
        "UPDATE libraries SET scan_generation = scan_generation + 1,
             last_scan_at = CASE WHEN ?3 IS NULL THEN ?1 ELSE last_scan_at END
         WHERE id = ?2 RETURNING scan_generation",
    )
    .bind(now)
    .bind(library.id)
    .bind(scope)
    .fetch_one(&mut *tx)
    .await?;

    // A title folder renamed stays its title; only a whole scan sees the old one gone.
    if scope.is_none() {
        let renamed = super::renamed::find(&mut tx, library, found, ignored).await?;
        super::renamed::follow(&mut tx, library, &renamed, (found, extras), now).await?;
    }
    let mut report = ScanReport {
        skipped,
        ..ScanReport::default()
    };
    let mut titles: HashMap<&str, i64> = HashMap::new();
    for file in found {
        let (title_path, in_folder) = match file.rel_path.split_once('/') {
            Some((folder, _)) => (folder, true),
            None => (file.rel_path.as_str(), false),
        };
        if library.kind == LibraryKind::Shows && !in_folder {
            report.skipped += 1;
            continue;
        }
        let title_id = match titles.get(title_path) {
            Some(&id) => id,
            None => {
                let id = upsert_title(&mut tx, library, title_path, generation).await?;
                titles.insert(title_path, id);
                id
            }
        };
        let first_seen =
            upsert_file(&mut tx, library, file, title_id, generation, &mut report).await?;
        report.files += 1;
        if first_seen == generation {
            report.added += 1;
        }
    }

    for (title_path, id) in &titles {
        title_files::record(
            &mut tx,
            library,
            *id,
            beside.get(title_path),
            nfo_ids.get(*title_path),
        )
        .await?;
    }
    let titles_table = match library.kind {
        LibraryKind::Shows => "series",
        _ => "movies",
    };
    for (table, column) in [
        ("media_files", "rel_path"),
        ("media_extras", "rel_path"),
        (titles_table, "rel_path"),
    ] {
        walk::keep_ignored(&mut tx, table, column, (library.id, generation), ignored).await?;
    }
    extras::record(&mut tx, library, &titles, extras, (generation, now, scope)).await?;
    report.newly_missing = mark_missing(&mut tx, library, now, (generation, scope)).await?;
    super::successors::carry_renamed(&mut tx, library.id, (generation, now, scope)).await?;

    tx.commit().await?;
    Ok(report)
}

/// Records a found file under its movie or series, with the episode its name gives; returns
/// the generation it was first seen in.
async fn upsert_file(
    tx: &mut SqliteConnection,
    library: &Library,
    file: &FoundFile,
    title_id: i64,
    generation: i64,
    report: &mut ScanReport,
) -> anyhow::Result<i64> {
    let (movie_id, series_id, episode, absolute) = match library.kind {
        LibraryKind::Movies => (Some(title_id), None, None, None),
        LibraryKind::Shows => {
            let (episode, absolute) = episode_of(stem(&file.rel_path));
            if episode.is_none() {
                report.unparsed += 1;
            }
            (None, Some(title_id), episode, absolute)
        }
        LibraryKind::Music => bail!("music libraries are scanned by music.rs"),
    };
    let first_seen: i64 = sqlx::query_scalar(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id, series_id,
                 season, episode, episode_end, absolute_episode, first_seen_generation, seen_generation,
                 added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 -- Came in now (or when last changed, for a file adopted), unless it's another
                 -- file for the same movie or exactly the same episodes: then when the first did.
                 min(?13, ?4, coalesce((SELECT min(o.added_at) FROM media_files o
                     WHERE o.movie_id = ?5
                        OR (?7 IS NOT NULL AND o.series_id = ?6 AND o.season = ?7
                            AND o.episode = ?8
                            AND coalesce(o.episode_end, o.episode) = coalesce(?9, ?8))), ?13)))
             ON CONFLICT (library_id, rel_path) DO UPDATE SET
                 size = excluded.size, modified_at = excluded.modified_at,
                 movie_id = excluded.movie_id, series_id = excluded.series_id,
                 -- A season and episode mapped from the absolute number stay while the name
                 -- still gives that number.
                 mapped = (excluded.season IS NULL AND media_files.mapped = 1
                           AND media_files.absolute_episode IS excluded.absolute_episode),
                 season = CASE WHEN excluded.season IS NULL AND media_files.mapped = 1
                                    AND media_files.absolute_episode IS excluded.absolute_episode
                               THEN media_files.season ELSE excluded.season END,
                 episode = CASE WHEN excluded.season IS NULL AND media_files.mapped = 1
                                     AND media_files.absolute_episode IS excluded.absolute_episode
                                THEN media_files.episode ELSE excluded.episode END,
                 episode_end = excluded.episode_end,
                 absolute_episode = excluded.absolute_episode,
                 seen_generation = excluded.seen_generation, missing_since = NULL,
                 added_at = coalesce(media_files.added_at, excluded.added_at)
             RETURNING first_seen_generation",
        )
        .bind(library.id)
        .bind(&file.rel_path)
        .bind(file.size)
        .bind(file.modified)
        .bind(movie_id)
        .bind(series_id)
        .bind(episode.as_ref().map(|e| e.season))
        .bind(episode.as_ref().map(|e| e.episode))
        .bind(episode.as_ref().and_then(|e| e.episode_end))
        .bind(absolute)
        .bind(generation)
        .bind(generation)
        .bind(unix_seconds(Some(SystemTime::now())))
        .fetch_one(&mut *tx)
        .await?;
    Ok(first_seen)
}

/// Marks the files, then the movies or series, this scan didn't see as missing since now
/// (within its scope: the library, or one title folder); returns how many files.
async fn mark_missing(
    tx: &mut SqliteConnection,
    library: &Library,
    now: i64,
    (generation, scope): (i64, Option<&str>),
) -> anyhow::Result<u64> {
    let titles = match library.kind {
        LibraryKind::Movies => "movies",
        LibraryKind::Shows => "series",
        LibraryKind::Music => bail!("music libraries are scanned by music.rs"),
    };
    let mut newly_missing = 0;
    for table in ["media_files", titles] {
        let marked = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET missing_since = ?1
             WHERE library_id = ?2 AND seen_generation < ?3 AND missing_since IS NULL AND {}",
            walk::in_scope("rel_path", 4)
        )))
        .bind(now)
        .bind(library.id)
        .bind(generation)
        .bind(scope)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if table == "media_files" {
            newly_missing = marked;
        }
    }
    Ok(newly_missing)
}

/// Records a movie or series by its top-level path and returns its id.
async fn upsert_title(
    conn: &mut SqliteConnection,
    library: &Library,
    rel_path: &str,
    generation: i64,
) -> anyhow::Result<i64> {
    let parsed = match library.kind {
        // A loose movie file is named like a release, extension and all.
        LibraryKind::Movies => parse_title(stem(rel_path)),
        LibraryKind::Shows => parse_title(rel_path),
        LibraryKind::Music => bail!("music libraries are scanned by music.rs"),
    };
    let query = match library.kind {
        LibraryKind::Movies => sqlx::query_scalar(
            "INSERT INTO movies (library_id, rel_path, parsed_title, parsed_year, tmdb_id, imdb_id,
                 seen_generation)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, rel_path) DO UPDATE SET
                 parsed_title = excluded.parsed_title, parsed_year = excluded.parsed_year,
                 tmdb_id = excluded.tmdb_id, imdb_id = excluded.imdb_id,
                 seen_generation = excluded.seen_generation, missing_since = NULL
             RETURNING id",
        )
        .bind(library.id)
        .bind(rel_path)
        .bind(&parsed.title)
        .bind(parsed.year)
        .bind(parsed.tmdb_id)
        .bind(&parsed.imdb_id)
        .bind(generation),
        LibraryKind::Shows => sqlx::query_scalar(
            "INSERT INTO series (library_id, rel_path, parsed_title, parsed_year, tmdb_id, tvdb_id,
                 imdb_id, seen_generation)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (library_id, rel_path) DO UPDATE SET
                 parsed_title = excluded.parsed_title, parsed_year = excluded.parsed_year,
                 tmdb_id = excluded.tmdb_id, tvdb_id = excluded.tvdb_id, imdb_id = excluded.imdb_id,
                 seen_generation = excluded.seen_generation, missing_since = NULL
             RETURNING id",
        )
        .bind(library.id)
        .bind(rel_path)
        .bind(&parsed.title)
        .bind(parsed.year)
        .bind(parsed.tmdb_id)
        .bind(parsed.tvdb_id)
        .bind(&parsed.imdb_id)
        .bind(generation),
        LibraryKind::Music => bail!("music libraries are scanned by music.rs"),
    };
    Ok(query.fetch_one(conn).await?)
}

/// The file name of the last path component, without its extension.
fn stem(rel_path: &str) -> &str {
    Path::new(rel_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(rel_path)
}

/// An episode file's season and episode, or failing those, the number it's given counting from
/// the first episode ("Frieren - 05"). Names without "S01E05" are read as release names, which
/// also understand an anime season in the title ("Spy x Family Season 2 - 05").
fn episode_of(stem: &str) -> (Option<super::parse::EpisodeNumber>, Option<i64>) {
    if let Some(episode) = parse_episode(stem) {
        return (Some(episode), None);
    }
    match crate::release::parse(stem).episodes {
        crate::release::Episodes::Standard { season, episodes } => {
            let first = episodes.first().copied();
            let last = episodes.last().copied().filter(|l| Some(*l) != first);
            let episode = first.map(|episode| super::parse::EpisodeNumber {
                season,
                episode,
                episode_end: last,
            });
            (episode, None)
        }
        crate::release::Episodes::Absolute(numbers) if numbers.len() == 1 => {
            (None, Some(numbers[0]).filter(|n| *n > 0))
        }
        _ => (None, None),
    }
}

pub(super) fn is_video(name: &str) -> bool {
    has_extension(name, VIDEO_EXTENSIONS)
}

pub(super) fn has_extension(name: &str, extensions: &[&str]) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| extensions.contains(&e.to_lowercase().as_str()))
}

pub(super) fn unix_seconds(time: Option<SystemTime>) -> i64 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::library::{LibraryConfig, sync_config};
    use std::path::PathBuf;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"video").unwrap();
    }

    async fn setup(kind: LibraryKind, root: &Path) -> (SqlitePool, Library) {
        let pool = db::open_in_memory().await;
        let config = LibraryConfig {
            name: "test".into(),
            kind,
            path: PathBuf::from(root),
        };
        let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
        (pool, library)
    }

    #[tokio::test]
    async fn adopts_movies_and_skips_extras() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            root,
            "The Matrix (1999) [tmdbid-603]/The.Matrix.1999.1080p.mkv",
        );
        touch(root, "The Matrix (1999) [tmdbid-603]/Extras/Making Of.mkv");
        touch(root, "The Matrix (1999) [tmdbid-603]/sample.mkv");
        touch(
            root,
            "The Matrix (1999) [tmdbid-603]/The Matrix-trailer.mp4",
        );
        touch(root, "The Matrix (1999) [tmdbid-603]/movie.nfo");
        touch(root, "Loose.Movie.2010.720p.WEB.mkv");
        touch(root, "Other (2000)/Other.mkv");
        touch(root, ".incoming/Partial.mkv");
        let (pool, library) = setup(LibraryKind::Movies, root).await;

        let report = scan(&pool, &library).await.unwrap();
        assert_eq!(
            report,
            ScanReport {
                files: 3,
                added: 3,
                ..ScanReport::default()
            }
        );

        let movies: Vec<(String, String, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT rel_path, parsed_title, parsed_year, tmdb_id FROM movies ORDER BY rel_path",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            movies,
            vec![
                (
                    "Loose.Movie.2010.720p.WEB.mkv".into(),
                    "Loose Movie".into(),
                    Some(2010),
                    None
                ),
                ("Other (2000)".into(), "Other".into(), Some(2000), None),
                (
                    "The Matrix (1999) [tmdbid-603]".into(),
                    "The Matrix".into(),
                    Some(1999),
                    Some(603)
                ),
            ]
        );
    }

    #[tokio::test]
    async fn adopts_episodes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            root,
            "Severance (2022)/Season 01/Severance - S01E01 - Good News About Hell.mkv",
        );
        touch(root, "Severance (2022)/Season 01/Severance - S01E02E03.mkv");
        touch(root, "Frieren/[Group] Frieren - 05 [1080p].mkv");
        touch(root, "Loose S01E01.mkv");
        let (pool, library) = setup(LibraryKind::Shows, root).await;

        let report = scan(&pool, &library).await.unwrap();
        assert_eq!(
            report,
            ScanReport {
                files: 3,
                added: 3,
                unparsed: 1,
                skipped: 1,
                ..ScanReport::default()
            }
        );

        // (series title, file, season, episode, episode_end)
        type EpisodeRow = (String, String, Option<i64>, Option<i64>, Option<i64>);
        let files: Vec<EpisodeRow> = sqlx::query_as(
            "SELECT s.parsed_title, f.rel_path, f.season, f.episode, f.episode_end
             FROM media_files f JOIN series s ON s.id = f.series_id ORDER BY f.rel_path",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            files,
            vec![
                (
                    "Frieren".into(),
                    "Frieren/[Group] Frieren - 05 [1080p].mkv".into(),
                    None,
                    None,
                    None
                ),
                (
                    "Severance".into(),
                    "Severance (2022)/Season 01/Severance - S01E01 - Good News About Hell.mkv"
                        .into(),
                    Some(1),
                    Some(1),
                    None
                ),
                (
                    "Severance".into(),
                    "Severance (2022)/Season 01/Severance - S01E02E03.mkv".into(),
                    Some(1),
                    Some(2),
                    Some(3)
                ),
            ]
        );
    }

    #[tokio::test]
    async fn rescan_marks_missing_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "Keep (2001)/Keep.mkv");
        touch(root, "Gone (2002)/Gone.mkv");
        let (pool, library) = setup(LibraryKind::Movies, root).await;
        scan(&pool, &library).await.unwrap();

        let again = scan(&pool, &library).await.unwrap();
        assert_eq!((again.files, again.added, again.newly_missing), (2, 0, 0));

        fs::remove_file(root.join("Gone (2002)/Gone.mkv")).unwrap();
        let after_delete = scan(&pool, &library).await.unwrap();
        assert_eq!((after_delete.files, after_delete.newly_missing), (1, 1));
        let rows: Vec<(String, Option<i64>)> =
            sqlx::query_as("SELECT rel_path, missing_since FROM media_files ORDER BY rel_path")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2, "rows are kept");
        assert!(rows[0].1.is_some(), "Gone is marked missing");
        assert!(rows[1].1.is_none(), "Keep is not");
        let movie_missing: Option<i64> =
            sqlx::query_scalar("SELECT missing_since FROM movies WHERE rel_path = 'Gone (2002)'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(movie_missing.is_some());

        touch(root, "Gone (2002)/Gone.mkv");
        let restored = scan(&pool, &library).await.unwrap();
        assert_eq!((restored.files, restored.added), (2, 0));
        let still_missing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM media_files WHERE missing_since IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(still_missing, 0);
    }

    #[tokio::test]
    async fn empty_root_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "Movie (2001)/Movie.mkv");
        let (pool, library) = setup(LibraryKind::Movies, root).await;
        scan(&pool, &library).await.unwrap();

        fs::remove_dir_all(root.join("Movie (2001)")).unwrap();
        let err = scan(&pool, &library).await.unwrap_err();
        assert!(format!("{err:#}").contains("is the share mounted"));
        let missing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM media_files WHERE missing_since IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(missing, 0);
    }

    #[tokio::test]
    async fn links_to_files_outside_the_library_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "Real (2001)/real.mkv");
        touch(outside.path(), "elsewhere.mkv");
        std::os::unix::fs::symlink(
            root.join("Real (2001)/real.mkv"),
            root.join("Real (2001)/inside.mkv"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("elsewhere.mkv"),
            root.join("Real (2001)/outside.mkv"),
        )
        .unwrap();
        let (pool, library) = setup(LibraryKind::Movies, root).await;

        let report = scan(&pool, &library).await.unwrap();
        assert_eq!((report.files, report.skipped), (2, 1));
        let paths: Vec<String> =
            sqlx::query_scalar("SELECT rel_path FROM media_files ORDER BY rel_path")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(paths, ["Real (2001)/inside.mkv", "Real (2001)/real.mkv"]);
    }

    // Relies on file permissions, so it fails if the tests run as root.
    #[tokio::test]
    async fn unreadable_folder_changes_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(root, "Open (2001)/Open.mkv");
        touch(root, "Locked (2002)/Locked.mkv");
        let (pool, library) = setup(LibraryKind::Movies, root).await;
        scan(&pool, &library).await.unwrap();

        let locked = root.join("Locked (2002)");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let result = scan(&pool, &library).await;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("Locked (2002)"), "{err:#}");
        let missing: i64 =
            sqlx::query_scalar("SELECT count(*) FROM media_files WHERE missing_since IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(missing, 0);
    }

    #[tokio::test]
    async fn missing_root_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (pool, library) = setup(LibraryKind::Movies, &dir.path().join("absent")).await;
        assert!(scan(&pool, &library).await.is_err());
    }

    #[test]
    fn a_music_walk_takes_dolby_and_matroska_audio_too() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.ec3", "b.thd", "c.mka", "d.flac", "e.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let (found, _) = walk(dir.path(), Media::Audio).unwrap();
        let mut names: Vec<&str> = found.iter().map(|f| f.rel_path.as_str()).collect();
        names.sort();
        assert_eq!(names, ["a.ec3", "b.thd", "c.mka", "d.flac"]);
        assert!(
            !AUDIO_EXTENSIONS.contains(&"ec3"),
            "downloads and searches don't count them as tracks"
        );
    }
}
