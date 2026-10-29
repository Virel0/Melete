use super::*;
use crate::db;

struct Library {
    pool: SqlitePool,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

async fn library() -> Library {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    std::fs::create_dir(&root).unwrap();
    let pool = db::open_in_memory().await;
    sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Dune (2024)', 'Dune', 1);
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (2, 1, 'Show', 'Show', 1);
             INSERT INTO wanted_movies (id, tmdb_id, title, year, monitored, source, added_at, movie_id)
             VALUES (1, 693134, 'Dune', 2024, 1, 'radarr', 0, 1);
             INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, source, added_at, series_id)
             VALUES (1, 1, 'Show', 1, 'standard', 'sonarr', 0, 2)",
        )
        .bind(root.to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
    Library {
        pool,
        root,
        _dir: dir,
    }
}

/// Writes a file and records it as scanned, with a probe of `minutes` (none for `None`).
async fn file(
    lib: &Library,
    rel_path: &str,
    episodes: Option<(i64, i64, Option<i64>)>,
    minutes: Option<f64>,
) -> i64 {
    let path = lib.root.join(rel_path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, rel_path.as_bytes()).unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    let modified = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (movie, series) = if episodes.is_some() {
        (None, Some(2))
    } else {
        (Some(1), None)
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id, series_id,
                 season, episode, episode_end, first_seen_generation, seen_generation)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, 1, 1) RETURNING id",
    )
    .bind(rel_path)
    .bind(meta.len() as i64)
    .bind(modified)
    .bind(movie)
    .bind(series)
    .bind(episodes.map(|e| e.0))
    .bind(episodes.map(|e| e.1))
    .bind(episodes.and_then(|e| e.2))
    .fetch_one(&lib.pool)
    .await
    .unwrap();
    if let Some(minutes) = minutes {
        sqlx::query(
            "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
                 VALUES (?, ?, ?, 0, json_object('duration_seconds', ?))",
        )
        .bind(id)
        .bind(meta.len() as i64)
        .bind(modified)
        .bind(minutes * 60.0)
        .execute(&lib.pool)
        .await
        .unwrap();
    }
    id
}

/// An imported download for the movie that replaces `replaces` and placed `placed`.
async fn imported(lib: &Library, replaces: &[Replaced], placed: &[&str], age: i64) -> i64 {
    let placed: Vec<String> = placed
        .iter()
        .map(|p| lib.root.join(p).to_string_lossy().into_owned())
        .collect();
    sqlx::query_scalar(
            "INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, added_at,
                 updated_at, replaces, placed)
             VALUES (lower(hex(randomblob(20))), 'x', 1, '{}', 'imported', 0, ?, ?, ?) RETURNING id",
        )
        .bind(unix_now() - age)
        .bind(serde_json::to_string(replaces).unwrap())
        .bind(serde_json::to_string(&placed).unwrap())
        .fetch_one(&lib.pool)
        .await
        .unwrap()
}

