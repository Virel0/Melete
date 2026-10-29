use std::borrow::Cow;

use sqlx::migrate::{Migration, MigrationType};
use sqlx::sqlite::{SqliteJournalMode, SqlitePoolOptions};

use super::*;

/// Every migration Melete has.
pub(super) fn all() -> Migrator {
    sqlx::migrate!()
}

/// Melete's migrations up to (not including) `version`.
pub(super) fn up_to(version: i64) -> Migrator {
    let all = all();
    Migrator {
        migrations: all
            .migrations
            .iter()
            .filter(|m| m.version < version)
            .cloned()
            .collect(),
        ..Migrator::DEFAULT
    }
}

/// Melete's migrations and, after them, one that fails part way.
fn with_a_failing_one() -> Migrator {
    let mut migrations = all().migrations.into_owned();
    let last = migrations.last().map_or(0, |m| m.version);
    migrations.push(Migration::new(
        last + 1,
        Cow::Borrowed("fails"),
        MigrationType::Simple,
        sqlx::SqlStr::from_static(
            "CREATE TABLE half_done (x); INSERT INTO no_such_table VALUES (1);",
        ),
        false,
    ));
    Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    }
}

/// A database file in `dir` as Melete keeps it (WAL), migrated by `migrator`, with a user.
pub(super) async fn database(dir: &Path, migrator: &Migrator) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(dir.join("melete.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    migrator.run(&pool).await.unwrap();
    sqlx::query("INSERT INTO users (username, password_hash, is_admin, created_at) VALUES ('alice', 'x', 1, 0)")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

/// The newest migration a database file holds, and its users.
pub(super) async fn read(path: &Path) -> (i64, Vec<String>) {
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .connect()
        .await
        .unwrap();
    let version = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    let users = sqlx::query_scalar("SELECT username FROM users ORDER BY id")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    (version, users)
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join(FOLDER))
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn a_copy_holds_what_is_still_in_the_wal() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &all()).await;
    // Written to the WAL and not yet into the database file, which a file copy would miss.
    sqlx::query("INSERT INTO users (username, password_hash, is_admin, created_at) VALUES ('bob', 'x', 0, 0)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(dir.path().join("melete.db-wal").metadata().unwrap().len() > 0);
    let copy = copy(&pool, dir.path(), "x.db").await.unwrap();
    let path = dir.path().join(FOLDER).join(&copy.file);
    assert_eq!(copy.bytes, path.metadata().unwrap().len());
    assert_eq!(read(&path).await.1, ["alice", "bob"]);
    assert_eq!(files(dir.path()), ["x.db"], "no part file left behind");
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &dir.path().join(FOLDER).metadata().unwrap().permissions(),
    );
    assert_eq!(mode & 0o777, 0o700, "for Melete's user alone");
    // Never over a copy that's there.
    assert!(super::copy(&pool, dir.path(), "x.db").await.is_err());
}

#[tokio::test]
async fn a_migration_that_fails_leaves_the_copy_made_before_it() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &up_to(75)).await;
    pool.close().await;
    let opened = crate::db::open_with(dir.path(), &with_a_failing_one()).await;
    assert!(opened.is_err());
    let names = files(dir.path());
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        names[0].starts_with("before-migrating-from-0074-"),
        "named after the schema it holds: {}",
        names[0]
    );
    let path = dir.path().join(FOLDER).join(&names[0]);
    check(&path).await.unwrap();
    assert_eq!(read(&path).await, (74, vec!["alice".to_owned()]));
}

#[tokio::test]
async fn no_copy_no_migrating() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &up_to(75)).await;
    pool.close().await;
    // The copy can't be made: where its folder goes is a file.
    std::fs::write(dir.path().join(FOLDER), "").unwrap();
    let err = crate::db::open_with(dir.path(), &all()).await.unwrap_err();
    assert!(
        format!("{err:#}").contains("nothing was migrated"),
        "{err:#}"
    );
    let (version, _) = read(&dir.path().join("melete.db")).await;
    assert_eq!(version, 74, "still as it was");
}

