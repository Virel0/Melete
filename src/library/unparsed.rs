//! Naming the files a scan couldn't make sense of: episode files whose name gives no season
//! and episode, and music files with no tags to read. `scan finished` only counts them; these
//! lines say which they are, so they can be renamed or tagged, or the naming reported.
//!
//! Each is named once per run of Melete, by the first scan that finds it after Melete starts,
//! not by every scan. At most [`AT_MOST`] are named a scan; the rest are counted, and named by
//! the scans after.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

use sqlx::SqlitePool;

use super::{Library, LibraryKind};

/// Files named by one scan at most.
const AT_MOST: usize = 50;

/// The files already named in this run, by library id and path.
type Named = HashSet<(i64, String)>;

static NAMED: LazyLock<Mutex<Named>> = LazyLock::new(Default::default);

/// Logs each of the library's unparsed files not named yet in this run, at most [`AT_MOST`],
/// then how many more there are.
pub async fn name_new(pool: &SqlitePool, library: &Library) {
    match unparsed(pool, library.id).await {
        Ok(paths) => {
            let mut named = NAMED.lock().unwrap_or_else(PoisonError::into_inner);
            log(library, pick(&mut named, library.id, paths));
        }
        Err(err) => tracing::warn!(
            library = library.name,
            error = %err,
            "couldn't list the files a scan couldn't parse"
        ),
    }
}

/// The library's present files that couldn't be parsed, by path: the same files the libraries'
/// `unparsed_files` and the health page count.
async fn unparsed(pool: &SqlitePool, library_id: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT rel_path FROM media_files
         WHERE library_id = ?1 AND missing_since IS NULL
           AND series_id IS NOT NULL AND season IS NULL
         UNION ALL
         SELECT rel_path FROM music_tracks
         WHERE library_id = ?1 AND missing_since IS NULL AND tagged = 0
         ORDER BY rel_path",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await
}

/// Of `paths`, in order, the ones not named yet, at most [`AT_MOST`], noted as named now; and
/// how many more weren't named yet.
fn pick(named: &mut Named, library_id: i64, paths: Vec<String>) -> (Vec<String>, usize) {
    let new: Vec<String> = paths
        .into_iter()
        .filter(|path| !named.contains(&(library_id, path.clone())))
        .collect();
    let more = new.len().saturating_sub(AT_MOST);
    let now: Vec<String> = new.into_iter().take(AT_MOST).collect();
    named.extend(now.iter().map(|path| (library_id, path.clone())));
    (now, more)
}

fn log(library: &Library, (paths, more): (Vec<String>, usize)) {
    let why = match library.kind {
        LibraryKind::Music => "no tags to read in this file; it's named from its folders",
        _ => "no season and episode in this file's name",
    };
    for path in &paths {
        tracing::info!(library = library.name, path, "{why}");
    }
    if more > 0 {
        tracing::info!(
            library = library.name,
            more,
            "and {more} more files like these, named by the next scans"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// A library of `kind` over a folder holding `files`, scanned.
    async fn scanned(kind: LibraryKind, files: &[&str]) -> (SqlitePool, Library) {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"not a real file").unwrap();
        }
        let pool = crate::db::open_in_memory().await;
        let config = super::super::LibraryConfig {
            name: "test".into(),
            kind,
            path: dir.path().to_path_buf(),
        };
        let library = super::super::sync_config(&pool, &[config])
            .await
            .unwrap()
            .remove(0);
        let report = super::super::scan(&pool, &library).await.unwrap();
        assert_eq!(report.unparsed, 1);
        (pool, library)
    }

    #[tokio::test]
    async fn lists_the_episodes_without_a_season_and_episode_and_the_untagged_tracks() {
        let (pool, shows) = scanned(
            LibraryKind::Shows,
            &[
                "Severance (2022)/Season 01/Severance - S01E01.mkv",
                "Frieren/[Group] Frieren - 05 [1080p].mkv",
            ],
        )
        .await;
        assert_eq!(
            unparsed(&pool, shows.id).await.unwrap(),
            ["Frieren/[Group] Frieren - 05 [1080p].mkv"]
        );
        let (pool, music) = scanned(LibraryKind::Music, &["Artist/Album/01.flac"]).await;
        assert_eq!(
            unparsed(&pool, music.id).await.unwrap(),
            ["Artist/Album/01.flac"]
        );
    }

    #[test]
    fn names_each_file_once() {
        let mut named = Named::new();
        let first = pick(&mut named, 1, paths(&["a.mkv", "b.mkv"]));
        assert_eq!(first, (paths(&["a.mkv", "b.mkv"]), 0));
        assert_eq!(
            pick(&mut named, 1, paths(&["a.mkv", "b.mkv"])),
            (vec![], 0),
            "not again on the next scan"
        );
        assert_eq!(
            pick(&mut named, 1, paths(&["a.mkv", "b.mkv", "c.mkv"])),
            (paths(&["c.mkv"]), 0),
            "only the new one"
        );
        assert_eq!(
            pick(&mut named, 2, paths(&["a.mkv"])),
            (paths(&["a.mkv"]), 0),
            "another library's file of the same name is another file"
        );
    }

    #[test]
    fn a_large_batch_names_the_first_files_and_counts_the_rest() {
        let mut named = Named::new();
        let all: Vec<String> = (0..120).map(|n| format!("{n:03}.mkv")).collect();
        let (now, more) = pick(&mut named, 1, all.clone());
        assert_eq!((now.len(), more), (AT_MOST, 70));
        assert_eq!(now[0], "000.mkv");
        let (next, more) = pick(&mut named, 1, all.clone());
        assert_eq!(
            (next.first().map(String::as_str), more),
            (Some("050.mkv"), 20)
        );
        let (last, more) = pick(&mut named, 1, all.clone());
        assert_eq!((last.len(), more), (20, 0));
        assert_eq!(pick(&mut named, 1, all), (vec![], 0));
    }
}
