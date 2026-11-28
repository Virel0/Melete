//! The database: opening `melete.db` with its migrations applied, and small helpers every
//! module shares (the time now, spotting a duplicate, a transaction that writes, a busy
//! database).

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

/// Opens (creating if needed) `melete.db` in `data_dir` and applies pending migrations, after
/// copying the database into the backups folder; without that copy nothing is migrated.
pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    open_with(data_dir, &sqlx::migrate!()).await
}

/// [`open`], with the migrations in `migrator`.
pub async fn open_with(data_dir: &Path, migrator: &Migrator) -> anyhow::Result<SqlitePool> {
    let path = data_dir.join("melete.db");
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        // The container's root filesystem is read-only, so SQLite has no temp folder.
        // Large transactions (a full library scan) fail without this.
        .pragma("temp_store", "memory");
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let before = crate::backup::before_migrating(&pool, data_dir, migrator)
        .await
        .context("backing up the database before migrating it; nothing was migrated")?;
    migrator.run(&pool).await.context("applying migrations")?;
    if let Some(copy) = before {
        tracing::info!(
            file = copy.file,
            bytes = copy.bytes,
            "backed up the database before migrating it"
        );
        crate::backup::record(&pool, crate::backup::Kind::BeforeMigrating, &Ok(copy)).await?;
    }
    Ok(pool)
}

/// The time now as the database keeps it: seconds since the Unix epoch. One place for it,
/// instead of a copy in every module.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Milliseconds since the Unix epoch, for the Aoide protocol's times.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Whether a database error is a UNIQUE constraint refusing a duplicate.
pub fn unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.is_unique_violation())
}

/// Starts a transaction that writes: it takes the write lock as it begins (`BEGIN
/// IMMEDIATE`), waiting for it as long as the busy timeout allows. A plain `BEGIN` takes it
/// only at the first write, and one that has read by then is refused at once with "database is
/// locked" while another connection writes: SQLite can't let it wait, as its reads may be stale
/// by the time the lock is free.
pub async fn write(pool: &SqlitePool) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// Whether a database error is SQLite saying the database is busy or locked: tried again, it
/// may go through.
pub fn busy(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db)
    if db.code().is_some_and(|code| {
        // SQLITE_BUSY and SQLITE_LOCKED, with their extended codes.
        code.parse::<i32>().is_ok_and(|code| matches!(code & 0xff, 5 | 6))
    }))
}

/// An error that may be the database being busy.
pub trait MaybeBusy {
    fn is_busy(&self) -> bool;
}

impl MaybeBusy for sqlx::Error {
    fn is_busy(&self) -> bool {
        busy(self)
    }
}

impl MaybeBusy for anyhow::Error {
    fn is_busy(&self) -> bool {
        self.chain()
            .any(|err| err.downcast_ref::<sqlx::Error>().is_some_and(busy))
    }
}

/// How many times [`retry_busy`] tries.
const TRIES: u32 = 3;

/// Runs `work` again, up to [`TRIES`] times in all, while it fails with the database busy, a
/// second apart then two: for background steps, so a burst of other writes doesn't cost a run.
pub async fn retry_busy<T, E, F, Fut>(what: &str, mut work: F) -> Result<T, E>
where
    E: MaybeBusy,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut tried = 1;
    loop {
        match work().await {
            Err(err) if err.is_busy() && tried < TRIES => {
                tracing::info!(what, tried, "the database was busy; trying again");
                tokio::time::sleep(Duration::from_secs(u64::from(tried))).await;
                tried += 1;
            }
            done => return done,
        }
    }
}

/// A migrated in-memory database, for tests. One connection, since each would get its own.
#[cfg(test)]
pub async fn open_in_memory() -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("an in-memory database needs no file, so it opens");
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("the migrations apply cleanly to an empty database");
    pool
}

