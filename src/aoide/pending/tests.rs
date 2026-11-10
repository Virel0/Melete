use serde_json::value::RawValue;

use super::*;
use crate::aoide::playlists;

async fn user(pool: &SqlitePool, name: &str) -> i64 {
    crate::auth::testing::user(pool, name, false).await.id
}

async fn push(pool: &SqlitePool, user: i64, op: &str, operation: &str, entity: &str, row: Value) {
    let op = json!({
        "opId": op, "entity": entity, "entityId": row["id"], "operation": operation,
        "payload": row, "createdAt": 1,
    });
    let op = crate::aoide::sync::validate(&RawValue::from_string(op.to_string()).unwrap()).unwrap();
    crate::aoide::sync::push(pool, user, "phone", vec![op], 1)
        .await
        .unwrap();
}

/// A pending item's row on request `request`, at `position`.
fn pending_item(id: &str, request: Option<i64>, position: &str, at: i64) -> Value {
    json!({
        "id": id, "playlistId": "P", "position": position, "updatedAt": at, "deleted": false,
        "originDevice": "phone",
        "pending": {"requestId": request, "title": "Paranoid Android", "artist": "Radiohead",
                    "album": null, "durationMs": 383000, "why": null},
    })
}

async fn ops(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM aoide_ops")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn a_pending_song_reads_back_as_written_and_a_track_is_never_pending() {
    let song = Pending {
        request_id: Some(4),
        title: "Creep".into(),
        artist: Some("Radiohead".into()),
        album: None,
        duration_ms: Some(238_000),
        why: None,
    };
    let mut item = Row::new();
    item.insert("pending".into(), song.to_value());
    assert_eq!(Pending::of(&item), Some(song));
    assert_eq!(
        item["pending"],
        json!({"requestId": 4, "title": "Creep", "artist": "Radiohead", "album": null,
               "durationMs": 238000, "why": null}),
        "every field there, null when unknown"
    );
    item.insert("jellyfinId".into(), "7".into());
    assert_eq!(Pending::of(&item), None, "a track id wins");
    item.insert("jellyfinId".into(), "".into());
    item.insert("pending".into(), json!({"requestId": 4, "title": " "}));
    assert_eq!(Pending::of(&item), None, "no title, nothing to show");
}

