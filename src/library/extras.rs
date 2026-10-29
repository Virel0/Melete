//! Extras: trailers, featurettes and other bonus videos in a title's folder. Scans keep them
//! apart from the title's own videos, in a table of their own (`media_extras`), so they never
//! count as the movie or as episodes; they're listed with the title and play like any file.
//! Release samples are neither, and are left out.

use std::collections::HashMap;

use sqlx::SqliteConnection;

use super::scan::FoundFile;
use super::{Library, LibraryKind};

/// Extras' ids start here, so an extra's id is never a library file's: both play through the
/// same routes, which tell them apart by it.
pub const ID_BASE: i64 = 1 << 40;

/// Whether a playable id is an extra's rather than a library file's.
pub fn is_extra_id(id: i64) -> bool {
    id >= ID_BASE
}

/// Folders inside a title folder that hold extras rather than the movie or episodes, and the
/// kind each holds; `None` for release samples, which are left out altogether.
const FOLDERS: &[(&str, Option<&str>)] = &[
    ("extras", Some("other")),
    ("featurettes", Some("featurette")),
    ("behind the scenes", Some("behind_the_scenes")),
    ("deleted scenes", Some("deleted_scene")),
    ("interviews", Some("interview")),
    ("scenes", Some("scene")),
    ("shorts", Some("short")),
    ("trailers", Some("trailer")),
    ("clips", Some("clip")),
    ("other", Some("other")),
    ("sample", None),
    ("samples", None),
];

/// File-name endings that mark an extra, as in `Movie-trailer.mkv`, and its kind.
const SUFFIXES: &[(&str, Option<&str>)] = &[
    ("-trailer", Some("trailer")),
    ("-sample", None),
    ("-featurette", Some("featurette")),
    ("-behindthescenes", Some("behind_the_scenes")),
    ("-deleted", Some("deleted_scene")),
    ("-interview", Some("interview")),
    ("-scene", Some("scene")),
    ("-short", Some("short")),
    ("-clip", Some("clip")),
    ("-other", Some("other")),
    ("-extra", Some("other")),
];

/// Whether a folder below a title folder holds extras (or samples) rather than the title's
/// own videos.
pub(super) fn is_extras_folder(name: &str) -> bool {
    let lower = name.to_lowercase();
    FOLDERS.iter().any(|(folder, _)| *folder == lower)
}

