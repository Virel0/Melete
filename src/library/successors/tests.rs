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

async fn shows(root: &Path) -> (SqlitePool, Library) {
    let pool = db::open_in_memory().await;
    let config = LibraryConfig {
        name: "shows".into(),
        kind: LibraryKind::Shows,
        path: PathBuf::from(root),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    for name in ["alice", "bob"] {
        crate::auth::create_user(&pool, name, "a-long-password".into(), false)
            .await
            .unwrap()
            .unwrap();
    }
    (pool, library)
}

async fn id_of(pool: &SqlitePool, rel_path: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM media_files WHERE rel_path = ?")
        .bind(rel_path)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Alice watched it twice; Bob is an hour in.
async fn watch(pool: &SqlitePool, file_id: i64) {
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds,
             watched, play_count, updated_at)
         SELECT id, ?, CASE username WHEN 'bob' THEN 3600 ELSE 0 END, 5000,
             username = 'alice', CASE username WHEN 'alice' THEN 2 ELSE 0 END, 7
         FROM users",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
}

/// Each user's progress on the files still in the library, as `/me/progress` reads it.
async fn progress(pool: &SqlitePool) -> Vec<(String, String, f64, bool, i64)> {
    sqlx::query_as(
        "SELECT u.username, f.rel_path, p.position_seconds, p.watched, p.play_count
         FROM playback_progress p JOIN users u ON u.id = p.user_id
         JOIN media_files f ON f.id = p.file_id
         WHERE f.missing_since IS NULL ORDER BY u.username, f.rel_path",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn added_at(pool: &SqlitePool, rel_path: &str) -> i64 {
    sqlx::query_scalar("SELECT added_at FROM media_files WHERE rel_path = ?")
        .bind(rel_path)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_renamed_episode_keeps_who_watched_it_and_where_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Show/S01E01.mkv", b"pilot", 30);
    let (pool, library) = shows(root).await;
    scan(&pool, &library).await.unwrap();
    watch(&pool, id_of(&pool, "Show/S01E01.mkv").await).await;
    let first_added = added_at(&pool, "Show/S01E01.mkv").await;

    let renamed = "Show/Season 1/Show - S01E01 - Pilot.mkv";
    std::fs::create_dir_all(root.join("Show/Season 1")).unwrap();
    std::fs::rename(root.join("Show/S01E01.mkv"), root.join(renamed)).unwrap();
    scan(&pool, &library).await.unwrap();
    assert_eq!(
        progress(&pool).await,
        [
            ("alice".into(), renamed.into(), 0.0, true, 2),
            ("bob".into(), renamed.into(), 3600.0, false, 0),
        ]
    );
    assert_eq!(added_at(&pool, renamed).await, first_added, "not new");
    // Another scan carries nothing twice.
    scan(&pool, &library).await.unwrap();
    assert_eq!(progress(&pool).await[0].4, 2);
}

#[tokio::test]
async fn a_file_replaced_by_another_keeps_who_watched_it_but_not_where() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Show/S01E01.mkv", b"pilot", 30);
    let (pool, library) = shows(root).await;
    scan(&pool, &library).await.unwrap();
    watch(&pool, id_of(&pool, "Show/S01E01.mkv").await).await;
    let first_added = added_at(&pool, "Show/S01E01.mkv").await;

    // Another tool put a new file in its place: it may not play alike.
    std::fs::remove_file(root.join("Show/S01E01.mkv")).unwrap();
    write(root, "Show/S01E01.1080p.mkv", b"pilot, sharper", 0);
    scan(&pool, &library).await.unwrap();
    let file = "Show/S01E01.1080p.mkv".to_owned();
    assert_eq!(
        progress(&pool).await,
        [
            ("alice".into(), file.clone(), 0.0, true, 2),
            ("bob".into(), file.clone(), 0.0, false, 0),
        ]
    );
    assert_eq!(
        added_at(&pool, &file).await,
        first_added,
        "an upgrade isn't new"
    );
}

#[tokio::test]
async fn which_file_one_replaced_must_be_clear() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Show/S01E01.mkv", b"pilot", 30);
    write(root, "Show/S01E01.dub.mkv", b"pilot, dubbed", 30);
    write(root, "Show/S01E02.mkv", b"second", 30);
    let (pool, library) = shows(root).await;
    scan(&pool, &library).await.unwrap();
    for rel in ["Show/S01E01.mkv", "Show/S01E01.dub.mkv", "Show/S01E02.mkv"] {
        watch(&pool, id_of(&pool, rel).await).await;
    }
    // Both copies of one episode go, one new file comes: which it replaces is unclear. And a
    // new file for another episode takes nothing from this one.
    std::fs::remove_file(root.join("Show/S01E01.mkv")).unwrap();
    std::fs::remove_file(root.join("Show/S01E01.dub.mkv")).unwrap();
    write(root, "Show/S01E01.new.mkv", b"pilot again", 0);
    std::fs::remove_file(root.join("Show/S01E02.mkv")).unwrap();
    write(root, "Show/S01E03.mkv", b"third", 0);
    scan(&pool, &library).await.unwrap();
    assert!(
        progress(&pool).await.is_empty(),
        "{:?}",
        progress(&pool).await
    );
}

