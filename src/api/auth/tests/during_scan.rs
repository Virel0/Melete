//! Signing in while the first scan of a large library writes: the sign-in waits its turn for
//! the database rather than failing with "database is locked".

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, SqliteConnection};

use super::*;
use crate::library::{LibraryConfig, LibraryKind, scan, sync_config};

/// Movies in the library: enough that a scan whose writing grows with the square of the
/// library holds the database far longer than a sign-in waits for it.
const MOVIES: usize = 6000;

/// Whether another connection is writing to the database right now: taking the write lock
/// without waiting fails while someone else holds it.
async fn someone_writes(probe: &mut SqliteConnection) -> bool {
    match sqlx::query("BEGIN IMMEDIATE").execute(&mut *probe).await {
        Ok(_) => {
            sqlx::query("ROLLBACK").execute(&mut *probe).await.unwrap();
            false
        }
        Err(sqlx::Error::Database(e)) if e.message().contains("locked") => true,
        Err(e) => panic!("probing the database: {e}"),
    }
}

/// A connection of its own to the database in `data_dir`, that never waits for a lock.
async fn probe(data_dir: &Path) -> SqliteConnection {
    SqliteConnectOptions::new()
        .filename(data_dir.join("melete.db"))
        .busy_timeout(Duration::ZERO)
        .connect()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signing_in_during_a_large_first_scan_waits_its_turn() {
    let library_dir = tempfile::tempdir().unwrap();
    // Loose files, one per movie: quicker to make than a folder each, and scanned the same.
    for i in 0..MOVIES {
        std::fs::write(
            library_dir.path().join(format!("Movie {i} (2001).mkv")),
            b"video",
        )
        .unwrap();
    }
    // A database on disk, opened as the server opens it, so a writer holds it as it would there.
    let data_dir = tempfile::tempdir().unwrap();
    let pool = db::open(data_dir.path()).await.unwrap();
    auth::create_user(&pool, "alice", "alice-password".into(), false)
        .await
        .unwrap();
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: library_dir.path().to_path_buf(),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    let app = super::super::super::app(pool.clone());
    let mut probe = probe(data_dir.path()).await;

    let scanning = tokio::spawn({
        let pool = pool.clone();
        async move { scan(&pool, &library).await }
    });
    let body = json!({ "username": "alice", "password": "alice-password", "device_name": "Living room iPad" });
    let mut signed_in = 0;
    while !scanning.is_finished() {
        // Each sign-in starts while the scan holds the database.
        if !someone_writes(&mut probe).await {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        }
        let (status, answer) =
            send(&app, "POST", "/api/v1/auth/login", None, Some(body.clone())).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "signing in during the scan: {answer}"
        );
        signed_in += 1;
    }
    probe.close().await.unwrap();
    let report = scanning.await.unwrap().unwrap();
    assert_eq!(report.added, MOVIES);
    assert!(signed_in > 0, "no sign-in started while the scan wrote");
}
