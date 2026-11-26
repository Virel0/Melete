use super::*;
use crate::library::LibraryKind;

async fn library(pool: &SqlitePool, id: i64, name: &str, path: &Path) -> Library {
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (?, ?, 'movies', ?)")
        .bind(id)
        .bind(name)
        .bind(path.to_string_lossy().into_owned())
        .execute(pool)
        .await
        .unwrap();
    Library {
        id,
        name: name.into(),
        kind: LibraryKind::Movies,
        path: path.to_path_buf(),
    }
}

fn stuck(_: &Path) -> Result<(), String> {
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}

#[tokio::test]
async fn a_library_that_doesnt_answer_is_marked_until_it_does() {
    let pool = crate::db::open_in_memory().await;
    let full = tempfile::tempdir().unwrap();
    std::fs::write(full.path().join("Film (2020)"), b"").unwrap();
    let empty = tempfile::tempdir().unwrap();
    let libraries = [
        library(&pool, 1, "films", full.path()).await,
        library(&pool, 2, "unmounted", empty.path()).await,
    ];
    load(&pool, &libraries).await.unwrap();

    let changes = check_with(&pool, &libraries, LIMIT, read).await.unwrap();
    let [Change::Stopped(down)] = &changes[..] else {
        panic!("{changes:?}");
    };
    assert_eq!(down.name, "unmounted");
    assert!(down.why.contains("empty"), "{}", down.why);
    assert_eq!(
        down_at(&empty.path().join("Film/f.mkv")).map(|d| d.saying()),
        Some("the unmounted library isn't responding".into())
    );
    assert_eq!(down_at(full.path()), None);
    let stored: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, unreachable_since FROM libraries ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(stored[0].1.is_none() && stored[1].1.is_some(), "{stored:?}");
    assert!(
        findings(&pool)
            .await
            .unwrap()
            .iter()
            .any(|f| f.code == "library_unreachable" && f.id == Some(2))
    );

    // Still down: no new change. Then mounted again: back, and cleared.
    assert!(
        check_with(&pool, &libraries, LIMIT, read)
            .await
            .unwrap()
            .is_empty()
    );
    std::fs::write(empty.path().join("Show"), b"").unwrap();
    let changes = check_with(&pool, &libraries, LIMIT, read).await.unwrap();
    assert!(
        matches!(&changes[..], [Change::Back { name, .. }] if name == "unmounted"),
        "{changes:?}"
    );
    assert_eq!(down_at(empty.path()), None);
    let since: Option<i64> =
        sqlx::query_scalar("SELECT unreachable_since FROM libraries WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(since, None);
}

#[tokio::test]
async fn a_stuck_read_gives_up_in_time_and_isnt_started_again() {
    let pool = crate::db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    let libraries = [library(&pool, 1, "frozen", dir.path()).await];
    load(&pool, &libraries).await.unwrap();
    let started = std::time::Instant::now();
    let changes = check_with(&pool, &libraries, Duration::from_millis(100), stuck)
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        matches!(&changes[..], [Change::Stopped(d)] if d.why.contains("didn't answer")),
        "{changes:?}"
    );
    // The first read is still stuck: nothing else is started on that folder.
    let again = answers(dir.path(), Duration::from_millis(100), stuck).await;
    assert_eq!(
        again,
        Err("an earlier read of its folder still hasn't come back".into())
    );
}

#[tokio::test]
async fn work_on_a_library_not_responding_is_refused_and_work_that_hangs_marks_it() {
    let pool = crate::db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    let libraries = [library(&pool, 1, "slow", dir.path()).await];
    load(&pool, &libraries).await.unwrap();
    let inside = dir.path().join("Film");
    assert_eq!(within(&inside, LIMIT, || 7).await, Ok(7));
    let hung = within(&inside, Duration::from_millis(50), || {
        std::thread::sleep(Duration::from_secs(1));
    })
    .await
    .unwrap_err();
    assert_eq!(hung.name, "slow");
    // Refused now without starting the work.
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = ran.clone();
    assert!(
        within(&inside, LIMIT, move || seen
            .store(true, std::sync::atomic::Ordering::SeqCst))
        .await
        .is_err()
    );
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    // And a wait on it gives up.
    let walk = unless_down(dir.path(), std::future::pending::<()>()).await;
    assert!(walk.unwrap_err().to_string().contains("isn't responding"));
}

#[tokio::test]
async fn one_down_when_melete_stopped_stays_down_until_it_answers() {
    let pool = crate::db::open_in_memory().await;
    let dir = tempfile::tempdir().unwrap();
    let lib = library(&pool, 1, "remembered", dir.path()).await;
    sqlx::query("UPDATE libraries SET unreachable_since = 100, unreachable_why = 'x' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    load(&pool, &[lib]).await.unwrap();
    assert_eq!(down_at(dir.path()).map(|d| d.since), Some(100));
}