#[cfg(test)]
mod tests {
    use sqlx::migrate::Migrator;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    /// The indexers rebuild (0020) keeps every row, and what other tables link to them.
    #[tokio::test]
    async fn rebuilding_indexers_keeps_what_links_to_them() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        let all = sqlx::migrate!();
        let before = Migrator {
            migrations: all
                .migrations
                .iter()
                .filter(|m| m.version < 20)
                .cloned()
                .collect(),
            ..Migrator::DEFAULT
        };
        before.run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO indexers (id, name, url, api_key, enabled, categories, movie_search_params,
                 tv_search_params, source, source_id, added_at)
             VALUES (5, 'Tracker', 'http://p/5/api', 'key', 1, '[2000]', '[\"q\"]', '[]', 'prowlarr', 5, 0);
             INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at) VALUES (1, 1, 'X', 1, 'radarr', 0);
             INSERT INTO searches (id, wanted_movie_id, searched_at, indexers) VALUES (1, 1, 0, '[]');
             INSERT INTO release_decisions (search_id, indexer_id, title, accepted, decision)
             VALUES (1, 5, 'X.2024.1080p', 1, '{}');
             INSERT INTO rss_syncs (id, synced_at, indexers, new_releases, grabbed) VALUES (1, 0, '[]', 0, '[]');
             INSERT INTO rss_seen (indexer_id, release_key, seen_at) VALUES (5, 'k', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        all.run(&pool).await.unwrap();
        let kept: (Option<i64>, i64, String, String) = sqlx::query_as(
            "SELECT (SELECT indexer_id FROM release_decisions), (SELECT count(*) FROM rss_seen),
                    (SELECT kind FROM indexers WHERE id = 5), (SELECT api_key FROM indexers WHERE id = 5)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kept, (Some(5), 1, "torznab".into(), "key".into()));
        let on: bool = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(on, "foreign keys are back on");
        let broken: Vec<(String,)> = sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(broken.is_empty());
        // Melete's own need no source id; Prowlarr's still do.
        sqlx::query(
            "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                 tv_search_params, source, kind, added_at)
             VALUES ('The Pirate Bay', 'https://apibay.org', '', 1, '[2000]', '[]', '[]', 'melete', 'apibay', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            sqlx::query("INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params, tv_search_params, source, added_at) VALUES ('x', 'u', '', 1, '[]', '[]', '[]', 'prowlarr', 0)")
                .execute(&pool)
                .await
                .is_err()
        );
    }

    /// Adding Uindex's kind (0021) rebuilds the table again, keeping each indexer's kind.
    #[tokio::test]
    async fn adding_uindex_keeps_kinds_and_links() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        let all = sqlx::migrate!();
        let before = Migrator {
            migrations: all
                .migrations
                .iter()
                .filter(|m| m.version < 21)
                .cloned()
                .collect(),
            ..Migrator::DEFAULT
        };
        before.run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO indexers (id, name, url, api_key, enabled, categories, movie_search_params,
                 tv_search_params, source, kind, added_at)
             VALUES (3, 'The Pirate Bay', 'https://apibay.org', '', 1, '[2000]', '[]', '[]', 'melete', 'apibay', 0);
             INSERT INTO rss_syncs (id, synced_at, indexers, new_releases, grabbed) VALUES (1, 0, '[]', 0, '[]');
             INSERT INTO rss_seen (indexer_id, release_key, seen_at) VALUES (3, 'k', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        all.run(&pool).await.unwrap();
        let kept: (String, i64) = sqlx::query_as(
            "SELECT (SELECT kind FROM indexers WHERE id = 3), (SELECT count(*) FROM rss_seen)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kept, ("apibay".into(), 1));
        let on: bool = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(on);
        sqlx::query(
            "INSERT INTO indexers (name, url, api_key, enabled, categories, movie_search_params,
                 tv_search_params, source, kind, added_at)
             VALUES ('Uindex', 'https://uindex.org', '', 1, '[2000]', '[]', '[]', 'melete', 'uindex', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn adding_music_keeps_libraries_and_everything_in_them() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        let all = sqlx::migrate!();
        let before = Migrator {
            migrations: all
                .migrations
                .iter()
                .filter(|m| m.version < 22)
                .cloned()
                .collect(),
            ..Migrator::DEFAULT
        };
        before.run(&pool).await.unwrap();
        // A movie with a file and a watcher's progress, all hanging off the library.
        sqlx::raw_sql(
            "INSERT INTO libraries (id, name, kind, path, scan_generation) VALUES (4, 'movies', 'movies', '/m', 7);
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 4, 'Dune (2021)', 'Dune', 7);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
                 VALUES (1, 4, 'Dune (2021)/Dune.mkv', 1, 1, 1, 7, 7);
             INSERT INTO users (id, username, password_hash, is_admin, created_at)
                 VALUES (1, 'v', 'x', 1, 0);
             INSERT INTO playback_progress (user_id, file_id, position_seconds, updated_at)
                 VALUES (1, 1, 60, 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        all.run(&pool).await.unwrap();
        let kept: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT kind || scan_generation FROM libraries WHERE id = 4),
                 (SELECT count(*) FROM movies WHERE library_id = 4),
                 (SELECT count(*) FROM media_files WHERE library_id = 4),
                 (SELECT count(*) FROM playback_progress)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kept, ("movies7".into(), 1, 1, 1));
        let broken: Vec<(String,)> = sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(broken.is_empty(), "{broken:?}");
        let on: bool = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(on);
        sqlx::query("INSERT INTO libraries (name, kind, path) VALUES ('music', 'music', '/music')")
            .execute(&pool)
            .await
            .unwrap();
        // Rows can still only point at libraries that exist.
        let orphan = sqlx::query(
            "INSERT INTO movies (library_id, rel_path, parsed_title, seen_generation)
             VALUES (99, 'x', 'x', 1)",
        )
        .execute(&pool)
        .await;
        assert!(orphan.is_err());
    }

    /// Two connections to one file, as the server has.
    async fn two_connections() -> (tempfile::TempDir, sqlx::SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let options = SqliteConnectOptions::new()
            .filename(dir.path().join("t.db"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (n INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        (dir, pool)
    }

    /// A transaction that reads before it writes, begun with a plain BEGIN, is refused at once
    /// while another connection writes, busy timeout or not; begun with [`super::write`], it
    /// waits its turn and goes through.
    #[tokio::test]
    async fn a_write_transaction_waits_for_the_lock_rather_than_being_refused() {
        let (_dir, pool) = two_connections().await;
        let mut holding = super::write(&pool).await.unwrap();
        sqlx::query("INSERT INTO t (n) VALUES (1)")
            .execute(&mut *holding)
            .await
            .unwrap();

        let mut plain = pool.begin().await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT count(*) FROM t")
            .fetch_one(&mut *plain)
            .await
            .unwrap();
        let started = std::time::Instant::now();
        let refused = sqlx::query("INSERT INTO t (n) VALUES (2)")
            .execute(&mut *plain)
            .await
            .unwrap_err();
        assert!(super::busy(&refused), "{refused}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "not waited for"
        );
        drop(plain);

        let second = pool.clone();
        let waiting = tokio::spawn(async move {
            let mut tx = super::write(&second).await.unwrap();
            let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM t")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            sqlx::query("INSERT INTO t (n) VALUES (3)")
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            seen
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        holding.commit().await.unwrap();
        assert_eq!(
            waiting.await.unwrap(),
            1,
            "it read after the other's commit"
        );
    }

    /// A transaction begun with a plain BEGIN that reads, then writes while another connection
    /// holds the write lock: refused at once.
    async fn read_then_write(pool: &sqlx::SqlitePool) -> sqlx::Result<()> {
        let mut tx = pool.begin().await?;
        let _: i64 = sqlx::query_scalar("SELECT count(*) FROM t")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO t (n) VALUES (9)")
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }

    #[tokio::test]
    async fn a_busy_step_is_tried_again_a_few_times_then_given_up() {
        let (_dir, pool) = two_connections().await;
        let tries = std::sync::atomic::AtomicU32::new(0);
        let step = || {
            tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            read_then_write(&pool)
        };
        // Busy for the first try only.
        let mut holding = super::write(&pool).await.unwrap();
        sqlx::query("INSERT INTO t (n) VALUES (1)")
            .execute(&mut *holding)
            .await
            .unwrap();
        let release = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            holding.commit().await.unwrap();
        };
        let (done, ()) = tokio::join!(super::retry_busy("a test", step), release);
        done.unwrap();
        assert_eq!(tries.swap(0, std::sync::atomic::Ordering::SeqCst), 2);
        // Busy throughout: given up after three tries, still busy.
        let mut holding = super::write(&pool).await.unwrap();
        sqlx::query("INSERT INTO t (n) VALUES (1)")
            .execute(&mut *holding)
            .await
            .unwrap();
        let given_up = super::retry_busy("a test", step).await.unwrap_err();
        assert!(super::busy(&given_up), "{given_up}");
        assert_eq!(
            tries.load(std::sync::atomic::Ordering::SeqCst),
            super::TRIES
        );
        holding.rollback().await.unwrap();
    }
}