#[tokio::test]
async fn an_arrived_song_is_swapped_in_once_and_only_where_it_still_waits() {
    let pool = crate::db::open_in_memory().await;
    let (owner, friend, stranger) = (
        user(&pool, "owner").await,
        user(&pool, "friend").await,
        user(&pool, "stranger").await,
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'OK Computer', 'Radiohead', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
             seen_generation, album_id, tagged, title, artist, genres, mb_recording_id,
             duration_seconds)
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Paranoid Android', 'Radiohead', '[]', 'rec-1', 383.2),
                    (8, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Airbag', 'Radiohead', '[]', 'rec-3', 284.0);
         INSERT INTO requests (id, user_id, kind, mb_recording_id, title, state, created_at, decided_at)
             VALUES (10, {owner}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0),
                    (11, {owner}, 'song', 'rec-2', 'Lucky', 'requested', 0, NULL),
                    (12, {stranger}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0),
                    (13, {friend}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0),
                    (14, {friend}, 'song', 'rec-3', 'Airbag', 'approved', 0, 0);
         INSERT INTO aoide_playlist_shares (playlist_id, grantee_user_id, can_edit, created_at)
             VALUES ('P', {friend}, 1, 0);"
    )))
    .execute(&pool)
    .await
    .unwrap();
    let list = json!({"id": "P", "name": "Mix", "isSmart": false, "sortIndex": "V",
                      "updatedAt": 1, "deleted": false, "originDevice": "phone"});
    push(&pool, owner, "p", "upsert", "playlists", list).await;
    let items = [
        ("a", Some(10), "a0"),
        ("b", Some(10), "a1"),
        ("c", Some(10), "a2"),
        ("d", Some(11), "a3"),
        ("e", None, "a4"),
        ("f", Some(12), "a5"),
        // The friend's own request, put in by the friend.
        ("g", Some(13), "a6"),
        // The friend's request, put in by the owner: it tells the owner nothing.
        ("h", Some(14), "a7"),
    ];
    for (id, request, position) in items {
        let by = if id == "g" { friend } else { owner };
        push(
            &pool,
            by,
            id,
            "upsert",
            "playlist_items",
            pending_item(id, request, position, 10),
        )
        .await;
    }
    // On a device since: b deleted, c moved to the end.
    let mut gone = pending_item("b", Some(10), "a1", 20);
    gone["deleted"] = true.into();
    push(&pool, owner, "b-gone", "delete", "playlist_items", gone).await;
    push(
        &pool,
        owner,
        "c-moved",
        "upsert",
        "playlist_items",
        pending_item("c", Some(10), "a9", 20),
    )
    .await;
    let before = ops(&pool).await;

    assert_eq!(resolve(&pool, None).await.unwrap(), 3, "a, c and g");
    let held = playlists::visible(&pool, owner).await.unwrap();
    let rows: HashMap<&str, &Row> = held[0]
        .items
        .iter()
        .map(|i| (i["id"].as_str().unwrap(), i))
        .collect();
    let a = rows["a"];
    assert_eq!(
        (
            &a["jellyfinId"],
            &a["contentKey"],
            &a["position"],
            &a["originDevice"]
        ),
        (
            &json!("7"),
            &json!("radiohead|ok computer|paranoid android|383"),
            &json!("a0"),
            &json!("melete")
        )
    );
    assert!(!a.contains_key("pending"));
    assert_eq!(a["updatedAt"], 11, "just after its own last write");
    assert_eq!(rows["c"]["position"], "a9", "moved, it stays where it went");
    assert_eq!(rows["c"]["updatedAt"], 21);
    assert_eq!(rows["g"]["jellyfinId"], "7", "a collaborator's own request");
    assert!(!rows.contains_key("b"), "deleted, it stays deleted");
    for waiting in ["d", "e", "f", "h"] {
        assert!(
            Pending::of(rows[waiting]).is_some(),
            "{waiting} still waits"
        );
    }
    assert_eq!(ops(&pool).await, before + 3);

    // Again: nothing left to swap, nothing written.
    assert_eq!(resolve(&pool, None).await.unwrap(), 0);
    assert_eq!(resolve(&pool, Some("P")).await.unwrap(), 0);
    assert_eq!(ops(&pool).await, before + 3);

    // How the requests stand, as the playlist shows them: never someone else's.
    let states = vouched(&pool, "P", &[10, 11, 12, 13, 14, 99])
        .await
        .unwrap();
    let mut seen: Vec<(i64, &str)> = states.iter().map(|(k, v)| (*k, v.as_str())).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        [(10, "approved"), (11, "requested"), (13, "approved")]
    );
}