#[tokio::test]
async fn copies_only_when_there_is_something_to_migrate_and_lose() {
    let dir = tempfile::tempdir().unwrap();
    // A new database: nothing in it to lose.
    let pool = crate::db::open(dir.path()).await.unwrap();
    pool.close().await;
    assert!(files(dir.path()).is_empty());
    // Up to date: nothing to migrate.
    let pool = crate::db::open(dir.path()).await.unwrap();
    pool.close().await;
    assert!(files(dir.path()).is_empty());
    // Behind: copied, and the copy recorded once migrated.
    let behind = tempfile::tempdir().unwrap();
    database(behind.path(), &up_to(75)).await.close().await;
    let pool = crate::db::open(behind.path()).await.unwrap();
    assert_eq!(files(behind.path()).len(), 1);
    let recorded: (String, i64) =
        sqlx::query_as("SELECT kind, count(*) FROM backups WHERE file IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(recorded, ("before_migrating".into(), 1));
}

#[tokio::test]
async fn one_daily_copy_a_day_and_a_failure_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let pool = database(dir.path(), &all()).await;
    let made = daily_once(&pool, dir.path()).await.unwrap().unwrap();
    assert_eq!(
        made.file,
        format!("daily-{}.db", dates::date(crate::db::unix_now()))
    );
    assert_eq!(
        daily_once(&pool, dir.path()).await.unwrap(),
        None,
        "once a day"
    );
    assert_eq!(files(dir.path()), std::slice::from_ref(&made.file));
    let health = crate::admin_health::check(&pool).await.unwrap();
    assert!(
        health
            .areas
            .iter()
            .all(|a| a.findings.iter().all(|f| !f.code.starts_with("backup")))
    );

    // The folder's gone and a file is in its way: the attempt fails, and says why.
    std::fs::remove_dir_all(dir.path().join(FOLDER)).unwrap();
    std::fs::write(dir.path().join(FOLDER), "").unwrap();
    assert!(daily_once(&pool, dir.path()).await.is_err());
    let health = crate::admin_health::check(&pool).await.unwrap();
    let system = health.areas.iter().find(|a| a.area == "system").unwrap();
    let failed = system
        .findings
        .iter()
        .find(|f| f.code == "backup_failed")
        .unwrap();
    assert!(failed.message.contains("making"), "{}", failed.message);
}

#[tokio::test]
async fn an_old_newest_copy_is_pointed_out() {
    let pool = crate::db::open_in_memory().await;
    let now = crate::db::unix_now();
    sqlx::query(
        "INSERT INTO backups (kind, file, bytes, made_at) VALUES ('by_hand', 'x.db', 1, ?)",
    )
    .bind(now - STALE_AFTER + 60)
    .execute(&pool)
    .await
    .unwrap();
    let codes = |health: crate::admin_health::Health| -> Vec<&'static str> {
        health
            .areas
            .into_iter()
            .flat_map(|a| a.findings)
            .map(|f| f.code)
            .filter(|code| code.starts_with("backup"))
            .collect()
    };
    assert!(codes(crate::admin_health::check(&pool).await.unwrap()).is_empty());
    sqlx::query("UPDATE backups SET made_at = ?")
        .bind(now - 3 * 86_400)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        codes(crate::admin_health::check(&pool).await.unwrap()),
        ["backup_old"]
    );
}

#[tokio::test]
async fn pruning_leaves_folders_links_and_other_files_alone() {
    let dir = tempfile::tempdir().unwrap();
    let backups = folder(dir.path()).unwrap();
    let today = dates::day(crate::db::unix_now());
    for ago in 0..20 {
        let name = format!("daily-{}.db", dates::date((today - ago) * 86_400));
        std::fs::write(backups.join(name), "").unwrap();
    }
    let old = format!("daily-{}.db", dates::date((today - 400) * 86_400));
    std::fs::create_dir(backups.join(&old)).unwrap();
    std::fs::write(dir.path().join("elsewhere.db"), "").unwrap();
    let linked = format!("daily-{}.db", dates::date((today - 401) * 86_400));
    std::os::unix::fs::symlink(dir.path().join("elsewhere.db"), backups.join(&linked)).unwrap();
    std::fs::write(backups.join("notes.txt"), "").unwrap();
    let removed = prune(&backups, crate::db::unix_now()).unwrap();
    assert!(!removed.is_empty());
    let left = files(dir.path());
    assert!(left.contains(&old) && left.contains(&linked) && left.contains(&"notes.txt".into()));
    assert!(dir.path().join("elsewhere.db").exists());
    assert!(left.len() <= DAILY_KEPT + WEEKLY_KEPT + 3, "{left:?}");
}