/// Whether a video's name marks it as an extra or a sample (`Movie-trailer.mkv`, `sample.mkv`).
pub(super) fn has_extra_name(name: &str) -> bool {
    let stem = stem(name).to_lowercase();
    stem == "sample" || SUFFIXES.iter().any(|(suffix, _)| stem.ends_with(suffix))
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// An extra's kind and title, from its path in the library: the first extras folder below the
/// title folder decides the kind, else the end of its name. `None` for a sample, and for a
/// video that is no extra.
pub(super) fn classify(rel_path: &str) -> Option<(&'static str, String)> {
    let mut parts: Vec<&str> = rel_path.split('/').collect();
    let file = parts.pop()?;
    let stem = stem(file);
    let lower = stem.to_lowercase();
    let in_folder = parts.iter().skip(1).find_map(|part| {
        let part = part.to_lowercase();
        FOLDERS
            .iter()
            .find(|(folder, _)| *folder == part)
            .map(|(_, kind)| *kind)
    });
    let (kind, title) = match in_folder {
        Some(kind) => (kind?, stem.to_owned()),
        None if lower == "sample" => return None,
        None => {
            let (suffix, kind) = SUFFIXES.iter().find(|(s, _)| lower.ends_with(s))?;
            let cut = stem
                .len()
                .checked_sub(suffix.len())
                .and_then(|n| stem.get(..n));
            (kind.as_ref().copied()?, cut.unwrap_or(stem).to_owned())
        }
    };
    let title = title.replace(['_', '.'], " ").trim().to_owned();
    let title = if title.is_empty() {
        kind.replace('_', " ")
    } else {
        title
    };
    Some((kind, title))
}

/// Records a scan's extras under their titles (by title folder), and marks the extras it
/// didn't see as missing since `now`. An extra in a folder with no video of its own isn't
/// recorded: there's no title for it.
pub(super) async fn record(
    tx: &mut SqliteConnection,
    library: &Library,
    titles: &HashMap<&str, i64>,
    extras: &[FoundFile],
    (generation, now, scope): (i64, i64, Option<&str>),
) -> anyhow::Result<()> {
    let owner = match library.kind {
        LibraryKind::Shows => "series_id",
        _ => "movie_id",
    };
    for file in extras {
        let Some((kind, title)) = classify(&file.rel_path) else {
            continue;
        };
        let folder = file.rel_path.split('/').next().unwrap_or_default();
        let Some(title_id) = titles.get(folder) else {
            continue;
        };
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, {owner}, kind,
                 title, first_seen_generation, seen_generation)
             VALUES ((SELECT max(?1, coalesce(max(id) + 1, ?1)) FROM media_extras),
                 ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT (library_id, rel_path) DO UPDATE SET
                 size = excluded.size, modified_at = excluded.modified_at,
                 movie_id = excluded.movie_id, series_id = excluded.series_id,
                 kind = excluded.kind, title = excluded.title,
                 seen_generation = excluded.seen_generation, missing_since = NULL"
        )))
        .bind(ID_BASE)
        .bind(library.id)
        .bind(&file.rel_path)
        .bind(file.size)
        .bind(file.modified)
        .bind(title_id)
        .bind(kind)
        .bind(&title)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE media_extras SET missing_since = ?1
         WHERE library_id = ?2 AND seen_generation < ?3 AND missing_since IS NULL AND {}",
        super::walk::in_scope("rel_path", 4)
    )))
    .bind(now)
    .bind(library.id)
    .bind(generation)
    .bind(scope)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extras_are_known_by_their_folder_or_name_and_samples_by_neither() {
        for (path, expected) in [
            ("Film/Trailers/Teaser.mkv", Some(("trailer", "Teaser"))),
            (
                "Film/Behind The Scenes/Making_of.mp4",
                Some(("behind_the_scenes", "Making of")),
            ),
            ("Film/Extras/Deleted Scenes/Cut.mkv", Some(("other", "Cut"))),
            ("Film/Film-trailer.mkv", Some(("trailer", "Film"))),
            ("Film/-featurette.mkv", Some(("featurette", "featurette"))),
            (
                "Show/Season 1/Extras/Gag Reel.mkv",
                Some(("other", "Gag Reel")),
            ),
            ("Film/Sample/film.mkv", None),
            ("Film/film-sample.mkv", None),
            ("Film/sample.mkv", None),
            ("Film/Film.mkv", None),
            ("Extras/Extras - S01E01.mkv", None),
        ] {
            let found = classify(path);
            assert_eq!(
                found.as_ref().map(|(k, t)| (*k, t.as_str())),
                expected,
                "{path}"
            );
        }
        assert!(is_extras_folder("Featurettes") && is_extras_folder("samples"));
        assert!(has_extra_name("Film-Trailer.mkv") && !has_extra_name("Trailer Park.mkv"));
        assert!(is_extra_id(ID_BASE) && !is_extra_id(ID_BASE - 1));
    }

    async fn scanned(kind: LibraryKind, root: &std::path::Path) -> (sqlx::SqlitePool, Library) {
        let pool = crate::db::open_in_memory().await;
        let config = crate::library::LibraryConfig {
            name: "l".into(),
            kind,
            path: root.into(),
        };
        let library = crate::library::sync_config(&pool, &[config])
            .await
            .unwrap()
            .remove(0);
        crate::library::scan(&pool, &library).await.unwrap();
        (pool, library)
    }

    fn touch(root: &std::path::Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"video").unwrap();
    }

    #[tokio::test]
    async fn scans_keep_extras_apart_from_the_titles_videos() {
        let dir = tempfile::tempdir().unwrap();
        for rel in [
            "Film (2000)/Film.mkv",
            "Film (2000)/Film-trailer.mkv",
            "Film (2000)/Featurettes/Making Of.mkv",
            "Film (2000)/Sample/sample.mkv",
            "Only Extras/Trailers/Teaser.mkv",
        ] {
            touch(dir.path(), rel);
        }
        let (pool, _) = scanned(LibraryKind::Movies, dir.path()).await;
        let files: Vec<String> = sqlx::query_scalar("SELECT rel_path FROM media_files")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            files,
            ["Film (2000)/Film.mkv"],
            "extras are never the movie"
        );
        let extras: Vec<(i64, String, String, i64)> =
            sqlx::query_as("SELECT id, kind, title, movie_id FROM media_extras ORDER BY rel_path")
                .fetch_all(&pool)
                .await
                .unwrap();
        let found: Vec<(&str, &str, i64)> = extras
            .iter()
            .map(|(id, kind, title, movie)| {
                assert!(is_extra_id(*id));
                (kind.as_str(), title.as_str(), *movie)
            })
            .collect();
        assert_eq!(
            found,
            [("featurette", "Making Of", 1), ("trailer", "Film", 1)],
            "no sample, and nothing for a folder with no video of its own"
        );
        // Probed alongside the files (these aren't real videos, so ffprobe says so).
        crate::probe::probe_pending(&pool, std::path::Path::new("ffprobe"))
            .await
            .unwrap();
        let probed: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM media_extras WHERE probed_size = size AND probe_error IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(probed, 2);
    }

    #[tokio::test]
    async fn a_vanished_extra_is_marked_missing_and_a_show_named_extras_is_a_show() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Extras/Season 1/Extras - S01E01.mkv");
        touch(dir.path(), "Extras/Season 1/Behind The Scenes/Set.mkv");
        let (pool, library) = scanned(LibraryKind::Shows, dir.path()).await;
        let episodes: i64 =
            sqlx::query_scalar("SELECT count(*) FROM media_files WHERE episode = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(episodes, 1);
        std::fs::remove_dir_all(dir.path().join("Extras/Season 1/Behind The Scenes")).unwrap();
        crate::library::scan(&pool, &library).await.unwrap();
        let extra: (String, Option<i64>, i64) =
            sqlx::query_as("SELECT kind, missing_since, series_id FROM media_extras")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(extra.0, "behind_the_scenes");
        assert!(extra.1.is_some(), "kept, marked missing, never deleted");
    }
}
