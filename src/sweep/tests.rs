use std::sync::Arc;

use super::*;
use crate::config::Secret;
use crate::deluge;
use crate::downloads::{Fetcher, Pipeline};
use crate::rss::tests::{indexer, releases, setup, want_dune};

async fn pipeline() -> Arc<Pipeline> {
    let (url, _fake) = deluge::testing::start().await;
    Arc::new(Pipeline {
        deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: Fetcher::new().unwrap(),
        seedbox: None,
    })
}

fn sources(pipeline: Option<Arc<Pipeline>>, subtitles: &std::path::Path) -> Sources {
    Sources {
        search: search_missing::Sources {
            tmdb: None,
            slskd: None,
            pipeline,
            pace: Duration::ZERO,
        },
        subtitles: subtitles.to_path_buf(),
        poll: Duration::from_millis(10),
        through_jobs: false,
    }
}

async fn ended(pool: &SqlitePool, id: i64) -> Sweep {
    for _ in 0..1000 {
        let sweep = get(pool, id).await.unwrap().unwrap();
        if sweep.state != "running" {
            return sweep;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("sweep {id} didn't end");
}

const ALL: Asked = Asked {
    missing: true,
    upgrades: true,
    subtitles: true,
};

#[tokio::test]
async fn a_scan_and_search_scans_then_searches_the_librarys_missing_titles() {
    let pool = setup().await;
    let who = crate::auth::testing::user(&pool, "admin", true).await.id;
    let films = tempfile::tempdir().unwrap();
    std::fs::create_dir(films.path().join("Arrival (2016)")).unwrap();
    std::fs::write(films.path().join("Arrival (2016)/Arrival.2016.mkv"), b"x").unwrap();
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', ?)")
        .bind(films.path().to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
    let dune = want_dune(&pool).await;
    sqlx::query("UPDATE wanted_movies SET library_id = 1 WHERE id = ?")
        .bind(dune)
        .execute(&pool)
        .await
        .unwrap();
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX", "")]);
    let kept = tempfile::tempdir().unwrap();

    let started = start(
        &pool,
        sources(Some(pipeline().await), kept.path()),
        "films",
        ALL,
        who,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (started.state.as_str(), started.step.as_str()),
        ("running", "scanning")
    );
    let sweep = ended(&pool, started.id).await;
    assert_eq!(
        (sweep.state.as_str(), sweep.step.as_str()),
        ("done", "done"),
        "{sweep:?}"
    );
    let message = sweep.message.clone().unwrap();
    assert!(
        message.starts_with("films scanned; missing (search "),
        "{message}"
    );
    assert!(message.contains("subtitles: none fetched"), "{message}");
    // The scan found the film on disk.
    let scanned: i64 = sqlx::query_scalar("SELECT count(*) FROM media_files WHERE library_id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(scanned, 1);
    // Its search was for this library, and grabbed Dune.
    let run = search_missing::get(&pool, sweep.missing_search_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (run.library_id, run.grabbed, run.goal.as_str()),
        (Some(1), 1, "missing")
    );
    let upgrades = search_missing::get(&pool, sweep.upgrade_search_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (upgrades.goal.as_str(), upgrades.total),
        ("cutoff_unmet", 0)
    );
    assert_eq!(latest(&pool, "films").await.unwrap(), Some(sweep));
}

#[tokio::test]
async fn a_scan_and_search_is_refused_for_music_a_library_not_responding_or_none() {
    let pool = setup().await;
    let who = crate::auth::testing::user(&pool, "admin", true).await.id;
    let quiet = tempfile::tempdir().unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m'),
             (2, 'quiet', 'shows', '{}')",
        quiet.path().display()
    )))
    .execute(&pool)
    .await
    .unwrap();
    let library = crate::library::Library {
        id: 2,
        name: "quiet".into(),
        kind: LibraryKind::Shows,
        path: quiet.path().to_path_buf(),
    };
    crate::library::health::load(&pool, &[library])
        .await
        .unwrap();
    crate::library::health::mark_at(quiet.path(), "its folder didn't answer within 15 seconds");
    let kept = tempfile::tempdir().unwrap();
    let try_it = |name: &'static str| {
        let (pool, sources) = (pool.clone(), sources(None, kept.path()));
        async move { start(&pool, sources, name, ALL, who).await.unwrap() }
    };
    assert_eq!(try_it("music").await, Err(Refused::Music));
    assert_eq!(try_it("nope").await, Err(Refused::NoSuchLibrary));
    assert_eq!(
        try_it("quiet").await,
        Err(Refused::NotResponding(
            "the quiet library isn't responding".into()
        ))
    );
}