#[tokio::test]
async fn a_new_episode_is_new_and_counts_from_when_it_came() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Show/S01E01.mkv", b"pilot", 30);
    let (pool, library) = shows(root).await;
    scan(&pool, &library).await.unwrap();
    let now = db::unix_now();
    let first = added_at(&pool, "Show/S01E01.mkv").await;
    assert!(
        (now - 30 * 86_400 - first).abs() < 60,
        "adopted: when last changed"
    );
    // A file dated ahead of the clock still came in no later than now.
    write(root, "Show/S01E02.mkv", b"second", 0);
    let ahead = SystemTime::now() + Duration::from_secs(86_400);
    std::fs::File::options()
        .write(true)
        .open(root.join("Show/S01E02.mkv"))
        .unwrap()
        .set_modified(ahead)
        .unwrap();
    scan(&pool, &library).await.unwrap();
    let second = added_at(&pool, "Show/S01E02.mkv").await;
    assert!(second <= db::unix_now() && second >= now, "{second}");
}

/// Files known before `added_at` came in are given the earliest change of the files for the
/// same movie or the same episodes, so an upgrade already done doesn't count as new.
#[tokio::test]
async fn files_known_already_count_from_the_first_of_theirs() {
    use sqlx::migrate::Migrator;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().in_memory(true))
        .await
        .unwrap();
    let all = sqlx::migrate!();
    let adding = all
        .iter()
        .find(|m| m.description == "file added at")
        .unwrap()
        .version;
    let before = Migrator {
        migrations: all
            .migrations
            .iter()
            .filter(|m| m.version < adding)
            .cloned()
            .collect(),
        ..Migrator::DEFAULT
    };
    before.run(&pool).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'm', 'movies', '/m'), (2, 's', 'shows', '/s');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 1, 'Dune', 'Dune', 1);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES (1, 2, 'Show', 'Show', 1);
         INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id, series_id, season,
             episode, episode_end, first_seen_generation, seen_generation, missing_since) VALUES
             (1, 'Dune/old.mkv', 1, 500, 1, NULL, NULL, NULL, NULL, 1, 1, 9),
             (1, 'Dune/new.mkv', 1, 1000, 1, NULL, NULL, NULL, NULL, 1, 1, NULL),
             (2, 'Show/S01E01.mkv', 1, 300, NULL, 1, 1, 1, NULL, 1, 1, NULL),
             (2, 'Show/S01E01E02.mkv', 1, 400, NULL, 1, 1, 1, 2, 1, 1, NULL),
             (2, 'Show/S01E01.v2.mkv', 1, 700, NULL, 1, 1, 1, NULL, 1, 1, NULL),
             (2, 'Show/extra.mkv', 1, 800, NULL, 1, NULL, NULL, NULL, 1, 1, NULL),
             (2, 'Show/S01E03.mkv', 1, 99999999999, NULL, 1, 1, 3, NULL, 1, 1, NULL);",
    )
    .execute(&pool)
    .await
    .unwrap();
    all.run(&pool).await.unwrap();
    let added: Vec<(String, i64)> =
        sqlx::query_as("SELECT rel_path, added_at FROM media_files ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let now = db::unix_now();
    assert_eq!(
        &added[..6],
        [
            ("Dune/old.mkv".to_owned(), 500),
            ("Dune/new.mkv".to_owned(), 500),
            ("Show/S01E01.mkv".to_owned(), 300),
            ("Show/S01E01E02.mkv".to_owned(), 400),
            ("Show/S01E01.v2.mkv".to_owned(), 300),
            ("Show/extra.mkv".to_owned(), 800),
        ]
    );
    assert!((added[6].1 - now).abs() < 60, "never later than now");
}

