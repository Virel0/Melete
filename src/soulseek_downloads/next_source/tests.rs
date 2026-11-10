use super::*;
use crate::db;
use crate::slskd::testing;

const REFUSED: &str = "slskd: 01 Airbag.flac: Rejected";

/// OK Computer wanted, and a search that found it on Soulseek from `users`, best first, as
/// results 10, 11 and on; above them, a torrent, and a copy from dave the profile turned down.
async fn fixture(users: &[&str]) -> SqlitePool {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO wanted_artists (id, mb_artist_id, name, monitored, monitor_new, source, added_at)
             VALUES (1, 'mb-a', 'Radiohead', 1, 'all', 'lidarr', 0);
         INSERT INTO wanted_albums (id, wanted_artist_id, mb_release_group_id, title, monitored, source, added_at)
             VALUES (7, 1, 'rg', 'OK Computer', 1, 'lidarr', 0);
         INSERT INTO searches (id, wanted_album_id, searched_at, indexers) VALUES (1, 7, 0, '[]');
         INSERT INTO release_decisions (id, search_id, title, size, seeders, infohash, accepted, decision)
             VALUES (1, 1, 'Radiohead - OK Computer (1997) [FLAC]', 30, 50,
                     'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1,
                     '{\"accepted\":true,\"rank\":9,\"reasons\":[]}');",
    )
    .execute(&pool)
    .await
    .unwrap();
    insert_result(&pool, 2, "dave", 9, false).await;
    for (n, user) in users.iter().enumerate() {
        insert_result(&pool, 10 + n as i64, user, 8 - n as i64, true).await;
    }
    pool
}

async fn insert_result(pool: &SqlitePool, id: i64, user: &str, rank: i64, accepted: bool) {
    let folder = format!("@@m\\{user}{id}\\OK Computer");
    let source = serde_json::json!({
        "username": user,
        "folder": folder,
        "files": [{"filename": format!("{folder}\\01 Airbag.flac"), "size": 30}],
    });
    let decision = serde_json::json!({
        "accepted": accepted,
        "rank": accepted.then_some(rank),
        "reasons": if accepted { vec![] } else { vec!["the profile doesn't allow MP3-320"] },
    });
    sqlx::query(
        "INSERT INTO release_decisions (id, search_id, title, size, accepted, decision, soulseek)
         VALUES (?, 1, 'Radiohead - OK Computer', 30, ?, ?, ?)",
    )
    .bind(id)
    .bind(accepted)
    .bind(decision.to_string())
    .bind(source.to_string())
    .execute(pool)
    .await
    .unwrap();
}

/// Each Soulseek download, oldest first: whom it's from, its state and its message.
async fn downloads(pool: &SqlitePool) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT json_extract(soulseek, '$.username'), state, coalesce(message, '')
         FROM downloads WHERE source = 'soulseek' ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn row(user: &str, state: &str, message: &str) -> (String, String, String) {
    (user.into(), state.into(), message.into())
}

#[test]
fn a_failed_transfer_says_what_happened() {
    assert_eq!(
        cause("Rejected"),
        "a Soulseek user refused the file",
        "as the state reads once the leading \"Completed, \" is gone"
    );
    assert_eq!(cause("TimedOut"), "a Soulseek user's transfer timed out");
    assert_eq!(
        cause("Cancelled"),
        "a Soulseek user's transfer was cancelled"
    );
    assert_eq!(cause("Errored"), "a Soulseek user's transfer failed");
}

#[tokio::test]
async fn a_refused_file_grabs_the_next_source_from_another_user() {
    let pool = fixture(&["alice", "alice", "bob", "carol"]).await;
    let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
    grab(&pool, &slskd, 10).await.unwrap().unwrap();
    seen.lock().unwrap().transfer_state = "Completed, Rejected".into();
    refresh(&pool, &slskd).await.unwrap();

    let note = "trying the next source (2 of 5) after a Soulseek user refused the file";
    assert_eq!(
        downloads(&pool).await,
        [
            row(
                "alice",
                "failed",
                &format!("{REFUSED}; the next source is being tried (2 of 5)")
            ),
            row("bob", "downloading", &format!("{note}: queued with slskd")),
        ],
        "alice's other copy, the torrent and dave's turned-down copy are passed over"
    );

    // The note stays before slskd's word while it downloads, and the album's progress says it.
    seen.lock().unwrap().transfer_state = String::new();
    refresh(&pool, &slskd).await.unwrap();
    let progress = crate::progress::of(&pool, "album", 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        progress.message,
        Some(format!("{note}: queued by the peer (place 3)"))
    );
}

#[tokio::test]
async fn a_search_gets_five_downloads_at_most_and_then_it_says_so() {
    let pool = fixture(&["alice", "bob", "carol", "erin", "frank", "gina"]).await;
    let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
    grab(&pool, &slskd, 10).await.unwrap().unwrap();
    seen.lock().unwrap().transfer_state = "Completed, Rejected".into();
    for _ in 0..6 {
        refresh(&pool, &slskd).await.unwrap();
    }

    let last = format!("{REFUSED}; all 5 tries are used, so it stays failed");
    let next = |n: i64| format!("{REFUSED}; the next source is being tried ({n} of 5)");
    assert_eq!(
        downloads(&pool).await,
        [
            row("alice", "failed", &next(2)),
            row("bob", "failed", &next(3)),
            row("carol", "failed", &next(4)),
            row("erin", "failed", &next(5)),
            row("frank", "failed", &last),
        ]
    );
    let progress = crate::progress::of(&pool, "album", 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        progress.message,
        Some(format!("the last try failed: {last}"))
    );
}

