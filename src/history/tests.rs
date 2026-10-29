use super::*;
use crate::rss::tests::{indexer, releases, setup, want_dune};
use crate::search::Target;
use crate::wanted_edit::{self, Kind};

/// Every event of the service, oldest first: (kind, title, release).
async fn events(pool: &SqlitePool, service: Service) -> Vec<(String, String, Option<String>)> {
    let (_, mut page) = list(pool, service, &Filter::default(), 500, 0)
        .await
        .unwrap()
        .unwrap();
    page.reverse();
    page.into_iter()
        .map(|e| (e.kind, e.title, e.release))
        .collect()
}

fn kinds(events: &[(String, String, Option<String>)]) -> Vec<&str> {
    events.iter().map(|e| e.0.as_str()).collect()
}

#[tokio::test]
async fn each_event_is_written_where_it_happens() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    let feed = indexer(&pool, "[2000]").await;
    let release = "Dune.Part.Two.2024.1080p.WEB-DL.DDP5.1.H.264-FLUX";
    feed.lock().unwrap().0 = releases(&[(release, "")]);

    // A search for it, as the Wanted page makes one.
    let client = crate::torznab::Client::new().unwrap();
    let search = crate::search::search(&pool, &client, None, Target::Movie(dune))
        .await
        .unwrap()
        .unwrap();
    // A grab, as the download pipeline records one, then its import; a second one fails.
    let t = crate::db::unix_now();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, added_at,
                 updated_at)
             VALUES (7, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', '{release}', {dune}, '{{}}',
                     'downloading', {t} + 1, {t} + 1),
                    (8, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'Dune.Remux', {dune}, '{{}}',
                     'downloading', {t} + 2, {t} + 2);
         UPDATE downloads SET state = 'transferring', updated_at = {t} + 3 WHERE id = 7;
         UPDATE downloads SET state = 'imported', updated_at = {t} + 4 WHERE id = 7;
         UPDATE downloads SET state = 'imported', updated_at = {t} + 5 WHERE id = 7;
         UPDATE downloads SET state = 'failed', message = 'the seedbox lost it',
                 updated_at = {t} + 6
             WHERE id = 8;
         INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO trashed_files (library_id, rel_path, trash_path, size, download_id,
                 replaced_by, trashed_at)
             VALUES (1, 'Dune/Dune.720p.mkv', '.melete-trash/7/Dune.720p.mkv', 1, 7,
                     'Dune/{release}.mkv', {t} + 7);"
    )))
    .execute(&pool)
    .await
    .unwrap();
    // The failed one blocklisted, then the title taken off the list.
    crate::blocklist::add(&pool, crate::blocklist::From::Download(8))
        .await
        .unwrap()
        .unwrap();
    wanted_edit::remove_by(&pool, Kind::Movie, dune, None)
        .await
        .unwrap()
        .unwrap();

    let all = events(&pool, Service::Movies).await;
    let mut happened = kinds(&all);
    happened.sort_unstable();
    assert_eq!(
        happened,
        [
            "blocklisted",
            "failed",
            "grabbed",
            "grabbed",
            "imported",
            "searched",
            "taken_off",
            "upgraded"
        ],
        "a step that isn't imported or failed, and the same state again, record nothing"
    );
    assert!(all.iter().all(|e| e.1 == "Dune: Part Two"));
    let release_of = |kind: &str| {
        let event = all.iter().find(|e| e.0 == kind).unwrap();
        event.2.clone()
    };
    assert_eq!(release_of("grabbed").as_deref(), Some(release));
    assert_eq!(
        release_of("upgraded").as_deref(),
        Some("Dune/Dune.720p.mkv")
    );
    assert_eq!(release_of("blocklisted").as_deref(), Some("Dune.Remux"));
    let (_, page) = list(&pool, Service::Movies, &Filter::default(), 500, 0)
        .await
        .unwrap()
        .unwrap();
    let searched = page.iter().find(|e| e.kind == "searched").unwrap();
    assert_eq!(
        searched.search_id, None,
        "search {search} went with the title; its event stays"
    );
    assert_eq!(searched.message.as_deref(), Some("1 results"));
    let failed = page.iter().find(|e| e.kind == "failed").unwrap();
    assert_eq!(failed.message.as_deref(), Some("the seedbox lost it"));
    assert!(events(&pool, Service::Series).await.is_empty());
}

