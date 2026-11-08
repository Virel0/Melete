use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::json;

use super::*;
use crate::db;
use crate::library::{LibraryConfig, LibraryKind, sync_config};
use crate::tmdb::testing::{FakeTmdb, Reply};

const SERIES: i64 = 95396;

/// A shows library at `root` with one folder, matched to [`SERIES`], whose entry was fetched
/// `fetched_hours_ago` and has episode 1x6 named `name` with `overview`, aired `aired` days
/// from today (negative in the past). Returns the folder's id.
async fn series_with_episode(
    pool: &SqlitePool,
    root: &Path,
    (name, overview): (Option<&str>, Option<&str>),
    aired: i64,
    fetched_hours_ago: i64,
) -> i64 {
    let config = LibraryConfig {
        name: "shows".into(),
        kind: LibraryKind::Shows,
        path: PathBuf::from(root),
    };
    let library = sync_config(pool, &[config]).await.unwrap().remove(0).id;
    let now = unix_now();
    sqlx::query(
        "INSERT INTO tmdb_series (tmdb_id, name, fetched_at, language)
             VALUES (?1, 'Show', ?2 - ?3 * 3600, 'en-US');
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, overview, air_date)
             VALUES (?1, 1, 6, 6, ?4, ?5, date(?2, 'unixepoch', ?6 || ' days'));
         INSERT INTO series (library_id, rel_path, parsed_title, parsed_year, seen_generation,
                 match_status, match_tmdb_id, match_method, match_attempted_at)
             VALUES (?7, 'Show (2024)', 'Show', 2024, 1, 'matched', ?1, 'search', ?2)",
    )
    .bind(SERIES)
    .bind(now)
    .bind(fetched_hours_ago)
    .bind(name)
    .bind(overview)
    .bind(aired)
    .bind(library)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar("SELECT id FROM series")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Whether the library series of [`series_with_episode`] is due sooner, in `language`.
async fn due(
    language: &str,
    episode: (Option<&str>, Option<&str>),
    aired: i64,
    fetched_hours_ago: i64,
) -> bool {
    let pool = db::open_in_memory().await;
    series_with_episode(
        &pool,
        Path::new("/path/to/shows"),
        episode,
        aired,
        fetched_hours_ago,
    )
    .await;
    let found = with_unfilled(&pool, Among::Library, language, unix_now(), Vec::new())
        .await
        .unwrap();
    !found.is_empty()
}

#[tokio::test]
async fn a_new_episode_with_a_placeholder_makes_its_series_due_sooner() {
    let summary = Some("A summary.");
    for (what, name, aired) in [
        ("TMDB's placeholder", Some("Episode 6"), -1),
        ("no name", None, 0),
        ("airing tomorrow", Some("Episode 6"), 1),
        ("aired 14 days ago", Some("Episode 6"), -14),
    ] {
        assert!(due("en-US", (name, summary), aired, 7).await, "{what}");
    }
    assert!(
        due("en-US", (Some("A Real Title"), None), -2, 7).await,
        "a title but no summary yet"
    );
}

#[tokio::test]
async fn a_real_title_or_an_older_episode_isnt_due_sooner() {
    let summary = Some("A summary.");
    for (what, name, aired, fetched_hours_ago) in [
        ("a real title", Some("A Real Title"), -1, 7),
        (
            "a title with a word of the placeholder",
            Some("Episode Zero"),
            -1,
            7,
        ),
        ("another episode's number", Some("Episode 5"), -1, 7),
        ("the number written otherwise", Some("Episode 06"), -1, 7),
        ("aired 15 days ago", Some("Episode 6"), -15, 7),
        ("airing in 2 days", Some("Episode 6"), 2, 7),
        ("fetched 5 hours ago", Some("Episode 6"), -1, 5),
    ] {
        assert!(
            !due("en-US", (name, summary), aired, fetched_hours_ago).await,
            "{what}"
        );
    }
}

#[tokio::test]
async fn the_placeholder_is_the_one_in_the_metadata_language_or_english() {
    let summary = Some("Un résumé.");
    assert!(due("fr-FR", (Some("Épisode 6"), summary), -1, 7).await);
    assert!(
        due("fr-FR", (Some("Episode 6"), summary), -1, 7).await,
        "an untitled episode gets its English name"
    );
    assert!(!due("en-US", (Some("Épisode 6"), summary), -1, 7).await);
    assert!(!due("fr-FR", (Some("Épisode Zéro"), summary), -1, 7).await);
}

