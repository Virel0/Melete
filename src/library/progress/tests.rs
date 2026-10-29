use std::path::PathBuf;

use super::*;
use crate::library::{LibraryConfig, LibraryKind, scan, sync_config};

async fn rows(pool: &SqlitePool) -> Vec<(i64, bool, i64)> {
    sqlx::query_as("SELECT library_id, whole, folders_done FROM scan_progress")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn movies(root: PathBuf) -> (SqlitePool, crate::library::Library) {
    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: root,
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    (pool, library)
}

#[tokio::test]
async fn a_scan_is_noted_while_it_runs_with_the_folders_its_walk_has_read() {
    let dir = tempfile::tempdir().unwrap();
    let (pool, library) = movies(dir.path().into()).await;
    let progress = Progress::start(&pool, library.id, true).await;
    assert_eq!(rows(&pool).await, [(library.id, true, 0)]);
    let folders = progress.folders();
    let walk = async {
        folders.store(5, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let seen = rows(&pool).await;
        folders.store(7, Ordering::Relaxed);
        seen
    };
    let seen = progress.walking(Duration::from_millis(20), walk).await;
    assert_eq!(seen, [(library.id, true, 5)], "written as it walks");
    assert_eq!(
        rows(&pool).await,
        [(library.id, true, 7)],
        "and once it's done"
    );
    progress.finish().await;
    assert!(rows(&pool).await.is_empty());
}

#[tokio::test]
async fn a_scan_leaves_nothing_noted_whether_it_worked_or_failed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Film (2020)")).unwrap();
    std::fs::write(dir.path().join("Film (2020)/Film.mkv"), b"video").unwrap();
    let (pool, library) = movies(dir.path().into()).await;
    assert_eq!(scan(&pool, &library).await.unwrap().files, 1);
    assert!(rows(&pool).await.is_empty());
    std::fs::remove_dir_all(dir.path().join("Film (2020)")).unwrap();
    assert!(scan(&pool, &library).await.is_err(), "an empty root fails");
    assert!(rows(&pool).await.is_empty());
}

#[tokio::test]
async fn running_scans_are_listed_for_those_who_may_see_their_library() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'music', 'music', '/a');
         INSERT INTO scan_progress (library_id, whole, folders_done, started_at, updated_at)
             VALUES (1, 0, 3, 10, 11), (2, 1, 40, 20, 21);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let everything = crate::access::Access::everything().library_sql("p.library_id");
    let listed = running(&pool, &everything, true, true).await.unwrap();
    assert_eq!(
        listed[0],
        Running {
            library: "movies".into(),
            kind: "movies".into(),
            whole: false,
            folders_done: 3,
            started_at: 10,
        }
    );
    assert_eq!(listed.len(), 2);
    let music_only = running(&pool, &everything, false, true).await.unwrap();
    assert_eq!(music_only.len(), 1);
    assert_eq!(music_only[0].library, "music");
    let limited = crate::access::Access::limited(Some(vec![2]), None).library_sql("p.library_id");
    let seen: Vec<String> = running(&pool, &limited, true, true)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.library)
        .collect();
    assert_eq!(seen, ["music"]);
    recover(&pool).await.unwrap();
    assert!(
        running(&pool, &everything, true, true)
            .await
            .unwrap()
            .is_empty()
    );
}