/// A sign-in while a large first scan holds the database, looking for files that took others'
/// places: it waits its turn and goes through, rather than failing "database is locked".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signing_in_while_a_large_scan_finds_successors_waits_its_turn() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    // A database on disk, opened as the server opens it, so a writer holds it as it does there.
    let data_dir = tempfile::tempdir().unwrap();
    let pool = db::open(data_dir.path()).await.unwrap();
    // A first scan's worth of files, each a movie of its own, all first seen in this scan.
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path, scan_generation)
             VALUES (1, 'movies', 'movies', '/m', 1);
         WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 6000)
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             SELECT i, 1, 'Movie ' || i, 'Movie ' || i, 1 FROM n;
         INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             SELECT 1, rel_path || '/Movie.mkv', 1, 0, id, 1, 1 FROM movies;",
    )
    .execute(&pool)
    .await
    .unwrap();
    let password = crate::auth::generate_password().unwrap();
    crate::auth::create_user(&pool, "alice", password.clone(), false)
        .await
        .unwrap();

    let (holding, held) = tokio::sync::oneshot::channel();
    let scanning = tokio::spawn({
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            // As the scan does first: from here on it holds the database.
            sqlx::query("UPDATE libraries SET last_scan_at = 1 WHERE id = 1")
                .execute(&mut *tx)
                .await
                .unwrap();
            holding.send(()).unwrap();
            super::carry_renamed(&mut tx, 1, (1, 1, None))
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });
    held.await.unwrap();
    let body = serde_json::json!({
        "username": "alice", "password": password, "device_name": "Living room iPad",
    });
    let request = Request::post("/api/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = crate::api::app(pool.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "signing in during the scan"
    );
    scanning.await.unwrap();
}

/// Finding the files that took others' places looks each one's predecessors up by index: a
/// join on the library compares every new file with every file of it, which over a large first
/// scan kept the database locked for longer than anything else waits.
#[tokio::test]
async fn successors_are_looked_up_by_index() {
    let pool = db::open_in_memory().await;
    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "EXPLAIN QUERY PLAN {}",
        super::SUCCESSORS
    )))
    .bind(1)
    .bind(2)
    .bind(None::<&str>)
    .bind(0)
    .fetch_all(&pool)
    .await
    .unwrap();
    let plan: Vec<String> = plan.into_iter().map(|row| row.3).collect();
    let predecessors: Vec<&String> = plan.iter().filter(|step| step.contains(" o ")).collect();
    assert_eq!(predecessors.len(), 2, "{plan:#?}");
    for step in predecessors {
        assert!(
            step.contains("media_files_movie (movie_id=?)")
                || step.contains("media_files_series (series_id=? AND season=? AND episode=?)"),
            "{plan:#?}"
        );
    }
}
