use serde_json::{Value, json};

use super::*;
use crate::config::Secret;
use crate::db;
use crate::deluge::testing;

const MINE: &str = "a11bae70b2fdd9eb95b63e746f8594dce2650b0c";
const THEIRS: &str = "b11bae70b2fdd9eb95b63e746f8594dce2650b0c";
const COPYING: &str = "c11bae70b2fdd9eb95b63e746f8594dce2650b0c";
const GONE: &str = "d11bae70b2fdd9eb95b63e746f8594dce2650b0c";

/// A wanted movie with four downloads: 1 imported and added by Melete, 2 imported but in Deluge
/// before Melete grabbed it, 3 added by Melete and still being copied home, 4 imported and added
/// by Melete but no longer in Deluge. Deluge has the first three, seeding.
async fn rig() -> (SqlitePool, deluge::Client, testing::Shared) {
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 438631, 'Dune', 1, 'manual', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO downloads (id, infohash, title, wanted_movie_id, decision, state, added_here,
                 added_at, updated_at)
         VALUES (1, ?1, 'Dune.2021.1080p', 1, '{}', 'imported', 1, 0, 0),
                (2, ?2, 'Dune.2021.720p', 1, '{}', 'imported', 0, 0, 0),
                (3, ?3, 'Dune.2021.2160p', 1, '{}', 'transferring', 1, 0, 0),
                (4, ?4, 'Dune.2021.REPACK', 1, '{}', 'imported', 1, 0, 0)",
    )
    .bind(MINE)
    .bind(THEIRS)
    .bind(COPYING)
    .bind(GONE)
    .execute(&pool)
    .await
    .unwrap();
    let (url, fake) = testing::start().await;
    for hash in [MINE, THEIRS, COPYING] {
        fake.lock()
            .unwrap()
            .torrents
            .insert(hash.into(), seeding(0.0, 0));
    }
    let client = deluge::Client::new(&url, Secret::for_tests("pw")).unwrap();
    (pool, client, fake)
}

fn seeding(ratio: f64, seeding_time: i64) -> Value {
    let mut status = testing::status("Dune.2021", "Seeding", 100.0, true);
    status["ratio"] = json!(ratio);
    status["seeding_time"] = json!(seeding_time);
    status
}

async fn set(pool: &SqlitePool, name: &str, value: &str) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES (?, ?, 0)
         ON CONFLICT (name) DO UPDATE SET value = excluded.value",
    )
    .bind(name)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

async fn removed(pool: &SqlitePool) -> Vec<(i64, Option<String>)> {
    sqlx::query_as("SELECT id, deluge_removed FROM downloads ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// What's left in Deluge, and what Deluge deleted the data of.
fn in_deluge(fake: &testing::Shared) -> (Vec<String>, Vec<String>) {
    let fake = fake.lock().unwrap();
    let mut left: Vec<String> = fake.torrents.keys().cloned().collect();
    left.sort();
    (left, fake.data_removed.clone())
}

/// Clears as the settings say, as the follow loop does.
async fn clear(pool: &SqlitePool, deluge: &deluge::Client) -> usize {
    match Clearing::from_settings(pool).await.unwrap() {
        Some(clearing) => clear_imported(pool, deluge, &clearing).await.unwrap(),
        None => 0,
    }
}

#[tokio::test]
async fn imported_torrents_melete_added_go_keeping_their_data_and_say_so() {
    let (pool, deluge, fake) = rig().await;
    // Never, unless turned on.
    assert_eq!(Clearing::from_settings(&pool).await.unwrap(), None);
    set(&pool, REMOVE_SETTING, "never").await;
    assert_eq!(clear(&pool, &deluge).await, 0);
    assert_eq!(in_deluge(&fake).0.len(), 3);

    set(&pool, REMOVE_SETTING, "kept_data").await;
    assert_eq!(clear(&pool, &deluge).await, 1);
    // Someone else's and the one still being copied stay.
    assert_eq!(
        in_deluge(&fake),
        (vec![THEIRS.to_owned(), COPYING.to_owned()], vec![])
    );
    assert_eq!(
        removed(&pool).await,
        [
            (1, Some("kept_data".into())),
            (2, None),
            (3, None),
            (4, None)
        ]
    );
    let events: Vec<(String, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT kind, download_id, message FROM title_events
         WHERE kind = 'removed_from_deluge'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        events,
        [(
            "removed_from_deluge".into(),
            Some(1),
            Some("removed from Deluge; its data stays on the seedbox".into())
        )]
    );

    // Asked again, nothing more is removed; the one Deluge doesn't list is waited out.
    assert_eq!(clear(&pool, &deluge).await, 0);
    let calls = |fake: &testing::Shared| {
        fake.lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| *c == "core.remove_torrent")
            .count()
    };
    assert_eq!(calls(&fake), 1);
    assert_eq!(removed(&pool).await[3], (4, None));
    sqlx::query("UPDATE downloads SET missing_since = missing_since - 31 * 60 WHERE id = 4")
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool, &deluge).await;
    assert_eq!(removed(&pool).await[3], (4, Some("gone".into())));
    assert_eq!(calls(&fake), 1, "nothing to remove for one that's gone");
}

#[tokio::test]
async fn with_its_data_deluge_deletes_the_files_on_the_seedbox() {
    let (pool, deluge, fake) = rig().await;
    set(&pool, REMOVE_SETTING, "with_data").await;
    assert_eq!(clear(&pool, &deluge).await, 1);
    assert_eq!(in_deluge(&fake).1, [MINE]);
    let message: String =
        sqlx::query_scalar("SELECT message FROM title_events WHERE kind = 'removed_from_deluge'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(message, "removed from Deluge with its data");
}

#[tokio::test]
async fn it_waits_for_the_ratio_or_the_time_seeding_asked_for() {
    let (pool, deluge, fake) = rig().await;
    set(&pool, REMOVE_SETTING, "kept_data").await;
    set(&pool, RATIO_SETTING, "1.5").await;
    set(&pool, SEED_HOURS_SETTING, "2").await;
    let mine = |ratio, time| {
        fake.lock()
            .unwrap()
            .torrents
            .insert(MINE.into(), seeding(ratio, time));
    };
    mine(1.2, 3600);
    assert_eq!(clear(&pool, &deluge).await, 0, "neither reached");
    mine(1.5, 3600);
    assert_eq!(clear(&pool, &deluge).await, 1, "the ratio reached");

    // The time alone does it too.
    sqlx::query(
        "UPDATE downloads SET deluge_removed = NULL, deluge_removed_at = NULL WHERE id = 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    mine(0.1, 7199);
    assert_eq!(clear(&pool, &deluge).await, 0);
    mine(0.1, 7200);
    assert_eq!(clear(&pool, &deluge).await, 1);

    // A wait that isn't a number keeps everything, rather than removing too soon.
    set(&pool, RATIO_SETTING, "one and a half").await;
    assert_eq!(Clearing::from_settings(&pool).await.unwrap(), None);
}
