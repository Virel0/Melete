use serde_json::{Value, json};

use super::*;
use crate::aoide::playlists::{self, Row};
use crate::db;
use crate::musicbrainz::testing::{self, PARANOID_ANDROID};

const PLAYLIST: &str = "Track Name,Artist Name(s),Track Duration (ms),ISRC\n\
                        Paranoid Android,Radiohead,383000,GBAYE9700245\n\
                        Karma Police,Radiohead,264000,\n\
                        Paranoid Android,Radiohead,,\n\
                        Creep,Radiohead,238000,\n";

/// A library with Creep in it, and the track `extra` adds.
async fn library(pool: &SqlitePool, extra: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 4, 'k', 'Pablo Honey', 'Radiohead', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
             seen_generation, album_id, tagged, title, artist, genres, duration_seconds)
             VALUES (2, 4, 'a/2', 1, 1, 1, 1, 1, 1, 'Creep', 'Radiohead', '[]', 238.6);
         {extra}"
    )))
    .execute(pool)
    .await
    .unwrap();
}

/// Paranoid Android arriving: a track tagged with its recording, under a title nothing matches.
fn arrival() -> String {
    format!(
        "INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
             seen_generation, album_id, tagged, title, artist, genres, duration_seconds,
             mb_recording_id)
             VALUES (9, 4, 'a/9', 1, 1, 1, 1, 1, 1, 'Track 09', 'Unknown', '[]', 383.0,
                 '{PARANOID_ANDROID}');"
    )
}

/// Imports `PLAYLIST` into a new playlist and works it through; the importer, the import's
/// id and the playlist's.
async fn import(pool: &SqlitePool) -> (User, i64, String) {
    let alice = crate::auth::testing::user(pool, "alice", false).await;
    let editor = Editor {
        user_id: alice.id,
        device: "token-1",
        now: 1,
    };
    let playlist = playlist_edits::create(pool, &editor, "Road trip", None)
        .await
        .unwrap();
    let tracks = parse(PLAYLIST).unwrap();
    let made = create(pool, &alice, "Road trip", &tracks, Some(&playlist))
        .await
        .unwrap();
    assert_eq!(made.playlist_id.as_deref(), Some(playlist.as_str()));
    let (mb, _) = testing::start(0).await;
    run(pool, &mb, &fulfil::Sources::default(), made.id)
        .await
        .unwrap();
    (alice, made.id, playlist)
}

async fn items(pool: &SqlitePool, user: i64) -> Vec<Row> {
    playlists::visible(pool, user).await.unwrap()[0]
        .items
        .clone()
}

/// What each item holds: its track id, or its pending request (null when none).
fn holds(items: &[Row]) -> Vec<Value> {
    items
        .iter()
        .map(|i| match Pending::of(i) {
            Some(song) => json!({"pending": song.request_id}),
            None => i["jellyfinId"].clone(),
        })
        .collect()
}