async fn message(lib: &Library, id: i64) -> (Option<i64>, Option<String>) {
    sqlx::query_as("SELECT retired_at, message FROM downloads WHERE id = ?")
        .bind(id)
        .fetch_one(&lib.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn knows_exactly_what_a_release_replaces_or_nothing() {
    let lib = library().await;
    let pool = &lib.pool;
    file(
        &lib,
        "Dune (2024)/Dune.2024.1080p.WEB-DL.mkv",
        None,
        Some(166.0),
    )
    .await;
    let movie = replaced_by(pool, Some(1), None, None, "Dune.2024.2160p.REMUX")
        .await
        .unwrap();
    assert_eq!(movie.len(), 1);
    assert_eq!(movie[0].rel_path, "Dune (2024)/Dune.2024.1080p.WEB-DL.mkv");
    // Two files for one movie: which goes is unclear.
    file(&lib, "Dune (2024)/Dune.2024.720p.mkv", None, None).await;
    assert!(
        replaced_by(pool, Some(1), None, None, "x")
            .await
            .unwrap()
            .is_empty()
    );

    file(&lib, "Show/Show.S01E05.mkv", Some((1, 5, None)), None).await;
    file(&lib, "Show/Show.S01E06.mkv", Some((1, 6, None)), None).await;
    file(&lib, "Show/Show.S01E07E08.mkv", Some((1, 7, Some(8))), None).await;
    let names = |r: Vec<Replaced>| r.into_iter().map(|r| r.rel_path).collect::<Vec<_>>();
    let replaces = |title: &'static str| async move {
        names(
            replaced_by(pool, None, Some(1), Some(1), title)
                .await
                .unwrap(),
        )
    };
    assert_eq!(
        replaces("Show.S01E05.2160p.WEB").await,
        ["Show/Show.S01E05.mkv"]
    );
    assert_eq!(
        replaces("Show.S01E05E06.2160p.WEB").await,
        ["Show/Show.S01E05.mkv", "Show/Show.S01E06.mkv"]
    );
    assert!(
        replaces("Show.S01E07.2160p.WEB").await.is_empty(),
        "E07E08 reaches past it"
    );
    assert!(
        replaces("Show.S01.2160p.WEB").await.is_empty(),
        "season packs replace nothing"
    );
    assert!(
        replaces("Show.S02E05.2160p.WEB").await.is_empty(),
        "another season"
    );
    assert!(
        replaced_by(pool, None, None, None, "x")
            .await
            .unwrap()
            .is_empty(),
        "nothing wanted"
    );
}

#[tokio::test]
async fn trashes_what_a_confirmed_upgrade_replaced() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.2024.1080p.WEB-DL.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let new_path = "Dune (2024)/Dune.2024.2160p.REMUX.mkv";
    let download = imported(&lib, &replaces, &[new_path], 0).await;

    // Not scanned yet: it waits.
    let report = retire(&lib.pool).await.unwrap();
    assert_eq!(
        report,
        RetireReport {
            waiting: 1,
            ..Default::default()
        }
    );
    assert!(lib.root.join(old_path).exists());

    file(&lib, new_path, None, Some(166.5)).await;
    let report = retire(&lib.pool).await.unwrap();
    assert_eq!(
        report,
        RetireReport {
            trashed: 1,
            ..Default::default()
        }
    );
    let trashed = lib
        .root
        .join(TRASH)
        .join(download.to_string())
        .join(old_path);
    assert!(!lib.root.join(old_path).exists() && trashed.exists());
    assert!(lib.root.join(new_path).exists());
    let (retired, message) = message(&lib, download).await;
    assert!(retired.is_some());
    assert_eq!(
        message.as_deref(),
        Some("in the library; the file it replaced in the trash for 30 days")
    );
    let missing: Option<i64> =
        sqlx::query_scalar("SELECT missing_since FROM media_files WHERE rel_path = ?")
            .bind(old_path)
            .fetch_one(&lib.pool)
            .await
            .unwrap();
    assert!(missing.is_some(), "gone from the library at once");
    let listed = list(&lib.pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (listed[0].rel_path.as_str(), listed[0].replaced_by.as_str()),
        (old_path, new_path)
    );
    assert_eq!(listed[0].delete_after, listed[0].trashed_at + KEEP_SECONDS);
    // Done once.
    assert_eq!(retire(&lib.pool).await.unwrap(), RetireReport::default());

    // Put back, then nothing can be put back twice.
    assert_eq!(restore(&lib.pool, listed[0].id).await.unwrap(), Ok(()));
    assert!(lib.root.join(old_path).exists() && !trashed.exists());
    assert_eq!(
        restore(&lib.pool, listed[0].id).await.unwrap(),
        Err(NotRestored::NotInTrash)
    );
    assert!(list(&lib.pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn anything_unclear_keeps_the_old_file() {
    let lib = library().await;
    // Another cut: 166 minutes against 190.
    file(&lib, "Dune (2024)/Dune.mkv", None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let other_cut = imported(&lib, &replaces, &["Dune (2024)/Dune.Extended.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.Extended.mkv", None, Some(190.0)).await;
    assert_eq!(retire(&lib.pool).await.unwrap().kept, 1);
    assert!(lib.root.join("Dune (2024)/Dune.mkv").exists());
    let (retired, why) = message(&lib, other_cut).await;
    assert!(retired.is_some());
    assert!(why.unwrap().contains("may be another cut"));

    // Changed on disk since the scan.
    let show = |rel: &str| lib.root.join(rel);
    file(&lib, "Show/Show.S01E05.mkv", Some((1, 5, None)), Some(24.0)).await;
    let replaces = replaced_by(&lib.pool, None, Some(1), Some(1), "Show.S01E05.WEB")
        .await
        .unwrap();
    file(
        &lib,
        "Show/Show.S01E05.2160p.mkv",
        Some((1, 5, None)),
        Some(24.0),
    )
    .await;
    std::fs::write(
        show("Show/Show.S01E05.mkv"),
        b"rewritten, longer than before",
    )
    .unwrap();
    let changed = imported(&lib, &replaces, &["Show/Show.S01E05.2160p.mkv"], 0).await;
    retire(&lib.pool).await.unwrap();
    assert!(show("Show/Show.S01E05.mkv").exists());
    assert!(
        message(&lib, changed)
            .await
            .1
            .unwrap()
            .contains("isn't the file that was scanned")
    );

    // A link where the file was is never followed or moved.
    file(&lib, "Show/Show.S01E06.mkv", Some((1, 6, None)), Some(24.0)).await;
    let replaces = replaced_by(&lib.pool, None, Some(1), Some(1), "Show.S01E06.WEB")
        .await
        .unwrap();
    file(
        &lib,
        "Show/Show.S01E06.2160p.mkv",
        Some((1, 6, None)),
        Some(24.0),
    )
    .await;
    let real = lib.root.join("elsewhere.mkv");
    std::fs::rename(show("Show/Show.S01E06.mkv"), &real).unwrap();
    std::os::unix::fs::symlink(&real, show("Show/Show.S01E06.mkv")).unwrap();
    let linked = imported(&lib, &replaces, &["Show/Show.S01E06.2160p.mkv"], 0).await;
    retire(&lib.pool).await.unwrap();
    assert!(real.exists() && show("Show/Show.S01E06.mkv").is_symlink());
    assert!(message(&lib, linked).await.1.unwrap().starts_with("kept "));

    // Never scanned in a week: given up, the old file kept.
    file(&lib, "Show/Show.S01E07.mkv", Some((1, 7, None)), Some(24.0)).await;
    let replaces = replaced_by(&lib.pool, None, Some(1), Some(1), "Show.S01E07.WEB")
        .await
        .unwrap();
    let stale = imported(
        &lib,
        &replaces,
        &["Show/Show.S01E07.2160p.mkv"],
        WAIT_SECONDS + 1,
    )
    .await;
    retire(&lib.pool).await.unwrap();
    assert!(show("Show/Show.S01E07.mkv").exists());
    assert_eq!(
        message(&lib, stale).await.1.as_deref(),
        Some("kept the old file: the new one hasn't shown up in a scan")
    );
    assert!(
        list(&lib.pool).await.unwrap().is_empty(),
        "nothing in the trash"
    );
    assert!(
        !lib.root
            .join(TRASH)
            .join(linked.to_string())
            .join("Show/Show.S01E06.mkv")
            .exists()
    );
}

#[tokio::test]
async fn empties_the_trash_after_30_days_and_only_the_trash() {
    let lib = library().await;
    file(&lib, "Dune (2024)/Dune.mkv", None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let download = imported(&lib, &replaces, &["Dune (2024)/Dune.2160p.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.2160p.mkv", None, Some(166.0)).await;
    retire(&lib.pool).await.unwrap();
    assert_eq!(purge(&lib.pool).await.unwrap(), 0, "not yet");
    sqlx::query("UPDATE trashed_files SET trashed_at = trashed_at - ?")
        .bind(KEEP_SECONDS + 1)
        .execute(&lib.pool)
        .await
        .unwrap();
    // A row pointing out of the trash is never acted on.
    sqlx::query(
            "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, replaced_by, trashed_at)
             VALUES (1, 'Dune (2024)/Dune.2160p.mkv', 'Dune (2024)/Dune.2160p.mkv', 1, 'x', 0)",
        )
        .execute(&lib.pool)
        .await
        .unwrap();
    assert_eq!(purge(&lib.pool).await.unwrap(), 1);
    let trash = lib.root.join(TRASH);
    assert!(
        !trash.join(download.to_string()).exists(),
        "its folders went with it"
    );
    assert!(lib.root.join("Dune (2024)/Dune.2160p.mkv").exists());
    assert!(
        list(&lib.pool).await.unwrap().len() == 1,
        "the odd row stays listed, untouched"
    );
}

#[tokio::test]
async fn deletes_from_the_trash_now_and_only_from_the_trash() {
    let lib = library().await;
    file(&lib, "Dune (2024)/Dune.mkv", None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let download = imported(&lib, &replaces, &["Dune (2024)/Dune.2160p.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.2160p.mkv", None, Some(166.0)).await;
    retire(&lib.pool).await.unwrap();
    let trashed = list(&lib.pool).await.unwrap();
    assert_eq!(trashed.len(), 1);
    // A row pointing out of the trash, at the upgrade itself, is never acted on.
    sqlx::query(
            "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, replaced_by, trashed_at)
             VALUES (1, 'Dune (2024)/Dune.2160p.mkv', 'Dune (2024)/Dune.2160p.mkv', 1, 'x', 0)",
        )
        .execute(&lib.pool)
        .await
        .unwrap();
    assert_eq!(delete_now(&lib.pool, Some(999)).await.unwrap(), None);
    // One, by its id, well before its 30 days.
    assert_eq!(
        delete_now(&lib.pool, Some(trashed[0].id)).await.unwrap(),
        Some(Emptied {
            deleted: 1,
            left: 0
        })
    );
    assert!(!lib.root.join(TRASH).join(download.to_string()).exists());
    assert!(lib.root.join("Dune (2024)/Dune.2160p.mkv").exists());
    // Everything: the odd row is left, and so is the file it names.
    assert_eq!(
        delete_now(&lib.pool, None).await.unwrap(),
        Some(Emptied {
            deleted: 0,
            left: 1
        })
    );
    assert!(lib.root.join("Dune (2024)/Dune.2160p.mkv").exists());
    assert_eq!(
        list(&lib.pool).await.unwrap().len(),
        1,
        "the odd row stays listed"
    );
}

#[tokio::test]
async fn emptying_the_trash_leaves_a_file_that_changed_since() {
    let lib = library().await;
    file(&lib, "Dune (2024)/Dune.mkv", None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    imported(&lib, &replaces, &["Dune (2024)/Dune.2160p.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.2160p.mkv", None, Some(166.0)).await;
    retire(&lib.pool).await.unwrap();
    let trashed = list(&lib.pool).await.unwrap();
    let path = lib.root.join(&trashed[0].trash_path);
    std::fs::write(&path, b"something else now").unwrap();
    assert_eq!(
        delete_now(&lib.pool, None).await.unwrap(),
        Some(Emptied {
            deleted: 0,
            left: 1
        })
    );
    assert!(
        path.exists(),
        "not the file the trash put there, so it stays"
    );
}

#[tokio::test]
async fn a_loose_movie_file_or_a_second_copy_of_an_episode_stays() {
    let lib = library().await;
    // A movie kept as a loose file: its upgrade becomes a title of its own beside it.
    sqlx::query(
            "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (3, 1, 'Loose.2024.mkv', 'Loose', 1), (4, 1, 'Loose.2024.2160p.mkv', 'Loose', 1)",
        )
        .execute(&lib.pool)
        .await
        .unwrap();
    file(&lib, "Loose.2024.mkv", None, Some(100.0)).await;
    file(&lib, "Loose.2024.2160p.mkv", None, Some(100.0)).await;
    sqlx::query(
        "UPDATE media_files SET movie_id = 3 WHERE rel_path = 'Loose.2024.mkv';
                     UPDATE media_files SET movie_id = 4 WHERE rel_path = 'Loose.2024.2160p.mkv'",
    )
    .execute(&lib.pool)
    .await
    .unwrap();
    let replaces: Vec<Replaced> = sqlx::query_as(
            "SELECT id AS file_id, library_id, rel_path, size, modified_at, season, episode, episode_end
             FROM media_files WHERE rel_path = 'Loose.2024.mkv'",
        )
        .fetch_all(&lib.pool)
        .await
        .unwrap();
    let loose = imported(&lib, &replaces, &["Loose.2024.2160p.mkv"], 0).await;
    retire(&lib.pool).await.unwrap();
    assert!(lib.root.join("Loose.2024.mkv").exists());
    assert!(
        message(&lib, loose)
            .await
            .1
            .unwrap()
            .contains("same title's folder")
    );

    // Two files for one episode (a dub and the original): neither is replaced.
    file(&lib, "Show/Show - 05 [Dub].mkv", Some((1, 5, None)), None).await;
    file(&lib, "Show/Show - 05 [JP].mkv", Some((1, 5, None)), None).await;
    assert!(
        replaced_by(&lib.pool, None, Some(1), Some(1), "Show.S01E05.WEB")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_new_file_must_be_on_disk_as_scanned() {
    let lib = library().await;
    file(&lib, "Dune (2024)/Dune.mkv", None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let download = imported(&lib, &replaces, &["Dune (2024)/Dune.2160p.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.2160p.mkv", None, Some(166.0)).await;
    // Deleted by hand after the scan.
    std::fs::remove_file(lib.root.join("Dune (2024)/Dune.2160p.mkv")).unwrap();
    retire(&lib.pool).await.unwrap();
    assert!(lib.root.join("Dune (2024)/Dune.mkv").exists());
    assert!(
        message(&lib, download)
            .await
            .1
            .unwrap()
            .contains("isn't on disk as it was scanned")
    );
}

#[tokio::test]
async fn a_move_an_earlier_run_finished_is_recorded_not_kept() {
    let lib = library().await;
    let old = "Dune (2024)/Dune.mkv";
    file(&lib, old, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let download = imported(&lib, &replaces, &["Dune (2024)/Dune.2160p.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.2160p.mkv", None, Some(166.0)).await;
    let trashed = lib.root.join(TRASH).join(download.to_string()).join(old);
    std::fs::create_dir_all(trashed.parent().unwrap()).unwrap();
    // Stopped between the link and the unlink.
    std::fs::hard_link(lib.root.join(old), &trashed).unwrap();
    assert_eq!(retire(&lib.pool).await.unwrap().trashed, 1);
    assert!(!lib.root.join(old).exists() && trashed.exists());

    // Moved, then the write was lost and a scan marked the file missing.
    let other = "Show/Show.S01E05.mkv";
    file(&lib, other, Some((1, 5, None)), Some(24.0)).await;
    let replaces = replaced_by(&lib.pool, None, Some(1), Some(1), "Show.S01E05.WEB")
        .await
        .unwrap();
    let second = imported(&lib, &replaces, &["Show/Show.S01E05.2160p.mkv"], 0).await;
    file(
        &lib,
        "Show/Show.S01E05.2160p.mkv",
        Some((1, 5, None)),
        Some(24.0),
    )
    .await;
    let moved = lib.root.join(TRASH).join(second.to_string()).join(other);
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(lib.root.join(other), &moved).unwrap();
    sqlx::query("UPDATE media_files SET missing_since = 1 WHERE rel_path = ?")
        .bind(other)
        .execute(&lib.pool)
        .await
        .unwrap();
    assert_eq!(retire(&lib.pool).await.unwrap().trashed, 1);
    assert_eq!(
        list(&lib.pool).await.unwrap().len(),
        2,
        "both recorded, so both purge"
    );
    assert!(
        message(&lib, second)
            .await
            .1
            .unwrap()
            .starts_with("in the library")
    );
}

#[tokio::test]
async fn an_unmounted_share_isnt_taken_for_an_emptied_trash() {
    let lib = library().await;
    file(&lib, "Dune (2024)/Dune.mkv", None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    imported(&lib, &replaces, &["Dune (2024)/Dune.2160p.mkv"], 0).await;
    file(&lib, "Dune (2024)/Dune.2160p.mkv", None, Some(166.0)).await;
    retire(&lib.pool).await.unwrap();
    sqlx::query("UPDATE trashed_files SET trashed_at = 0")
        .execute(&lib.pool)
        .await
        .unwrap();
    // The share drops out: its trash folder is gone with it.
    let away = lib.root.with_file_name("unmounted");
    std::fs::rename(&lib.root, &away).unwrap();
    std::fs::create_dir(&lib.root).unwrap();
    assert_eq!(purge(&lib.pool).await.unwrap(), 0);
    assert_eq!(
        list(&lib.pool).await.unwrap().len(),
        1,
        "still waiting to be deleted"
    );
    // A trash entry that isn't what was put there isn't restored.
    std::fs::remove_dir(&lib.root).unwrap();
    std::fs::rename(&away, &lib.root).unwrap();
    let entry = &list(&lib.pool).await.unwrap()[0];
    std::fs::write(lib.root.join(&entry.trash_path), b"something else").unwrap();
    assert!(matches!(
        restore(&lib.pool, entry.id).await.unwrap(),
        Err(NotRestored::Failed(_))
    ));
    assert!(!lib.root.join("Dune (2024)/Dune.mkv").exists());
}

/// Every file below `dir`, by its path below it; none when it doesn't exist.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry.unwrap().path();
        if std::fs::symlink_metadata(&path).unwrap().is_dir() {
            found.extend(files_below(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[tokio::test]
async fn a_new_file_never_probed_keeps_the_old_one_after_a_week() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let new_path = "Dune (2024)/Dune.2160p.mkv";
    let download = imported(&lib, &replaces, &[new_path], WAIT_SECONDS + 1).await;
    // Scanned, but never probed.
    file(&lib, new_path, None, None).await;
    assert_eq!(
        retire(&lib.pool).await.unwrap(),
        RetireReport {
            kept: 1,
            ..Default::default()
        }
    );
    assert!(lib.root.join(old_path).exists());
    assert_eq!(
        message(&lib, download).await.1.as_deref(),
        Some("kept the old file: the new one hasn't been probed")
    );
    assert!(files_below(&lib.root.join(TRASH)).is_empty());
}

#[tokio::test]
async fn the_old_file_stays_when_ffprobe_cant_tell_both_lengths() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let new_path = "Dune (2024)/Dune.2160p.mkv";
    let download = imported(&lib, &replaces, &[new_path], 0).await;
    // Probed, but ffprobe failed on it.
    let new_id = file(&lib, new_path, None, None).await;
    sqlx::query(
        "INSERT INTO media_probes (file_id, size, modified_at, probed_at, error)
         SELECT id, size, modified_at, 0, 'ffprobe failed' FROM media_files WHERE id = ?",
    )
    .bind(new_id)
    .execute(&lib.pool)
    .await
    .unwrap();
    assert_eq!(
        retire(&lib.pool).await.unwrap(),
        RetireReport {
            kept: 1,
            ..Default::default()
        }
    );
    assert!(lib.root.join(old_path).exists());
    assert_eq!(
        message(&lib, download).await.1.as_deref(),
        Some("kept Dune (2024)/Dune.mkv: ffprobe couldn't tell both files' lengths")
    );
    assert!(files_below(&lib.root.join(TRASH)).is_empty());
}

#[tokio::test]
async fn a_new_file_that_is_the_old_file_keeps_it() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    // The import put the new file at the old one's name.
    let download = imported(&lib, &replaces, &[old_path], 0).await;
    assert_eq!(
        retire(&lib.pool).await.unwrap(),
        RetireReport {
            kept: 1,
            ..Default::default()
        }
    );
    assert!(lib.root.join(old_path).exists());
    assert_eq!(
        message(&lib, download).await.1.as_deref(),
        Some("kept Dune (2024)/Dune.mkv: the new file is this file")
    );
    assert!(files_below(&lib.root.join(TRASH)).is_empty());
}

#[tokio::test]
async fn a_replaced_file_whose_path_isnt_plain_is_kept() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let mut replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    // The same file, named through a way up and back down.
    let roundabout = "Dune (2024)/../Dune (2024)/Dune.mkv";
    replaces[0].rel_path = roundabout.into();
    let new_path = "Dune (2024)/Dune.2160p.mkv";
    let download = imported(&lib, &replaces, &[new_path], 0).await;
    file(&lib, new_path, None, Some(166.0)).await;
    assert_eq!(
        retire(&lib.pool).await.unwrap(),
        RetireReport {
            kept: 1,
            ..Default::default()
        }
    );
    assert!(lib.root.join(old_path).exists());
    assert_eq!(
        message(&lib, download).await.1.unwrap(),
        format!("kept {roundabout}: its path isn't a plain one inside the library")
    );
    assert!(files_below(&lib.root.join(TRASH)).is_empty());
}

#[tokio::test]
async fn a_file_changed_since_the_upgrade_was_grabbed_is_kept() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let new_path = "Dune (2024)/Dune.2160p.mkv";
    let download = imported(&lib, &replaces, &[new_path], 0).await;
    file(&lib, new_path, None, Some(166.0)).await;
    // Rewritten after the grab, and scanned again since.
    let rewritten = b"rewritten after the grab, and longer";
    std::fs::write(lib.root.join(old_path), rewritten).unwrap();
    sqlx::query("UPDATE media_files SET size = ? WHERE rel_path = ?")
        .bind(rewritten.len() as i64)
        .bind(old_path)
        .execute(&lib.pool)
        .await
        .unwrap();
    assert_eq!(
        retire(&lib.pool).await.unwrap(),
        RetireReport {
            kept: 1,
            ..Default::default()
        }
    );
    assert_eq!(std::fs::read(lib.root.join(old_path)).unwrap(), rewritten);
    assert_eq!(
        message(&lib, download).await.1.as_deref(),
        Some("kept Dune (2024)/Dune.mkv: it changed since the upgrade was grabbed")
    );
    assert!(files_below(&lib.root.join(TRASH)).is_empty());
}

#[tokio::test]
async fn a_move_that_cant_be_recorded_is_put_back() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.mkv";
    file(&lib, old_path, None, Some(166.0)).await;
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let new_path = "Dune (2024)/Dune.2160p.mkv";
    let download = imported(&lib, &replaces, &[new_path], 0).await;
    file(&lib, new_path, None, Some(166.0)).await;
    sqlx::query(
        "CREATE TRIGGER no_trash_records BEFORE INSERT ON trashed_files
         BEGIN SELECT RAISE(ABORT, 'recording refused for the test'); END",
    )
    .execute(&lib.pool)
    .await
    .unwrap();

    let err = retire(&lib.pool).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("recording refused for the test"),
        "{err:#}"
    );
    assert_eq!(
        std::fs::read(lib.root.join(old_path)).unwrap(),
        old_path.as_bytes(),
        "back where it was"
    );
    assert!(files_below(&lib.root.join(TRASH)).is_empty());
    assert!(list(&lib.pool).await.unwrap().is_empty());
    let missing: Option<i64> =
        sqlx::query_scalar("SELECT missing_since FROM media_files WHERE rel_path = ?")
            .bind(old_path)
            .fetch_one(&lib.pool)
            .await
            .unwrap();
    assert_eq!(missing, None, "still in the library");
    assert_eq!(message(&lib, download).await.0, None, "tried again later");

    // Once recording works again, the next run trashes it.
    sqlx::query("DROP TRIGGER no_trash_records")
        .execute(&lib.pool)
        .await
        .unwrap();
    assert_eq!(retire(&lib.pool).await.unwrap().trashed, 1);
    assert!(!lib.root.join(old_path).exists());
}

#[tokio::test]
async fn an_upgrade_keeps_who_watched_it_and_where_they_were() {
    let lib = library().await;
    let old_path = "Dune (2024)/Dune.2024.1080p.WEB-DL.mkv";
    let old = file(&lib, old_path, None, Some(166.0)).await;
    for (name, position, watched, plays) in [("alice", 0.0, 1, 2), ("bob", 3600.0, 0, 0)] {
        let user = crate::auth::create_user(&lib.pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
                 watched, play_count, updated_at) VALUES (?, ?, ?, 9960, ?, ?, 5)",
        )
        .bind(user.id)
        .bind(old)
        .bind(position)
        .bind(watched)
        .bind(plays)
        .execute(&lib.pool)
        .await
        .unwrap();
    }
    let replaces = replaced_by(&lib.pool, Some(1), None, None, "x")
        .await
        .unwrap();
    let new_path = "Dune (2024)/Dune.2024.2160p.REMUX.mkv";
    imported(&lib, &replaces, &[new_path], 0).await;
    let new = file(&lib, new_path, None, Some(166.5)).await;
    assert_eq!(retire(&lib.pool).await.unwrap().trashed, 1);
    let carried: Vec<(String, f64, i64, i64)> = sqlx::query_as(
        "SELECT u.username, p.position_seconds, p.watched, p.play_count
         FROM playback_progress p JOIN users u ON u.id = p.user_id
         WHERE p.file_id = ? ORDER BY u.username",
    )
    .bind(new)
    .fetch_all(&lib.pool)
    .await
    .unwrap();
    assert_eq!(
        carried,
        [
            ("alice".to_owned(), 0.0, 1, 2),
            ("bob".to_owned(), 3600.0, 0, 0)
        ]
    );
}
