use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sqlx::SqlitePool;

use crate::db;
use crate::library::{Library, LibraryConfig, LibraryKind, scan, sync_config};

fn write(root: &Path, rel: &str, body: &[u8], days_ago: u64) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    let when = SystemTime::now() - Duration::from_secs(days_ago * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// Renames `from` to `to` under `root`, as a person or another tool would.
fn rename(root: &Path, from: &str, to: &str) {
    std::fs::rename(root.join(from), root.join(to)).unwrap();
}

async fn library(kind: LibraryKind, root: &Path) -> (SqlitePool, Library) {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "l".into(),
        kind,
        path: PathBuf::from(root),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    crate::auth::create_user(&pool, "alice", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    (pool, library)
}

async fn id_of(pool: &SqlitePool, table: &str, rel_path: &str) -> Option<i64> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE rel_path = ?"
    )))
    .bind(rel_path)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// Alice watched it once, and is part way through it again.
async fn watch(pool: &SqlitePool, file_id: i64) {
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
             watched, play_count, updated_at)
         SELECT id, ?, 600, 3000, 1, 1, 7 FROM users",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
}

/// Alice's progress on the files still in the library: the file, its position, watched, plays.
async fn progress(pool: &SqlitePool) -> Vec<(String, f64, bool, i64)> {
    sqlx::query_as(
        "SELECT f.rel_path, p.position_seconds, p.watched, p.play_count
         FROM playback_progress p JOIN media_files f ON f.id = p.file_id
         WHERE f.missing_since IS NULL ORDER BY f.rel_path",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_renamed_series_folder_stays_the_same_series_with_its_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Show/Season 01/Show - S01E01.mkv", b"episode one", 30);
    write(
        root,
        "Show/Season 01/Show - S01E02.mkv",
        b"episode two!",
        30,
    );
    write(root, "Show/Trailers/Show.mkv", b"a trailer", 30);
    write(
        root,
        "Other/Season 01/Other - S01E01.mkv",
        b"something else",
        30,
    );
    let (pool, library) = library(LibraryKind::Shows, root).await;
    scan(&pool, &library).await.unwrap();
    let series = id_of(&pool, "series", "Show").await.unwrap();
    let first = id_of(&pool, "media_files", "Show/Season 01/Show - S01E01.mkv")
        .await
        .unwrap();
    let extra = id_of(&pool, "media_extras", "Show/Trailers/Show.mkv")
        .await
        .unwrap();
    watch(&pool, first).await;
    sqlx::query(
        "INSERT INTO wanted_series (tvdb_id, title, series_id, source, added_at, monitored,
             series_type)
         VALUES (12345, 'Show', ?, 'manual', 0, 1, 'standard')",
    )
    .bind(series)
    .execute(&pool)
    .await
    .unwrap();

    rename(root, "Show", "Show (2019) [tvdb-12345]");
    // Renamed in the same go: the second episode, by another tool.
    rename(
        root,
        "Show (2019) [tvdb-12345]/Season 01/Show - S01E02.mkv",
        "Show (2019) [tvdb-12345]/Season 01/Show (2019) - S01E02.mkv",
    );
    let report = scan(&pool, &library).await.unwrap();
    assert_eq!(report.newly_missing, 1, "only the episode renamed inside");

    let folder = "Show (2019) [tvdb-12345]";
    assert_eq!(id_of(&pool, "series", folder).await, Some(series));
    assert_eq!(
        id_of(&pool, "series", "Show").await,
        None,
        "no row left behind"
    );
    let tvdb: Option<i64> = sqlx::query_scalar("SELECT tvdb_id FROM series WHERE id = ?")
        .bind(series)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(tvdb, Some(12345), "read from its new name");
    assert_eq!(
        id_of(
            &pool,
            "media_files",
            &format!("{folder}/Season 01/Show - S01E01.mkv")
        )
        .await,
        Some(first),
        "the same file keeps its row"
    );
    assert_eq!(
        id_of(
            &pool,
            "media_extras",
            &format!("{folder}/Trailers/Show.mkv")
        )
        .await,
        Some(extra)
    );
    assert_eq!(
        progress(&pool).await,
        [(
            format!("{folder}/Season 01/Show - S01E01.mkv"),
            600.0,
            true,
            1
        )]
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM wanted_series WHERE series_id IS NOT NULL"
        )
        .await,
        1
    );
    let events: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, message FROM title_events ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        events.contains(&("renamed".into(), "the folder took the place of Show".into())),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|(kind, _)| kind == "extra_found"),
        "the trailer was there before: {events:?}"
    );
}

