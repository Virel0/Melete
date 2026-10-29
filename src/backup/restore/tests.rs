use super::*;
use crate::backup::tests::{all, database, read, up_to};

/// The files directly in `dir`, sorted, the lock file left out.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n != LOCK_FILE)
        .collect();
    names.sort();
    names
}

async fn add_user(pool: &SqlitePool, name: &str) {
    sqlx::query(
        "INSERT INTO users (username, password_hash, is_admin, created_at) VALUES (?, 'x', 0, 0)",
    )
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_restore_goes_back_to_the_copy_and_keeps_what_was_there() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &all()).await;
    let made = by_hand(&pool, dir.path()).await.unwrap();
    // A change after the copy, still in the WAL when the server stops.
    add_user(&pool, "bob").await;
    let wal = dir.path().join("melete.db-wal");
    assert!(wal.metadata().unwrap().len() > 0);
    // As a server stopped hard leaves it: the WAL and shared memory still there.
    std::mem::forget(pool);

    let restored = restore(dir.path(), &made.file, &all()).await.unwrap();
    assert_eq!(restored.from, dir.path().join(FOLDER).join(&made.file));
    assert!(!restored.behind);
    assert_eq!(
        restored.version,
        all().iter().map(|m| m.version).max().unwrap()
    );
    let (_, users) = read(&dir.path().join("melete.db")).await;
    assert_eq!(users, ["alice"], "the change after the copy is gone");
    // The old WAL isn't left to be replayed onto the copy.
    assert!(!wal.exists());
    assert_eq!(names(dir.path()), ["backups", "melete.db"]);

    let Kept::Copied(kept) = &restored.kept else {
        panic!("kept by a copy: {:?}", restored.kept);
    };
    assert!(kept.file.starts_with("before-restoring-"), "{}", kept.file);
    let (_, users) = read(&dir.path().join(FOLDER).join(&kept.file)).await;
    assert_eq!(users, ["alice", "bob"], "what was there, its WAL included");

    // And the database opens as Melete opens it, with nothing to migrate or copy.
    let pool = crate::db::open(dir.path()).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn no_restore_while_the_server_runs() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &all()).await;
    let made = by_hand(&pool, dir.path()).await.unwrap();
    add_user(&pool, "bob").await;
    let server = lock(dir.path()).unwrap();
    let err = restore(dir.path(), &made.file, &all()).await.unwrap_err();
    assert!(format!("{err:#}").contains("is locked"), "{err:#}");
    assert!(lock(dir.path()).is_err(), "one holder at a time");
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 2, "nothing replaced");
    assert_eq!(
        names(&dir.path().join(FOLDER)),
        [made.file.as_str()],
        "nothing kept aside"
    );
    // Once the server's gone, it can.
    drop(server);
    pool.close().await;
    restore(dir.path(), &made.file, &all()).await.unwrap();
}

#[tokio::test]
async fn a_copy_that_fails_its_checks_is_refused_and_nothing_changes() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &all()).await;
    let made = by_hand(&pool, dir.path()).await.unwrap();
    pool.close().await;
    let backups = dir.path().join(FOLDER);
    std::fs::write(backups.join("garbage.db"), "not a database at all").unwrap();
    // A copy made by a newer Melete, with a migration this one doesn't have.
    std::fs::copy(backups.join(&made.file), backups.join("newer.db")).unwrap();
    let mut conn = SqliteConnectOptions::new()
        .filename(backups.join("newer.db"))
        .connect()
        .await
        .unwrap();
    sqlx::query("INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) VALUES (99999, 'later', 1, x'00', 0)")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    // A copy damaged part way through: the pages past its first few overwritten.
    let mut damaged = std::fs::read(backups.join(&made.file)).unwrap();
    let page = 4096;
    for byte in &mut damaged[page * 8..page * 12] {
        *byte = 0x5a;
    }
    std::fs::write(backups.join("damaged.db"), damaged).unwrap();
    let before = std::fs::read(dir.path().join("melete.db")).unwrap();

    for (chosen, why) in [
        ("garbage.db", "not a database"),
        ("damaged.db", "malformed"),
        ("newer.db", "made by a newer Melete"),
        ("missing.db", "reading"),
    ] {
        let err = restore(dir.path(), chosen, &all()).await.unwrap_err();
        assert!(format!("{err:#}").contains(why), "{chosen}: {err:#}");
    }
    let melete_db = dir.path().join("melete.db");
    let err = restore(dir.path(), melete_db.to_str().unwrap(), &all())
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").contains("the database itself"),
        "{err:#}"
    );

    assert_eq!(std::fs::read(&melete_db).unwrap(), before);
    assert_eq!(
        names(dir.path()),
        ["backups", "melete.db"],
        "no copy left beside it"
    );
    assert_eq!(
        names(&backups),
        [made.file.as_str(), "damaged.db", "garbage.db", "newer.db"],
        "nothing kept aside"
    );
}

#[tokio::test]
async fn an_older_copy_is_restored_and_migrated_when_melete_starts() {
    let dir = tempfile::tempdir().unwrap();
    let old = tempfile::tempdir().unwrap();
    database(old.path(), &up_to(75)).await.close().await;
    database(dir.path(), &all()).await.close().await;
    let path = old.path().join("melete.db");
    let restored = restore(dir.path(), path.to_str().unwrap(), &all())
        .await
        .unwrap();
    assert_eq!((restored.version, restored.behind), (74, true));
    let pool = crate::db::open(dir.path()).await.unwrap();
    let copies: Vec<String> = sqlx::query_scalar("SELECT file FROM backups")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(copies.len(), 1);
    assert!(
        copies[0].starts_with("before-migrating-from-0074-"),
        "{copies:?}"
    );
}

#[tokio::test]
async fn a_database_sqlite_cannot_copy_is_moved_aside_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &all()).await;
    let made = by_hand(&pool, dir.path()).await.unwrap();
    pool.close().await;
    std::fs::write(dir.path().join("melete.db"), "broken").unwrap();
    let restored = restore(dir.path(), &made.file, &all()).await.unwrap();
    let Kept::Moved { files, .. } = &restored.kept else {
        panic!("moved: {:?}", restored.kept);
    };
    assert_eq!(files.len(), 1);
    let moved = dir.path().join(FOLDER).join(&files[0]);
    assert_eq!(std::fs::read(moved).unwrap(), b"broken");
    assert_eq!(read(&dir.path().join("melete.db")).await.1, ["alice"]);
}

#[test]
fn copies_are_listed_newest_first_and_only_copies() {
    let dir = tempfile::tempdir().unwrap();
    assert!(copies(dir.path()).unwrap().is_empty());
    let backups = folder(dir.path()).unwrap();
    for name in [
        "daily-2026-10-27.db",
        "by-hand-x.db",
        ".daily-x.db.part",
        "notes.txt",
    ] {
        std::fs::write(backups.join(name), "x").unwrap();
    }
    std::fs::create_dir(backups.join("folder.db")).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    File::options()
        .write(true)
        .open(backups.join("daily-2026-10-27.db"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let names: Vec<String> = copies(dir.path())
        .unwrap()
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(names, ["by-hand-x.db", "daily-2026-10-27.db"]);
}