#[tokio::test]
async fn wanted_series_are_due_sooner_too() {
    let pool = db::open_in_memory().await;
    let now = unix_now();
    sqlx::query(
        "INSERT INTO wanted_series (tvdb_id, tmdb_id, title, monitored, series_type, source, added_at)
             VALUES (1, ?1, 'Show', 1, 'standard', 'sonarr', 0);
         INSERT INTO tmdb_series (tmdb_id, name, status, fetched_at)
             VALUES (?1, 'Show', 'Returning Series', ?2 - 7 * 3600);
         INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, overview, air_date)
             VALUES (?1, 1, 6, 6, 'Episode 6', 'o', date(?2, 'unixepoch'))",
    )
    .bind(SERIES)
    .bind(now)
    .execute(&pool)
    .await
    .unwrap();
    let due = with_unfilled(&pool, Among::Wanted, "en-US", now, vec![7, SERIES])
        .await
        .unwrap();
    assert_eq!(due, [7, SERIES], "each once");
    let due = with_unfilled(&pool, Among::Wanted, "en-US", now, Vec::new())
        .await
        .unwrap();
    assert_eq!(due, [SERIES]);
    sqlx::query("UPDATE tmdb_episodes SET name = 'A Real Title'")
        .execute(&pool)
        .await
        .unwrap();
    let due = with_unfilled(&pool, Among::Wanted, "en-US", now, Vec::new())
        .await
        .unwrap();
    assert!(due.is_empty());
}

fn write_old(root: &Path, rel: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"video").unwrap();
    // Changed long before it came in, as a copy keeping its times would be.
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(30 * 86_400))
        .unwrap();
}

/// TMDB with [`SERIES`]'s episode 1x6 titled by now.
fn titled_now() -> FakeTmdb {
    let fake = FakeTmdb::default();
    fake.reply(
        "/tv/95396?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": SERIES, "name": "Show", "seasons": [{ "season_number": 1 }]
        }))],
    )
    .reply(
        "/tv/95396?append_to_response=season/1",
        vec![Reply::ok(json!({ "id": SERIES, "name": "Show",
            "season/1": { "episodes": [
                { "id": 6, "season_number": 1, "episode_number": 6, "name": "A Real Title",
                  "overview": "A summary." }
            ]}
        }))],
    );
    fake
}

#[tokio::test]
async fn a_file_scanned_for_an_untitled_episode_fetches_its_series_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let pool = db::open_in_memory().await;
    // Fetched an hour ago, so not due; aired a month ago, so not new either.
    series_with_episode(&pool, dir.path(), (Some("Episode 6"), None), -30, 1).await;
    let fake = titled_now();
    let tmdb = fake.start().await;
    assert_eq!(
        refresh(&pool, &tmdb).await.unwrap(),
        RefreshReport::default()
    );

    // An import puts the episode's file in, and the scan it asks for finds it.
    write_old(dir.path(), "Show (2024)/Season 01/Show.S01E06.mkv");
    let library = sync_config(
        &pool,
        &[LibraryConfig {
            name: "shows".into(),
            kind: LibraryKind::Shows,
            path: dir.path().into(),
        }],
    )
    .await
    .unwrap()
    .remove(0);
    crate::library::scan(&pool, &library).await.unwrap();
    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(report.refreshed, 1, "{report:?}");
    let name: String = sqlx::query_scalar("SELECT name FROM tmdb_episodes WHERE episode = 6")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "A Real Title");

    // Fetched since the file came in: not again.
    fake.log.lock().unwrap().clear();
    assert_eq!(
        refresh(&pool, &tmdb).await.unwrap(),
        RefreshReport::default()
    );
    assert!(fake.log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_file_for_a_titled_episode_or_from_before_the_fetch_changes_nothing() {
    for (name, came_in_after) in [(Some("A Real Title"), true), (Some("Episode 6"), false)] {
        let pool = db::open_in_memory().await;
        let series =
            series_with_episode(&pool, Path::new("/path/to/shows"), (name, None), -30, 1).await;
        let now = unix_now();
        sqlx::query(
            "INSERT INTO media_files (library_id, rel_path, size, modified_at, series_id, season,
                 episode, first_seen_generation, seen_generation, first_seen_at)
             VALUES (1, 'Show (2024)/Season 01/Show.S01E06.mkv', 1, 0, ?, 1, 6, 1, 1, ?)",
        )
        .bind(series)
        .bind(if came_in_after { now } else { now - 2 * 3600 })
        .execute(&pool)
        .await
        .unwrap();
        let due = with_unfilled(&pool, Among::Library, "en-US", now, Vec::new())
            .await
            .unwrap();
        assert!(due.is_empty(), "{name:?}");
    }
}
