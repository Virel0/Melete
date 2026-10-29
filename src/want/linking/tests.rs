use super::*;
use crate::db;

/// A movies library given with a trailing slash, a shows library, and two TMDB movies.
async fn fixture() -> SqlitePool {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES
             (1, 'movies', 'movies', '/m/'), (2, 'shows', 'shows', '/s');
         INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (20, 'Matched', 0),
                                                                    (30, 'Twice', 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn link_of(pool: &SqlitePool, query: &str) -> Option<i64> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(query.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_requested_title_gets_the_folder_its_download_went_into() {
    let pool = fixture().await;
    // Asked for before anything was on disk; imported into new folders, which the scan found
    // (not matched to TMDB: it may be off).
    sqlx::raw_sql(
        "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 10, 'Dune', 1, 'manual', 0);
         INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, source, added_at)
             VALUES (1, 100, 'A Show', 1, 'standard', 'manual', 0);
         INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, destination,
                 added_at, updated_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Dune.2024', 1, '{}', 'imported',
                     '/m/Dune (2024)', 0, 0);
         INSERT INTO downloads (infohash, title, wanted_series_id, season, decision, state,
                 destination, added_at, updated_at)
             VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'A.Show.S01', 1, 1, '{}', 'imported',
                     '/s/A Show (2020)/Season 01', 0, 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (7, 1, 'Dune (2024)', 'Dune', 1), (8, 1, 'Dune (2024) Extras', 'Dune', 1);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (9, 2, 'A Show (2020)', 'A Show', 1), (10, 2, 'A Show', 'A Show', 1);
         INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation)
             VALUES (1, 'Dune (2024)/Dune.2024.1080p.WEB-DL.mkv', 5, 0, 7, 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Before linking, nothing that reads the link sees the file.
    let replaced = crate::upgrades::replaced_by(&pool, Some(1), None, None, "Dune.2024.2160p")
        .await
        .unwrap();
    assert!(replaced.is_empty());

    assert_eq!(link_folders(&pool).await.unwrap(), 2);
    assert_eq!(
        link_of(&pool, "SELECT movie_id FROM wanted_movies").await,
        Some(7)
    );
    assert_eq!(
        link_of(&pool, "SELECT series_id FROM wanted_series").await,
        Some(9)
    );
    // An upgrade now knows the file it would replace, and the title is on disk.
    let replaced = crate::upgrades::replaced_by(&pool, Some(1), None, None, "Dune.2024.2160p")
        .await
        .unwrap();
    assert_eq!(replaced.len(), 1);
    let step = crate::progress::of(&pool, "movie", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(step.step, crate::progress::Step::InLibrary);
    assert_eq!(link_folders(&pool).await.unwrap(), 0, "a link is made once");
}

#[tokio::test]
async fn otherwise_the_one_folder_matched_to_it_is_linked() {
    let pool = fixture().await;
    sqlx::raw_sql(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
                 match_tmdb_id, missing_since) VALUES
             (1, 1, 'Matched', 'Matched', 1, 20, NULL),
             (2, 1, 'Twice A', 'Twice', 1, 30, NULL),
             (3, 1, 'Twice B', 'Twice', 1, 30, NULL),
             (4, 1, 'Gone', 'Gone', 1, NULL, 99),
             (5, 1, 'Old Folder', 'Linked Already', 1, NULL, NULL),
             (6, 1, 'New Folder', 'Linked Already', 1, NULL, NULL);
         INSERT INTO wanted_movies (id, tmdb_id, title, monitored, movie_id, source, added_at)
             VALUES (1, 20, 'Matched', 1, NULL, 'manual', 0),
                    (2, 30, 'Twice', 1, NULL, 'manual', 0),
                    (3, 40, 'Gone', 1, NULL, 'manual', 0),
                    (4, 50, 'Linked Already', 1, 5, 'radarr', 0);
         UPDATE movies SET tmdb_id = 40 WHERE id = 4;
         UPDATE movies SET tmdb_id = 50 WHERE id = 6;",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(link_folders(&pool).await.unwrap(), 1);
    let links: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, movie_id FROM wanted_movies ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        links,
        [(1, Some(1)), (2, None), (3, None), (4, Some(5))],
        "two folders for one title, a folder that's gone, and a link already made stay as they are"
    );
}

#[tokio::test]
async fn the_folder_scan_of_an_import_finds_the_folder_it_links_to() {
    use crate::library::{LibraryConfig, LibraryKind, scan, scan_folder, sync_config};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("Arrival (2016)")).unwrap();
    std::fs::write(root.join("Arrival (2016)/Arrival.mkv"), b"a").unwrap();
    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "movies".into(),
        kind: LibraryKind::Movies,
        path: root.to_path_buf(),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    scan(&pool, &library).await.unwrap();
    // Asked for, then imported into a new folder, as the import scans only that folder.
    let folder = root.join("Dune (2024)");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("Dune.2024.1080p.WEB-DL.mkv"), b"d").unwrap();
    sqlx::query(
        "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 10, 'Dune', 1, 'manual', 0);
         INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, destination,
                 added_at, updated_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Dune.2024', 1, '{}', 'imported',
                     ?, 0, 0);",
    )
    .bind(folder.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    scan_folder(&pool, &library, "Dune (2024)").await.unwrap();
    assert_eq!(link_folders(&pool).await.unwrap(), 1);
    let linked: String = sqlx::query_scalar(
        "SELECT m.rel_path FROM wanted_movies w JOIN movies m ON m.id = w.movie_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, "Dune (2024)");
}