#[tokio::test]
async fn an_import_makes_a_playlist_of_every_track_in_order_and_swaps_arrivals_in() {
    let pool = db::open_in_memory().await;
    library(&pool, "").await;
    let (alice, id, playlist) = import(&pool).await;
    let view = get(&pool, id).await.unwrap().unwrap();
    let s = &view.summary;
    assert_eq!(
        (s.asked, s.already, s.unmatched, s.in_library),
        (2, 1, 0, 1)
    );
    assert_eq!(
        s.message.as_deref(),
        Some("2 asked for, 1 in the library already, 1 asked for already")
    );
    let asked = view.items[0].request_id.unwrap();
    // MusicBrainz has no recording that fits Karma Police: asked for by name.
    let by_name = view.items[1].request_id.unwrap();
    assert_eq!(view.items[3].state, "in_library");
    assert_eq!(view.items[3].request_id, None, "not asked for");
    let held = items(&pool, alice.id).await;
    assert_eq!(
        holds(&held),
        [
            json!({"pending": asked}),
            json!({"pending": by_name}),
            json!({"pending": asked}),
            json!("2"),
        ],
        "in the import's order"
    );
    let karma = Pending::of(&held[1]).unwrap();
    assert_eq!(
        (
            karma.title.as_str(),
            karma.artist.as_deref(),
            karma.duration_ms
        ),
        ("Karma Police", Some("Radiohead"), Some(264_000))
    );
    assert_eq!(held[0]["originDevice"], "melete");

    // Cut off by a restart and taken up again, a track handled twice is the same item.
    sqlx::query(
        "UPDATE playlist_imports SET state = 'running', message = NULL, finished_at = NULL
         WHERE id = ?1;
         UPDATE playlist_import_tracks SET state = 'pending', why = NULL, request_id = NULL
         WHERE import_id = ?1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let (mb, _) = testing::start(0).await;
    run(&pool, &mb, &fulfil::Sources::default(), id)
        .await
        .unwrap();
    let places =
        |items: &[Row]| -> Vec<Value> { items.iter().map(|i| i["position"].clone()).collect() };
    let before = items(&pool, alice.id).await;
    assert_eq!(before.len(), 4);

    // The song arrives: both items waiting on it become the track, where they were.
    sqlx::raw_sql(sqlx::AssertSqlSafe(arrival()))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(pending::resolve(&pool, None).await.unwrap(), 2);
    let held = items(&pool, alice.id).await;
    assert_eq!(
        holds(&held),
        [
            json!("9"),
            json!({"pending": by_name}),
            json!("9"),
            json!("2")
        ]
    );
    assert_eq!(places(&held), places(&before));
    assert_eq!(pending::resolve(&pool, Some(&playlist)).await.unwrap(), 0);
}

#[tokio::test]
async fn an_item_deleted_while_an_import_was_cut_off_stays_deleted() {
    let pool = db::open_in_memory().await;
    library(&pool, "").await;
    let (alice, id, playlist) = import(&pool).await;
    let held = items(&pool, alice.id).await;
    let karma = held[1]["id"].as_str().unwrap().to_owned();
    let editor = Editor {
        user_id: alice.id,
        device: "token-1",
        now: now_ms(),
    };
    playlist_edits::remove(&pool, &editor, &playlist, &karma)
        .await
        .unwrap();

    // A restart had cut the import off after Karma Police went in, before that was saved.
    sqlx::query(
        "UPDATE playlist_imports SET state = 'running', message = NULL, finished_at = NULL
         WHERE id = ?1;
         UPDATE playlist_import_tracks SET state = 'pending', why = NULL, request_id = NULL
         WHERE import_id = ?1 AND position >= 2",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let (mb, _) = testing::start(0).await;
    run(&pool, &mb, &fulfil::Sources::default(), id)
        .await
        .unwrap();
    let after = items(&pool, alice.id).await;
    assert_eq!(after.len(), 3);
    assert!(after.iter().all(|i| i["id"] != karma.as_str()));
}

#[tokio::test]
async fn a_song_the_library_has_under_another_name_goes_in_as_its_track_at_once() {
    let pool = db::open_in_memory().await;
    library(&pool, &arrival()).await;
    let (alice, _, _) = import(&pool).await;
    // Karma Police, which MusicBrainz can't tell, waits on its request by name.
    let karma: i64 = sqlx::query_scalar("SELECT id FROM requests WHERE title = 'Karma Police'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        holds(&items(&pool, alice.id).await),
        [
            json!("9"),
            json!({"pending": karma}),
            json!("9"),
            json!("2")
        ]
    );
}

#[tokio::test]
async fn an_import_without_a_playlist_leaves_out_what_the_library_has_as_its_importer_sees_it() {
    let pool = db::open_in_memory().await;
    // Creep is in the music library; a second music library holds nothing.
    library(
        &pool,
        "INSERT INTO libraries (id, name, kind, path) VALUES (5, 'more', 'music', '/more');",
    )
    .await;
    let (mb, _) = testing::start(0).await;
    let sources = fulfil::Sources::default();
    let csv = "Track Name,Artist Name(s),Track Duration (ms)\nCreep,Radiohead,238000\n";
    let alice = crate::auth::testing::user(&pool, "alice", false).await;
    let made = create(&pool, &alice, "Mix", &parse(csv).unwrap(), None)
        .await
        .unwrap();
    run(&pool, &mb, &sources, made.id).await.unwrap();
    let view = get(&pool, made.id).await.unwrap().unwrap();
    assert_eq!(
        (view.summary.in_library, view.summary.asked),
        (1, 0),
        "in the library already, so not asked for"
    );
    assert_eq!(view.items[0].state, "in_library");
    assert_eq!(view.items[0].request_id, None);
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(requests, 0);

    // Someone who may see only the other library: Creep is asked for, as one the library lacks
    // would be, so nothing tells them what the hidden one holds.
    let bob = crate::auth::testing::user(&pool, "bob", false).await;
    let only_more = crate::access::Restriction {
        libraries: Some(vec![5]),
        max_age: None,
    };
    crate::access::set_restriction(&pool, bob.id, Some(&only_more))
        .await
        .unwrap();
    let made = create(&pool, &bob, "Mix", &parse(csv).unwrap(), None)
        .await
        .unwrap();
    run(&pool, &mb, &sources, made.id).await.unwrap();
    let view = get(&pool, made.id).await.unwrap().unwrap();
    assert_eq!(view.summary.in_library, 0);
    assert_ne!(view.items[0].state, "in_library", "{:?}", view.items[0]);
}

#[tokio::test]
async fn a_track_left_unmatched_before_is_asked_for_by_name_in_its_place() {
    let pool = db::open_in_memory().await;
    library(&pool, "").await;
    let alice = crate::auth::testing::user(&pool, "alice", false).await;
    let editor = Editor {
        user_id: alice.id,
        device: "token-1",
        now: 1,
    };
    let playlist = playlist_edits::create(&pool, &editor, "Road trip", None)
        .await
        .unwrap();
    // As an import before asking by name left it: unmatched, its item pending on nothing.
    let csv = "Track Name,Artist Name(s)\nCreep,Radiohead\nKarma Police,\n";
    let made = create(
        &pool,
        &alice,
        "Road trip",
        &parse(csv).unwrap(),
        Some(&playlist),
    )
    .await
    .unwrap();
    let (mb, _) = testing::start(0).await;
    run(&pool, &mb, &fulfil::Sources::default(), made.id)
        .await
        .unwrap();
    let before = items(&pool, alice.id).await;
    assert_eq!(holds(&before), [json!("2"), json!({"pending": null})]);

    // What the update does to such tracks (its migration): handled again, here with the artist
    // the earlier import couldn't use.
    sqlx::query(
        "UPDATE playlist_import_tracks SET artist = 'Radiohead' WHERE import_id = ?1 AND position = 2;
         UPDATE playlist_import_tracks SET state = 'pending', why = NULL, request_id = NULL
         WHERE state = 'unmatched';
         UPDATE playlist_imports SET state = 'queued', message = NULL, finished_at = NULL
         WHERE id IN (SELECT import_id FROM playlist_import_tracks WHERE state = 'pending');",
    )
    .bind(made.id)
    .execute(&pool)
    .await
    .unwrap();
    run(&pool, &mb, &fulfil::Sources::default(), made.id)
        .await
        .unwrap();
    let karma: i64 = sqlx::query_scalar("SELECT id FROM requests WHERE title = 'Karma Police'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let after = items(&pool, alice.id).await;
    assert_eq!(
        holds(&after),
        [json!("2"), json!({"pending": karma})],
        "the same item, now waiting on its request, not a second one"
    );
    assert_eq!(after[1]["id"], before[1]["id"]);
    assert_eq!(after[1]["position"], before[1]["position"]);
    let summary = summary(&pool, made.id).await.unwrap().unwrap();
    assert_eq!(
        (
            summary.state.as_str(),
            summary.asked,
            summary.unmatched,
            summary.in_library
        ),
        ("done", 1, 0, 1)
    );
}