#[tokio::test]
async fn an_edit_made_before_the_swap_but_sent_after_it_still_wins() {
    let pool = crate::db::open_in_memory().await;
    let owner = user(&pool, "owner").await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'OK Computer', 'Radiohead', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
             seen_generation, album_id, tagged, title, artist, genres, mb_recording_id,
             duration_seconds)
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Paranoid Android', 'Radiohead', '[]', 'rec-1', 383.2);
         INSERT INTO requests (id, user_id, kind, mb_recording_id, title, state, created_at, decided_at)
             VALUES (10, {owner}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0);"
    )))
    .execute(&pool)
    .await
    .unwrap();
    let list = json!({"id": "P", "name": "Mix", "isSmart": false, "sortIndex": "V",
                      "updatedAt": 1, "deleted": false, "originDevice": "phone"});
    push(&pool, owner, "p", "upsert", "playlists", list).await;
    for (id, position) in [("a", "a0"), ("b", "a1")] {
        let item = pending_item(id, Some(10), position, 10);
        push(&pool, owner, id, "upsert", "playlist_items", item).await;
    }
    assert_eq!(resolve(&pool, None).await.unwrap(), 2);

    // A phone offline deleted a and moved b at 15, after they were added but before the swap
    // was written, and sends both now.
    let mut gone = pending_item("a", Some(10), "a0", 15);
    gone["deleted"] = true.into();
    push(&pool, owner, "a-gone", "delete", "playlist_items", gone).await;
    let moved = pending_item("b", Some(10), "a9", 15);
    push(&pool, owner, "b-moved", "upsert", "playlist_items", moved).await;
    let held = playlists::visible(&pool, owner).await.unwrap();
    assert_eq!(held[0].items.len(), 1, "a stays deleted");
    assert_eq!(held[0].items[0]["position"], "a9", "b stays moved");

    // b, pending again as the phone had it, is swapped once more where it went; a isn't.
    assert_eq!(resolve(&pool, None).await.unwrap(), 1);
    let held = playlists::visible(&pool, owner).await.unwrap();
    let b = &held[0].items[0];
    assert_eq!(
        (&b["jellyfinId"], &b["position"]),
        (&json!("7"), &json!("a9"))
    );
    assert_eq!(resolve(&pool, None).await.unwrap(), 0);
}

#[tokio::test]
async fn the_track_a_requests_own_download_brought_fills_its_items_whatever_its_recording() {
    let pool = crate::db::open_in_memory().await;
    let owner = user(&pool, "owner").await;
    let download = |id: i64, song: i64, state: &str, placed: &str| {
        format!(
            "INSERT INTO downloads (id, source, soulseek, title, wanted_song_id, decision, state,
                 placed, added_at, updated_at)
             VALUES ({id}, 'soulseek', '{{\"username\":\"peer{id}\",\"folder\":\"f\",\"parts\":[]}}',
                     't', {song}, '{{}}', '{state}', '{placed}', 0, 0);"
        )
    };
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m/');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'OK Computer', 'Radiohead', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
             seen_generation, album_id, tagged, title, artist, genres, mb_recording_id,
             duration_seconds)
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Paranoid Android', 'Radiohead', '[]',
                     'rec-other', 383.2),
                    (8, 1, 'a/2', 1, 1, 1, 1, 1, 1, 'Lucky', 'Radiohead', '[]', NULL, 259.0),
                    (9, 1, 'b/1', 1, 1, 1, 1, 1, 1, 'Lucky', 'Radiohead', '[]', NULL, 259.0),
                    (6, 1, 'c/1', 1, 1, 1, 1, 1, 1, 'Airbag', 'Radiohead', '[]', NULL, 284.0);
         INSERT INTO wanted_songs (id, title, artist, source, added_at)
             VALUES (2, 'Paranoid Android', 'Radiohead', 'manual', 0),
                    (3, 'Lucky', 'Radiohead', 'manual', 0),
                    (4, 'Airbag', 'Radiohead', 'manual', 0),
                    (5, 'Airbag', 'Radiohead', 'manual', 0);
         {}{}{}{}
         INSERT INTO requests (id, user_id, kind, mb_recording_id, title, state, created_at,
             decided_at, wanted_song_id)
             VALUES (10, {owner}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0, 2),
                    (11, {owner}, 'song', 'rec-2', 'Lucky', 'approved', 0, 0, 3),
                    (12, {owner}, 'song', 'rec-3', 'Airbag', 'approved', 0, 0, 4),
                    (13, {owner}, 'song', 'rec-4', 'Airbag', 'approved', 0, 0, 5);",
        // Tagged as another recording of the same song, and a cover beside it.
        download(1, 2, "imported", r#"["/m/a/1", "/m/a/cover.jpg"]"#),
        // Two downloads of one song that placed a track each: which is it?
        download(2, 3, "imported", r#"["/m/a/2"]"#),
        download(3, 3, "imported", r#"["/m/b/1"]"#),
        // Failed after placing a file: not the song's.
        download(4, 4, "failed", r#"["/m/c/1"]"#),
    )))
    .execute(&pool)
    .await
    .unwrap();
    let list = json!({"id": "P", "name": "Mix", "isSmart": false, "sortIndex": "V",
                      "updatedAt": 1, "deleted": false, "originDevice": "phone"});
    push(&pool, owner, "p", "upsert", "playlists", list).await;
    for (id, request, position) in [
        ("a", 10, "a0"),
        ("b", 11, "a1"),
        ("c", 12, "a2"),
        ("d", 13, "a3"),
    ] {
        let item = pending_item(id, Some(request), position, 10);
        push(&pool, owner, id, "upsert", "playlist_items", item).await;
    }

    assert_eq!(resolve(&pool, None).await.unwrap(), 1, "only a");
    let held = playlists::visible(&pool, owner).await.unwrap();
    let rows: HashMap<&str, &Row> = held[0]
        .items
        .iter()
        .map(|i| (i["id"].as_str().unwrap(), i))
        .collect();
    assert_eq!(rows["a"]["jellyfinId"], "7");
    for waiting in ["b", "c", "d"] {
        assert!(
            Pending::of(rows[waiting]).is_some(),
            "{waiting} still waits"
        );
    }
    // The request says it's in the library, and its playlist item now plays it.
    let progress = crate::progress::of(&pool, "song", 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (progress.step, progress.download_id),
        (crate::progress::Step::InLibrary, Some(1))
    );
}

