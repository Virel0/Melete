use std::sync::Arc;

use super::*;
use crate::db;

/// Now: the searches a run makes are stamped with the clock.
static NOW: std::sync::LazyLock<i64> = std::sync::LazyLock::new(unix_now);

/// Paranoid Android wanted (song 2), searched for on Soulseek `ago` seconds before *NOW.
async fn song(ago: i64) -> SqlitePool {
    let pool = db::open_in_memory().await;
    let preset = crate::music_profiles::lossless(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (3, 'music', 'music', '/music');
         INSERT INTO wanted_songs (id, title, artist, library_id, music_profile_id, source, added_at)
             VALUES (2, 'Paranoid Android', 'Radiohead', 3, ?1, 'manual', 0);
         INSERT INTO searches (id, wanted_song_id, searched_at, indexers) VALUES (1, 2, ?2, '[]');",
    )
    .bind(preset)
    .bind(*NOW - ago)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

/// A Soulseek download of song 2 from `user`, failed `ago` seconds before *NOW.
async fn failed_from(pool: &SqlitePool, id: i64, user: &str, ago: i64) {
    let soulseek = serde_json::json!({"username": user, "folder": "f", "parts": []});
    sqlx::query(
        "INSERT INTO downloads (id, source, soulseek, title, wanted_song_id, decision, state,
             added_at, updated_at) VALUES (?, 'soulseek', ?, 'x', 2, '{}', 'failed', 0, ?)",
    )
    .bind(id)
    .bind(soulseek.to_string())
    .bind(*NOW - ago)
    .execute(pool)
    .await
    .unwrap();
}

fn flac(user: &str) -> serde_json::Value {
    serde_json::json!({"username": user, "hasFreeUploadSlot": true, "files": [{
        "filename": format!("@@{user}\\Radiohead\\OK Computer\\02 Paranoid Android.flac"),
        "size": 10, "bitDepth": 16,
    }]})
}

async fn sources(users: &[&str]) -> (Sources, Arc<std::sync::Mutex<crate::slskd::testing::Seen>>) {
    let answers: Vec<_> = users.iter().map(|u| flac(u)).collect();
    let (slskd, seen) = crate::slskd::testing::start(serde_json::json!(answers), 1).await;
    let sources = Sources {
        slskd: Some(Arc::new(slskd)),
        pipeline: None,
    };
    (sources, seen)
}

/// The users of the song's downloads under way.
async fn grabbed_from(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT json_extract(soulseek, '$.username') FROM downloads
         WHERE wanted_song_id = 2 AND state <> 'failed' ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_song_soulseek_couldnt_give_is_searched_again_a_day_on_avoiding_who_failed() {
    let pool = song(AFTER + 1).await;
    // alice refused it a day ago; she still lists it, and so does bob now.
    failed_from(&pool, 5, "alice", AFTER + 1).await;
    let (sources, seen) = sources(&["alice", "bob"]).await;
    let searched = run(&pool, &sources, *NOW).await.unwrap();
    assert_eq!(searched, [Again::Song(2)]);
    assert_eq!(seen.lock().unwrap().texts, ["Radiohead Paranoid Android"]);
    assert_eq!(grabbed_from(&pool).await, ["bob"], "not alice again");
    // On its way now: not searched for again.
    assert!(
        run(&pool, &sources, *NOW + 2 * AFTER)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_song_is_searched_again_a_day_apart_three_times_at_most() {
    let pool = song(AFTER + 1).await;
    // Soulseek has nothing for it, every time.
    let (sources, seen) = sources(&[]).await;
    // From the clock now, not NOW: the searches the runs make are stamped with it, and NOW may
    // be from long before this test began, when other tests in the run first read it.
    let mut at = unix_now();
    for time in 1..=TIMES {
        assert_eq!(
            run(&pool, &sources, at).await.unwrap(),
            [Again::Song(2)],
            "time {time}"
        );
        // Not again within a day of that search.
        assert!(
            run(&pool, &sources, at + AFTER - 1)
                .await
                .unwrap()
                .is_empty()
        );
        at += AFTER + 60;
    }
    assert!(
        run(&pool, &sources, at).await.unwrap().is_empty(),
        "three times at most"
    );
    assert_eq!(seen.lock().unwrap().texts.len(), TIMES as usize);
}

#[tokio::test]
async fn nothing_is_searched_again_too_soon_or_while_something_is_on_its_way() {
    // Searched an hour ago.
    let pool = song(3600).await;
    let (sources, _) = sources(&["bob"]).await;
    assert!(run(&pool, &sources, *NOW).await.unwrap().is_empty());
    // Searched a day ago, but a download failed an hour ago (its next source may follow).
    let pool = song(AFTER + 1).await;
    failed_from(&pool, 5, "alice", 3600).await;
    assert!(run(&pool, &sources, *NOW).await.unwrap().is_empty());
    // A download of it on its way.
    sqlx::query("UPDATE downloads SET state = 'downloading', updated_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    assert!(run(&pool, &sources, *NOW).await.unwrap().is_empty());
    // Without slskd, nothing.
    let pool = song(AFTER + 1).await;
    let none = Sources {
        slskd: None,
        pipeline: None,
    };
    assert!(run(&pool, &none, *NOW).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_album_whose_soulseek_downloads_failed_is_searched_again_and_one_never_had_isnt() {
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
         INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
             VALUES (7, 1, 'rg-7', 'OK Computer', 1, 'lidarr', 0),
                    (8, 1, 'rg-8', 'Kid A', 1, 'lidarr', 0);
         INSERT INTO searches (wanted_album_id, searched_at, indexers) VALUES (7, ?1, '[]'), (8, ?1, '[]');
         INSERT INTO downloads (id, source, soulseek, title, wanted_album_id, decision, state,
             added_at, updated_at)
             VALUES (5, 'soulseek', '{\"username\":\"alice\"}', 'x', 7, '{}', 'failed', 0, ?1);",
    )
    .bind(*NOW - AFTER - 1)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(due_albums(&pool, *NOW).await.unwrap(), [Again::Album(7)]);
    // Not while it isn't monitored.
    sqlx::query("UPDATE wanted_albums SET monitored = 0 WHERE id = 7")
        .execute(&pool)
        .await
        .unwrap();
    assert!(due_albums(&pool, *NOW).await.unwrap().is_empty());
}
