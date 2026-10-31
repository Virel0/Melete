use super::*;
use crate::config::Secret;
use crate::rss::tests::{indexer, releases, setup, want_dune};

fn failure(n: i64) -> Failure {
    Failure {
        at: n,
        download_id: n,
        release: format!("Dune.Part.Two.2024.1080p.WEB-DL-FAKE{n}"),
        reason: format!("it runs 1 h 3{n} m; the film runs 2 h 46 m"),
    }
}

async fn runtime(pool: &SqlitePool, minutes: i64) {
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, title, runtime_minutes, fetched_at)
         VALUES (693134, 'Dune: Part Two', ?1, 0)
         ON CONFLICT (tmdb_id) DO UPDATE SET runtime_minutes = ?1",
    )
    .bind(minutes)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn three_failed_checks_stop_searching_until_an_import_or_an_override() {
    let pool = setup().await;
    let dune = Title::Movie(want_dune(&pool).await);
    runtime(&pool, 166).await;
    for n in 1..=2 {
        record_failure(&pool, dune, failure(n), Some(5520.0))
            .await
            .unwrap();
    }
    assert!(!stopped(&pool, dune).await.unwrap());
    record_failure(&pool, dune, failure(3), Some(5580.0))
        .await
        .unwrap();
    assert!(stopped(&pool, dune).await.unwrap());
    let now = status(&pool, dune).await.unwrap().unwrap();
    assert_eq!((now.failures, now.stopped, now.reasons.len()), (3, true, 3));

    // Admin health names it, with the failures and their reasons.
    let health = crate::admin_health::check(&pool).await.unwrap();
    let movies = health.areas.iter().find(|a| a.area == "movies").unwrap();
    let finding = movies
        .findings
        .iter()
        .find(|f| f.code == "runtime_checks_failed")
        .expect("a problem");
    assert_eq!((finding.level, finding.count), (Level::Problem, 3));
    assert!(
        finding
            .message
            .contains("stopped searching for Dune: Part Two")
            && finding
                .message
                .contains("Dune.Part.Two.2024.1080p.WEB-DL-FAKE3: it runs 1 h 33 m"),
        "{}",
        finding.message
    );

    // An import starts the count again.
    reset(&pool, dune).await.unwrap();
    assert!(!stopped(&pool, dune).await.unwrap());
    assert_eq!(status(&pool, dune).await.unwrap().unwrap().failures, 0);
}

#[tokio::test]
async fn the_override_checks_against_the_last_held_back_film_until_tmdb_changes() {
    let pool = setup().await;
    let dune = Title::Movie(want_dune(&pool).await);
    runtime(&pool, 166).await;
    assert_eq!(override_for(&pool, dune).await.unwrap(), None);
    for n in 1..=3 {
        record_failure(&pool, dune, failure(n), Some(5500.0 + n as f64))
            .await
            .unwrap();
    }
    let said = say_right(&pool, dune).await.unwrap().unwrap();
    assert_eq!(
        (
            said.failures,
            said.stopped,
            said.overridden,
            said.override_seconds
        ),
        (0, false, true, Some(5503.0)),
        "the count starts again; the last held-back film's running time is used"
    );
    assert_eq!(override_for(&pool, dune).await.unwrap(), Some(Some(5503.0)));
    // TMDB's runtime changes: the override no longer holds.
    runtime(&pool, 95).await;
    assert_eq!(override_for(&pool, dune).await.unwrap(), None);
    assert!(!status(&pool, dune).await.unwrap().unwrap().overridden);
    assert_eq!(say_right(&pool, Title::Movie(999)).await.unwrap(), None);
}

#[tokio::test]
async fn a_stopped_title_isnt_grabbed_by_an_rss_sync() {
    let pool = setup().await;
    let dune = Title::Movie(want_dune(&pool).await);
    for n in 1..=3 {
        record_failure(&pool, dune, failure(n), None).await.unwrap();
    }
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(
        "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
        "",
    )]);
    let (url, deluge) = crate::deluge::testing::start().await;
    let pipeline = crate::downloads::Pipeline {
        deluge: crate::deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: crate::downloads::Fetcher::new().unwrap(),
        seedbox: None,
    };
    let client = crate::torznab::Client::new().unwrap();
    let view = crate::rss::sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.searches.len(), 1, "still decided on");
    assert!(view.grabbed.is_empty(), "{:?}", view.grabbed);
    assert!(deluge.lock().unwrap().torrents.is_empty());

    // Once an admin says the runtime is right, it's grabbed again.
    say_right(&pool, dune).await.unwrap();
    feed.lock().unwrap().0 = releases(&[(
        "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-OTHER",
        "",
    )]);
    let view = crate::rss::sync(&pool, &client, None, Some(&pipeline))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.grabbed.len(), 1, "{:?}", view.grabbed);
}