#[tokio::test]
async fn a_song_fetched_from_youtube_fills_its_items() {
    let pool = crate::db::open_in_memory().await;
    let owner = user(&pool, "owner").await;
    // Asked for by name (no recording), fetched from YouTube into Singles; another only being
    // fetched still.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Singles', 'Duo', 'Duo/Singles', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
             seen_generation, album_id, tagged, title, artist, genres, duration_seconds)
             VALUES (7, 1, 'Duo/Singles/Song.opus', 1, 1, 1, 1, 1, 1, 'Song', 'Duo', '[]', 128.0);
         INSERT INTO wanted_songs (id, title, artist, library_id, source, added_at)
             VALUES (2, 'Song', 'Duo', 1, 'manual', 0), (3, 'Other', 'Duo', 1, 'manual', 0);
         INSERT INTO song_youtube (wanted_song_id, state, video, placed, started_at, updated_at)
             VALUES (2, 'imported', 'aaaaaaaaaaa', 'Duo/Singles/Song.opus', 0, 0),
                    (3, 'downloading', 'bbbbbbbbbbb', NULL, 0, 0);
         INSERT INTO requests (id, user_id, kind, title, state, created_at, decided_at,
             wanted_song_id)
             VALUES (10, {owner}, 'song', 'Song', 'approved', 0, 0, 2),
                    (11, {owner}, 'song', 'Other', 'approved', 0, 0, 3);"
    )))
    .execute(&pool)
    .await
    .unwrap();
    let list = json!({"id": "P", "name": "Mix", "isSmart": false, "sortIndex": "V",
                      "updatedAt": 1, "deleted": false, "originDevice": "phone"});
    push(&pool, owner, "p", "upsert", "playlists", list).await;
    for (id, request, position) in [("a", 10, "a0"), ("b", 11, "a1")] {
        let item = pending_item(id, Some(request), position, 10);
        push(&pool, owner, id, "upsert", "playlist_items", item).await;
    }
    assert_eq!(resolve(&pool, None).await.unwrap(), 1, "only a");
    let held = playlists::visible(&pool, owner).await.unwrap();
    let rows: HashMap<&str, &Row> = held[0]
        .items
        .iter()
        .map(|i| (i["id"].as_str().unwrap(), i))
        .collect();
    assert_eq!(rows["a"]["jellyfinId"], "7");
    assert!(Pending::of(rows["b"]).is_some(), "b still waits");
}
