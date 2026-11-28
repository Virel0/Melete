use super::*;
use crate::library::{Library, LibraryKind};

/// A library in a folder of its own, known to the library check, with one file in its trash;
/// the trash entry's id.
async fn trashed(pool: &SqlitePool, id: i64, root: &Path, name: &str) -> i64 {
    sqlx::query(
        "INSERT OR IGNORE INTO libraries (id, name, kind, path) VALUES (?, ?, 'movies', ?)",
    )
    .bind(id)
    .bind(name)
    .bind(root.to_string_lossy().into_owned())
    .execute(pool)
    .await
    .unwrap();
    let rel = format!("{TRASH}/1/{name}.mkv");
    let path = root.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"old").unwrap();
    sqlx::query_scalar(
        "INSERT INTO trashed_files (library_id, rel_path, trash_path, size, replaced_by, trashed_at)
         VALUES (?, ?, ?, 3, 'x', 0) RETURNING id",
    )
    .bind(id)
    .bind(format!("{name}.mkv"))
    .bind(&rel)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn row(pool: &SqlitePool, id: i64) -> TrashRow {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ROWS} AND t.id = ?")))
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn deleted(pool: &SqlitePool, id: i64) -> bool {
    sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM trashed_files WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A delete that takes a second and a half, as one on a share that stopped answering would
/// take hours.
fn slow_unlink(root: &Path, path: &Path, size: i64) -> Result<(), String> {
    std::thread::sleep(Duration::from_millis(1500));
    unlink_trashed(root, path, size)
}

#[tokio::test]
async fn a_stuck_delete_holds_up_neither_the_rest_nor_the_database() {
    let pool = crate::db::open_in_memory().await;
    let (frozen, fine) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let stuck = trashed(&pool, 1, frozen.path(), "frozen").await;
    let after = trashed(&pool, 1, frozen.path(), "after").await;
    let other = trashed(&pool, 2, fine.path(), "fine").await;
    let library = |id, name: &str, path: &Path| Library {
        id,
        name: name.into(),
        kind: LibraryKind::Movies,
        path: path.to_path_buf(),
    };
    crate::library::health::load(
        &pool,
        &[
            library(1, "frozen", frozen.path()),
            library(2, "fine", fine.path()),
        ],
    )
    .await
    .unwrap();

    let started = Instant::now();
    let stuck_row = row(&pool, stuck).await;
    let went = delete_row_with(&pool, stuck_row, Duration::from_millis(100), slow_unlink).await;
    assert_eq!(went.unwrap(), Deletion::Waiting);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the caller moved on"
    );
    // The database is free meanwhile: the one connection takes a write at once.
    tokio::time::timeout(
        Duration::from_millis(500),
        sqlx::query("UPDATE trashed_files SET size = size WHERE id = ?")
            .bind(other)
            .execute(&pool),
    )
    .await
    .expect("no write waits for the stuck delete")
    .unwrap();
    // Its library now counts as not responding: the next file there is left, saying so, and
    // another library's file goes.
    let next = delete_row(&pool, row(&pool, after).await).await.unwrap();
    assert!(
        matches!(&next, Deletion::Left(why) if why.contains("frozen library isn't responding")),
        "{next:?}"
    );
    assert_eq!(
        delete_row(&pool, row(&pool, other).await).await.unwrap(),
        Deletion::Gone
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    // The stuck delete finishes in the end, and is recorded then.
    assert!(!deleted(&pool, stuck).await);
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(deleted(&pool, stuck).await, "recorded once it came back");
    assert!(!frozen.path().join(TRASH).join("1/frozen.mkv").exists());
    assert!(
        frozen.path().join(TRASH).join("1/after.mkv").exists(),
        "left while its library wasn't responding"
    );
}

#[tokio::test]
async fn emptying_the_trash_in_the_background_says_how_far_it_got() {
    let pool = crate::db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    let first = trashed(&pool, 1, dir.path(), "one").await;
    let second = trashed(&pool, 1, dir.path(), "two").await;
    // One changed since the trash put it there: left.
    std::fs::write(dir.path().join(TRASH).join("1/two.mkv"), b"changed").unwrap();
    let started = start_emptying(&pool).await.unwrap();
    assert!(started.running && started.started_at.is_some());
    // Asked again while it runs: the same run.
    let again = start_emptying(&pool).await.unwrap();
    assert_eq!(again.started_at, started.started_at);
    let done = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let now = emptying();
            if !now.running {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        (
            done.files,
            done.deleted,
            done.left,
            done.waiting,
            done.freed
        ),
        (2, 1, 1, 0, 3)
    );
    assert!(done.current.is_none() && done.finished_at.is_some());
    assert!(deleted(&pool, first).await);
    assert!(!deleted(&pool, second).await);
}
