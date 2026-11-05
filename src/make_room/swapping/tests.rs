use std::sync::Arc;

use super::*;
use crate::config::Secret;
use crate::deluge;
use crate::downloads::Fetcher;
use crate::rss::tests::{indexer, releases, setup, want_dune};

/// Dune in the library as a 40 GB, two-hour copy, wanted; and a film nobody wants.
async fn library(pool: &SqlitePool) -> i64 {
    let dune = want_dune(pool).await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, parsed_year, seen_generation)
             VALUES (5, 1, 'Dune Part Two (2024)', 'Dune Part Two', 2024, 1),
                 (6, 1, 'Unwanted (2001)', 'Unwanted', 2001, 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, movie_id)
             VALUES (50, 1, 'Dune Part Two (2024)/dune.mkv', 40000000000, 1, 1, 1, 5),
                 (60, 1, 'Unwanted (2001)/u.mkv', 1, 1, 1, 1, 6);
         INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
             VALUES (50, 40000000000, 1, 1, '{\"duration_seconds\": 7200}');",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE wanted_movies SET movie_id = 5 WHERE id = ?")
        .bind(dune)
        .execute(pool)
        .await
        .unwrap();
    dune
}

async fn pipeline() -> (Arc<Pipeline>, impl Sized) {
    let (url, fake) = deluge::testing::start().await;
    let pipeline = Arc::new(Pipeline {
        deluge: deluge::Client::new(&url, Secret::for_tests("pw")).unwrap(),
        fetcher: Fetcher::new().unwrap(),
        seedbox: None,
    });
    (pipeline, fake)
}

async fn state(pool: &SqlitePool) -> (String, Option<String>) {
    sqlx::query_as("SELECT state, reason FROM swaps WHERE file_id = 50")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_swap_grabs_a_smaller_release_and_is_done_once_it_replaces_the_copy() {
    let pool = setup().await;
    let dune = library(&pool).await;
    assert_eq!(
        start(&pool, false, 6).await.unwrap(),
        Err(NotStarted::NotWanted)
    );
    assert_eq!(
        start(&pool, false, 99).await.unwrap(),
        Err(NotStarted::NotFound)
    );
    assert_eq!(start(&pool, false, 5).await.unwrap(), Ok(1));
    assert_eq!(
        start(&pool, false, 5).await.unwrap(),
        Err(NotStarted::NothingToSwap),
        "already under way"
    );
    let monitored: bool = sqlx::query_scalar("SELECT monitored FROM wanted_movies WHERE id = ?")
        .bind(dune)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!monitored, "nothing upgrades it back meanwhile");
    let under_way = progress(&pool).await.unwrap();
    assert_eq!(
        (
            under_way[0].kind.as_str(),
            under_way[0].id,
            under_way[0].waiting
        ),
        ("movie", 5, 1)
    );

    // Two 5 GB releases: the 2160p one is above a smaller copy's height.
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[
        (
            "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
            "",
        ),
        ("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.265-FLUX", ""),
    ]);
    let (pipeline, _deluge) = pipeline().await;
    step(&pool, None, &pipeline, &[]).await.unwrap();
    assert_eq!(state(&pool).await.0, "grabbed");
    let grabbed: Vec<String> = sqlx::query_scalar("SELECT title FROM downloads")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        grabbed,
        ["Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.265-FLUX"]
    );
    let replaces: String = sqlx::query_scalar("SELECT replaces FROM downloads")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        replaces.contains("dune.mkv"),
        "it replaces the copy: {replaces}"
    );

    // The upgrade step trashed the copy for it: done.
    sqlx::query("UPDATE media_files SET missing_since = 9 WHERE id = 50")
        .execute(&pool)
        .await
        .unwrap();
    step(&pool, None, &pipeline, &[]).await.unwrap();
    assert_eq!(state(&pool).await, ("done".into(), None));
}

#[tokio::test]
async fn nothing_small_enough_is_said_so_and_a_failed_download_is_searched_again() {
    let pool = setup().await;
    library(&pool).await;
    start(&pool, false, 5).await.unwrap().unwrap();
    let feed = indexer(&pool, "[2000]").await;
    feed.lock().unwrap().0 = releases(&[(
        "Dune.Part.Two.2024.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.Atmos.7.1-FraMeSToR",
        "",
    )]);
    let (pipeline, _deluge) = pipeline().await;
    step(&pool, None, &pipeline, &[]).await.unwrap();
    assert_eq!(state(&pool).await.0, "none_found");

    // Started again with a release that fits: grabbed; its download fails: waiting again.
    start(&pool, false, 5).await.unwrap().unwrap();
    feed.lock().unwrap().0 = releases(&[("Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.265-FLUX", "")]);
    step(&pool, None, &pipeline, &[]).await.unwrap();
    assert_eq!(state(&pool).await.0, "grabbed");
    sqlx::query("UPDATE downloads SET state = 'failed'")
        .execute(&pool)
        .await
        .unwrap();
    settle(&pool).await.unwrap();
    assert_eq!(state(&pool).await.0, "waiting");
}

#[test]
fn a_smaller_copy_is_at_most_three_quarters_of_the_copy_and_4_5_gb_an_hour() {
    assert_eq!(limit(40_000_000_000, Some(7200.0)), 9_000_000_000);
    assert_eq!(limit(8_000_000_000, Some(7200.0)), 6_000_000_000);
    assert_eq!(limit(8_000_000_000, None), 6_000_000_000);
}

#[tokio::test]
async fn a_series_is_swapped_one_episode_file_at_a_time_specials_aside() {
    let pool = setup().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (2, 'anime', 'shows', '/a');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (7, 2, 'Frieren', 'Frieren', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, series_id, season, episode)
             VALUES (71, 2, 'Frieren/S01E01.mkv', 1, 1, 1, 1, 7, 1, 1),
                 (72, 2, 'Frieren/S01E02.mkv', 1, 1, 1, 1, 7, 1, 2),
                 (73, 2, 'Frieren/S00E01.mkv', 1, 1, 1, 1, 7, 0, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    crate::rss::tests::want_frieren(&pool, Some(7)).await;
    assert_eq!(start(&pool, true, 7).await.unwrap(), Ok(2));
    let rows: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT file_id, season, episode FROM swaps ORDER BY file_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, [(71, 1, 1), (72, 1, 2)]);
}
