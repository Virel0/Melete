use super::*;
use crate::library::{Library, LibraryKind};

async fn known(pool: &sqlx::SqlitePool, dir: &Path) {
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'slow', 'movies', ?)")
        .bind(dir.to_string_lossy().into_owned())
        .execute(pool)
        .await
        .unwrap();
    let library = Library {
        id: 1,
        name: "slow".into(),
        kind: LibraryKind::Movies,
        path: dir.to_path_buf(),
    };
    super::super::load(pool, &[library]).await.unwrap();
    sqlx::query("CREATE TABLE settled (n INTEGER NOT NULL)")
        .execute(pool)
        .await
        .unwrap();
}

async fn count(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM settled")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn slow_file_work_holds_up_neither_the_caller_nor_the_database_and_is_recorded_later() {
    // One connection: a write waiting on it would show.
    let pool = crate::db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    known(&pool, dir.path()).await;
    let started = std::time::Instant::now();
    let record = pool.clone();
    let settled = settle(
        &dir.path().join("Film/f.mkv"),
        Duration::from_millis(100),
        || {
            std::thread::sleep(Duration::from_millis(1000));
            5
        },
        move |n: i64| async move {
            sqlx::query("INSERT INTO settled (n) VALUES (?)")
                .bind(n)
                .execute(&record)
                .await
                .unwrap();
            n
        },
    )
    .await;
    assert!(
        matches!(&settled, Settled::StillOut(down) if down.name == "slow"),
        "{settled:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(800));
    tokio::time::timeout(
        Duration::from_millis(300),
        sqlx::query("UPDATE libraries SET name = name").execute(&pool),
    )
    .await
    .expect("nothing holds the database while the file work is out")
    .unwrap();
    // Its library counts as not responding now: more work there isn't started.
    let refused = settle(
        dir.path(),
        Duration::from_secs(1),
        || 1,
        |n: i64| async move { n },
    )
    .await;
    assert!(matches!(refused, Settled::Refused(_)), "{refused:?}");
    assert_eq!(count(&pool).await, 0);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(count(&pool).await, 1, "recorded once the work came back");
}

#[tokio::test]
async fn file_work_that_fails_is_recorded_as_it_went() {
    let dir = tempfile::tempdir().unwrap();
    let settled = settle(
        dir.path(),
        Duration::from_secs(5),
        || Err::<(), _>("no such file".to_owned()),
        |done| async move { done },
    )
    .await;
    assert!(
        matches!(&settled, Settled::Done(Err(why)) if why == "no such file"),
        "{settled:?}"
    );
}