#[tokio::test]
async fn a_queue_slskd_refuses_counts_as_a_try() {
    let pool = fixture(&["alice", "bob", "carol", "erin", "frank", "gina"]).await;
    let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
    grab(&pool, &slskd, 10).await.unwrap().unwrap();
    {
        let mut seen = seen.lock().unwrap();
        seen.transfer_state = "Completed, Rejected".into();
        seen.refuse_enqueue = Some("User appears to be offline".into());
    }
    refresh(&pool, &slskd).await.unwrap();

    let offline = "slskd couldn't queue it: User appears to be offline";
    assert_eq!(
        downloads(&pool).await,
        [
            row(
                "alice",
                "failed",
                &format!("{REFUSED}; all 5 tries are used, so it stays failed")
            ),
            row("bob", "failed", offline),
            row("carol", "failed", offline),
            row("erin", "failed", offline),
            row("frank", "failed", offline),
        ],
        "gina is never tried"
    );
}

#[tokio::test]
async fn with_no_other_user_left_it_stays_failed_and_says_so() {
    let pool = fixture(&["alice", "alice", "bob"]).await;
    // bob failed this album before, from an earlier search.
    sqlx::raw_sql(
        "INSERT INTO downloads (source, soulseek, title, wanted_album_id, decision, state,
             added_at, updated_at)
         VALUES ('soulseek', '{\"username\":\"bob\",\"folder\":\"x\",\"parts\":[]}', 'x', 7, '{}',
                 'failed', 0, 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
    grab(&pool, &slskd, 10).await.unwrap().unwrap();
    seen.lock().unwrap().transfer_state = "Completed, Rejected".into();
    refresh(&pool, &slskd).await.unwrap();

    assert_eq!(
        downloads(&pool).await,
        [
            row("bob", "failed", ""),
            row(
                "alice",
                "failed",
                &format!(
                    "{REFUSED}; no other accepted result from another Soulseek user is left, so it stays failed"
                )
            ),
        ]
    );
}

#[tokio::test]
async fn nothing_is_grabbed_for_an_album_that_needs_nothing_more() {
    for (case, change) in [
        (
            "the album isn't monitored",
            "UPDATE wanted_albums SET monitored = 0",
        ),
        (
            "its artist isn't monitored",
            "UPDATE wanted_artists SET monitored = 0",
        ),
        ("it isn't wanted any more", "DELETE FROM wanted_albums"),
        (
            "another download of it is under way",
            "INSERT INTO downloads (infohash, title, wanted_album_id, decision, state, added_at,
                 updated_at)
             VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'x', 7, '{}', 'downloading', 0, 0)",
        ),
    ] {
        let pool = fixture(&["alice", "bob"]).await;
        let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
        grab(&pool, &slskd, 10).await.unwrap().unwrap();
        sqlx::raw_sql(change).execute(&pool).await.unwrap();
        seen.lock().unwrap().transfer_state = "Completed, Rejected".into();
        refresh(&pool, &slskd).await.unwrap();
        assert_eq!(
            downloads(&pool).await,
            [row("alice", "failed", REFUSED)],
            "{case}"
        );
    }
}

#[tokio::test]
async fn a_song_slskd_lost_gets_the_next_source_too() {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO wanted_songs (id, title, artist, source, added_at)
             VALUES (2, 'Paranoid Android', 'Radiohead', 'manual', 0);
         INSERT INTO searches (id, wanted_song_id, searched_at, indexers) VALUES (1, 2, 0, '[]');
         INSERT INTO release_decisions (id, search_id, title, accepted, decision, soulseek) VALUES
             (5, 1, 'Radiohead - Paranoid Android', 1, '{\"accepted\":true,\"rank\":1,\"reasons\":[]}',
              '{\"username\":\"alice\",\"folder\":\"a\",\"files\":[{\"filename\":\"a\\\\02 Paranoid Android.flac\",\"size\":9}]}'),
             (6, 1, 'Radiohead - Paranoid Android', 1, '{\"accepted\":true,\"rank\":1,\"reasons\":[]}',
              '{\"username\":\"bob\",\"folder\":\"b\",\"files\":[{\"filename\":\"b\\\\02 Paranoid Android.flac\",\"size\":9}]}');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (slskd, seen) = testing::start(serde_json::json!([]), 1).await;
    grab(&pool, &slskd, 5).await.unwrap().unwrap();
    seen.lock().unwrap().forgotten = true;
    refresh(&pool, &slskd).await.unwrap();

    let note = "trying the next source (2 of 5) after slskd lost track of the last one";
    assert_eq!(
        downloads(&pool).await,
        [
            row(
                "alice",
                "failed",
                "slskd no longer knows part of it; the next source is being tried (2 of 5)"
            ),
            row("bob", "downloading", &format!("{note}: queued with slskd")),
        ]
    );
    let progress = crate::progress::of(&pool, "song", 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progress.message, Some(format!("{note}: queued with slskd")));
}
