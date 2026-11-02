use serde_json::value::RawValue;

use super::*;
use crate::aoide::playlists;

async fn user(pool: &SqlitePool, name: &str) -> i64 {
    crate::auth::create_user(pool, name, "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap()
        .id
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
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Paranoid Android', 'Radiohead', '[]', 'rec-1', 383.2);
         INSERT INTO requests (id, user_id, kind, mb_recording_id, title, state, created_at, decided_at)
             VALUES (10, {owner}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0),
                    (11, {owner}, 'song', 'rec-2', 'Lucky', 'requested', 0, NULL),
                    (12, {stranger}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0),
                    (13, {friend}, 'song', 'rec-1', 'Paranoid Android', 'approved', 0, 0);
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
        ("g", Some(13), "a6"),
    ];
    for (id, request, position) in items {
        push(
            &pool,
            owner,
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
    assert!(
        a["updatedAt"].as_i64().unwrap() > 20,
        "after everything held"
    );
    assert_eq!(rows["c"]["position"], "a9", "moved, it stays where it went");
    assert_eq!(rows["g"]["jellyfinId"], "7", "a collaborator's own request");
    assert!(!rows.contains_key("b"), "deleted, it stays deleted");
    for waiting in ["d", "e", "f"] {
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
    let states = request_states(&pool, "P", owner, &[10, 11, 12, 13, 99])
        .await
        .unwrap();
    let mut seen: Vec<(i64, &str)> = states.iter().map(|(k, v)| (*k, v.as_str())).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        [(10, "approved"), (11, "requested"), (13, "approved")]
    );
}
