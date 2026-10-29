//! The database: opening `melete.db` with its migrations applied, and small helpers every
//! module shares (the time now, spotting a duplicate).

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
}