#[tokio::test]
async fn an_rss_syncs_searches_are_kept_with_the_sync_instead() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    sqlx::query(
        "INSERT INTO rss_syncs (id, synced_at, indexers, new_releases, grabbed)
             VALUES (1, 0, '[]', 0, '[]');
         INSERT INTO searches (wanted_movie_id, searched_at, indexers, rss_sync_id)
             VALUES (?, 0, '[]', 1);",
    )
    .bind(dune)
    .execute(&pool)
    .await
    .unwrap();
    assert!(events(&pool, Service::Movies).await.is_empty());
}

#[tokio::test]
async fn anime_and_music_have_their_own_history_and_titles_theirs() {
    let pool = setup().await;
    sqlx::raw_sql(
        "INSERT INTO wanted_series (id, tvdb_id, title, monitored, series_type, source, added_at)
             VALUES (1, 10, 'Frieren', 1, 'anime', 'manual', 0),
                    (2, 20, 'Severance', 1, 'standard', 'manual', 0);
         INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source,
                 added_at)
             VALUES (1, 'mb-rh', 'Radiohead', 1, 'all', 'manual', 0);
         INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored,
                 source, added_at)
             VALUES (5, 1, 'rg-ok', 'OK Computer', 1, 'manual', 0);
         INSERT INTO searches (wanted_series_id, season, episode, searched_at, indexers)
             VALUES (1, 1, 5, 10, '[{\"results\": 2}, {\"results\": 3}]'),
                    (2, 1, NULL, 20, '[]'), (2, 2, NULL, 30, '[]');
         INSERT INTO searches (wanted_album_id, searched_at, indexers) VALUES (5, 40, '[]');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let anime = events(&pool, Service::Anime).await;
    assert_eq!(anime.len(), 1);
    let (_, page) = list(&pool, Service::Anime, &Filter::default(), 10, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (page[0].season, page[0].episode, page[0].message.as_deref()),
        (Some(1), Some(5), Some("5 results"))
    );
    assert_eq!(events(&pool, Service::Series).await.len(), 2);
    assert_eq!(
        events(&pool, Service::Music).await[0].1,
        "OK Computer by Radiohead"
    );

    // One title's own, by its wanted id; a title of another service isn't found.
    let mine = Filter {
        wanted_id: Some(1),
        ..Filter::default()
    };
    let (total, _) = list(&pool, Service::Anime, &mine, 10, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(
        list(&pool, Service::Series, &mine, 10, 0).await.unwrap(),
        None
    );
    // The artist's history holds its albums'.
    let (total, _) = list(&pool, Service::Music, &mine, 10, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 1);

    // Past searches, newest first; paging and kinds.
    let past = searches(&pool, Service::Series, 2).await.unwrap().unwrap();
    assert_eq!(
        past.iter().map(|s| s.season).collect::<Vec<_>>(),
        [Some(2), Some(1)]
    );
    let albums = searches(&pool, Service::Music, 1).await.unwrap().unwrap();
    assert_eq!(albums[0].album.as_deref(), Some("OK Computer"));
    let (total, page) = list(&pool, Service::Series, &Filter::default(), 1, 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((total, page.len(), page[0].season), (2, 1, Some(1)));
    let grabbed = Filter {
        kind: Some("grabbed".into()),
        ..Filter::default()
    };
    let (total, _) = list(&pool, Service::Series, &grabbed, 10, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(total, 0);
}

#[tokio::test]
async fn events_are_kept_a_year() {
    let pool = setup().await;
    let dune = want_dune(&pool).await;
    let year = 365 * 24 * 3600;
    let now = crate::db::unix_now();
    for at in [now - year - 10, now - year + 10000] {
        sqlx::query(
            "INSERT INTO searches (wanted_movie_id, searched_at, indexers) VALUES (?, ?, '[]')",
        )
        .bind(dune)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO searches (wanted_movie_id, searched_at, indexers) VALUES (?, ?, '[]')",
    )
    .bind(dune)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    let (total, _) = list(&pool, Service::Movies, &Filter::default(), 10, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        total, 2,
        "the one older than a year went when a newer one came"
    );
}

#[tokio::test]
async fn histories_are_read_through_their_indexes() {
    let pool = setup().await;
    for query in [
        "SELECT id FROM title_events WHERE service = 'movies' AND (NULL IS NULL OR key = NULL)
             AND ('grabbed' IS NULL OR kind = 'grabbed') ORDER BY at DESC, id DESC LIMIT 100",
        "SELECT id FROM title_events WHERE service = 'movies' AND key = '1'
             ORDER BY at DESC, id DESC LIMIT 100",
        "DELETE FROM title_events WHERE at < 0",
    ] {
        let plan: Vec<(i64, i64, i64, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(format!("EXPLAIN QUERY PLAN {query}")))
                .fetch_all(&pool)
                .await
                .unwrap();
        let plan: Vec<String> = plan.into_iter().map(|r| r.3).collect();
        assert!(
            plan.iter().any(|s| s.contains("INDEX title_events_by")),
            "{query}: {plan:?}"
        );
        assert!(
            !plan.iter().any(|s| s.starts_with("SCAN title_events")),
            "{plan:?}"
        );
    }
}

#[tokio::test]
async fn a_scan_records_extras_found_and_files_renamed_in_a_wanted_titles_folder() {
    use crate::library::{LibraryConfig, LibraryKind, scan, sync_config};

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("Show")).unwrap();
    std::fs::write(root.join("Show/S01E01.mkv"), b"pilot").unwrap();
    std::fs::create_dir_all(root.join("Unwanted")).unwrap();
    std::fs::write(root.join("Unwanted/S01E01.mkv"), b"other").unwrap();
    let pool = crate::db::open_in_memory().await;
    let config = LibraryConfig {
        name: "shows".into(),
        kind: LibraryKind::Shows,
        path: root.to_path_buf(),
    };
    let library = sync_config(&pool, &[config]).await.unwrap().remove(0);
    scan(&pool, &library).await.unwrap();
    sqlx::query(
        "INSERT INTO wanted_series (tvdb_id, title, monitored, series_type, series_id, source,
                 added_at)
         SELECT 10, 'Show', 1, 'standard', id, 'manual', 0 FROM series WHERE rel_path = 'Show'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // A trailer comes, and the episode is renamed; the same happens beside a title nobody
    // wants, which has no history.
    for folder in ["Show", "Unwanted"] {
        std::fs::write(root.join(format!("{folder}/{folder}-trailer.mkv")), b"t").unwrap();
        std::fs::create_dir_all(root.join(format!("{folder}/Season 1"))).unwrap();
        std::fs::rename(
            root.join(format!("{folder}/S01E01.mkv")),
            root.join(format!("{folder}/Season 1/S01E01 - Pilot.mkv")),
        )
        .unwrap();
    }
    scan(&pool, &library).await.unwrap();

    let (_, page) = list(&pool, Service::Series, &Filter::default(), 10, 0)
        .await
        .unwrap()
        .unwrap();
    let mut seen: Vec<(String, Option<String>, Option<String>)> = page
        .into_iter()
        .map(|e| (e.kind, e.release, e.message))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            (
                "extra_found".into(),
                Some("Show/Show-trailer.mkv".into()),
                Some("trailer".into())
            ),
            (
                "renamed".into(),
                Some("Show/Season 1/S01E01 - Pilot.mkv".into()),
                Some("took the place of Show/S01E01.mkv".into())
            ),
        ]
    );
}