#[tokio::test]
async fn a_renamed_movie_folder_carries_watched_state_to_its_renamed_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Dune/Dune.mkv", b"the whole movie", 30);
    write(root, "Loose.Movie.2010.mkv", b"a loose movie", 30);
    let (pool, library) = library(LibraryKind::Movies, root).await;
    scan(&pool, &library).await.unwrap();
    let movie = id_of(&pool, "movies", "Dune").await.unwrap();
    let loose = id_of(&pool, "movies", "Loose.Movie.2010.mkv")
        .await
        .unwrap();
    let loose_file = id_of(&pool, "media_files", "Loose.Movie.2010.mkv")
        .await
        .unwrap();
    watch(
        &pool,
        id_of(&pool, "media_files", "Dune/Dune.mkv").await.unwrap(),
    )
    .await;
    watch(&pool, loose_file).await;

    rename(root, "Dune", "Dune (2021)");
    rename(root, "Dune (2021)/Dune.mkv", "Dune (2021)/Dune (2021).mkv");
    rename(root, "Loose.Movie.2010.mkv", "Loose Movie (2010).mkv");
    scan(&pool, &library).await.unwrap();

    assert_eq!(id_of(&pool, "movies", "Dune (2021)").await, Some(movie));
    assert_eq!(
        id_of(&pool, "movies", "Loose Movie (2010).mkv").await,
        Some(loose)
    );
    assert_eq!(
        id_of(&pool, "media_files", "Loose Movie (2010).mkv").await,
        Some(loose_file)
    );
    assert_eq!(
        progress(&pool).await,
        [
            ("Dune (2021)/Dune (2021).mkv".into(), 600.0, true, 1),
            ("Loose Movie (2010).mkv".into(), 600.0, true, 1),
        ],
        "the position too: the same bytes"
    );
    assert_eq!(count(&pool, "SELECT count(*) FROM movies").await, 2);
}

#[tokio::test]
async fn anything_less_clear_is_a_new_title_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Two titles with the same bytes: which one a new folder is can't be told.
    write(root, "Twin A/Twin.mkv", b"same", 30);
    write(root, "Twin B/Twin.mkv", b"same", 30);
    // A title whose file changed as its folder was renamed.
    write(root, "Changed/Changed.mkv", b"before", 30);
    // A title gone and a new folder holding only part of it.
    write(root, "Split/Part 1.mkv", b"part one", 30);
    write(root, "Split/Part 2.mkv", b"part two", 30);
    let (pool, library) = library(LibraryKind::Movies, root).await;
    scan(&pool, &library).await.unwrap();
    let before: Vec<i64> = sqlx::query_scalar("SELECT id FROM movies ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    watch(
        &pool,
        id_of(&pool, "media_files", "Twin A/Twin.mkv")
            .await
            .unwrap(),
    )
    .await;

    std::fs::remove_dir_all(root.join("Twin B")).unwrap();
    rename(root, "Twin A", "Twin (2020)");
    rename(root, "Changed", "Changed (2020)");
    write(root, "Changed (2020)/Changed.mkv", b"after it changed", 1);
    rename(root, "Split", "Split (2020)");
    std::fs::remove_file(root.join("Split (2020)/Part 2.mkv")).unwrap();
    scan(&pool, &library).await.unwrap();

    let missing: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM movies WHERE missing_since IS NOT NULL ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(missing, before, "every old title stays as it was, missing");
    assert_eq!(count(&pool, "SELECT count(*) FROM movies").await, 7);
    assert!(progress(&pool).await.is_empty());
}

#[tokio::test]
async fn a_scan_of_one_folder_follows_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Dune/Dune.mkv", b"the whole movie", 30);
    let (pool, library) = library(LibraryKind::Movies, root).await;
    scan(&pool, &library).await.unwrap();
    let movie = id_of(&pool, "movies", "Dune").await.unwrap();
    rename(root, "Dune", "Dune (2021)");
    crate::library::scan_folder(&pool, &library, "Dune (2021)")
        .await
        .unwrap();
    assert_ne!(id_of(&pool, "movies", "Dune (2021)").await, Some(movie));
    assert_eq!(id_of(&pool, "movies", "Dune").await, Some(movie));
}
