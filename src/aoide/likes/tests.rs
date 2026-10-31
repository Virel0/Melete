use super::*;

fn row(id: &str, track: &str, liked: bool, at: i64, device: &str, deleted: bool) -> String {
    format!(
        r#"{{"contentKey":"a|b|c|1","deleted":{deleted},"id":"{id}","jellyfinId":"{track}","liked":{liked},"originDevice":"{device}","updatedAt":{at}}}"#
    )
}

fn folded(ops: &[(&str, &str, String)]) -> Vec<Like> {
    let mut rows = fold(
        Kind::Track,
        ops.iter().map(|(e, o, p)| (*e, *o, p.as_str())),
    );
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

#[test]
fn a_later_write_wins_and_a_tie_goes_to_the_greater_device() {
    let rows = folded(&[
        ("L", "upsert", row("L", "7", true, 100, "phone", false)),
        ("L", "delete", row("L", "7", true, 200, "phone", true)),
        // Older, arriving late: changes nothing.
        ("L", "upsert", row("L", "7", true, 150, "desk", false)),
    ]);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].deleted);
    for order in [["a", "b"], ["b", "a"]] {
        let rows = folded(&[
            (
                "L",
                "upsert",
                row("L", "7", order[0] == "a", 100, order[0], false),
            ),
            (
                "L",
                "upsert",
                row("L", "7", order[1] == "a", 100, order[1], false),
            ),
        ]);
        assert_eq!(rows[0].origin_device, "b", "either order");
    }
}

#[test]
fn two_rows_for_one_track_leave_the_winner() {
    // Two devices liked the same track offline, each minting its own row.
    let rows = folded(&[
        ("A", "upsert", row("A", "7", true, 100, "phone", false)),
        ("B", "upsert", row("B", "7", false, 300, "desk", false)),
        ("C", "upsert", row("C", "7", true, 200, "tablet", false)),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].id.as_str(), rows[0].liked), ("B", false));
}

#[test]
fn rows_a_device_would_refuse_are_skipped() {
    let rows = folded(&[
        // The op's id isn't the row's.
        ("X", "upsert", row("Y", "1", true, 1, "d", false)),
        // A delete that doesn't say so in the row.
        ("D", "delete", row("D", "2", true, 1, "d", false)),
        // A field missing, and one of the wrong type.
        ("M", "upsert", r#"{"id":"M","jellyfinId":"3","liked":true}"#.into()),
        ("T", "upsert", row("T", "4", true, 1, "d", false).replace(r#""liked":true"#, r#""liked":"yes""#)),
        // Snake case, and flags as numbers, read as the same row.
        ("S", "upsert", r#"{"id":"S","jellyfin_id":"5","content_key":"k","liked":1,"updated_at":1,"deleted":0,"origin_device":"d"}"#.into()),
    ]);
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["S"]);
    assert!(rows[0].liked && !rows[0].deleted);
}

#[tokio::test]
async fn liking_writes_the_tracks_own_row_and_unliking_deletes_it() {
    let pool = crate::db::open_in_memory().await;
    let user = crate::auth::create_user(&pool, "u", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Homogenic', 'Björk', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, artist, genres,
             duration_seconds)
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Jóga', 'Björk', '[]', 305.4);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let ops = |pool: SqlitePool| async move {
        sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT device_id, entity_id, operation, payload FROM aoide_ops ORDER BY seq",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    assert!(
        set(&pool, Kind::Track, user.id, "token-1", 7, true, 1000)
            .await
            .unwrap()
    );
    // Liked already: nothing more is written.
    assert!(
        set(&pool, Kind::Track, user.id, "token-1", 7, true, 2000)
            .await
            .unwrap()
    );
    let written = ops(pool.clone()).await;
    assert_eq!(written.len(), 1);
    let (device, entity_id, operation, payload) = &written[0];
    assert_eq!((device.as_str(), operation.as_str()), ("token-1", "upsert"));
    assert_eq!(
        payload,
        &format!(
            r#"{{"contentKey":"bjork|homogenic|joga|305","deleted":false,"id":"{entity_id}","jellyfinId":"7","liked":true,"originDevice":"token-1","updatedAt":1000}}"#
        )
    );
    let liked = liked(&pool, Kind::Track, user.id).await.unwrap();
    assert_eq!(liked.len(), 1);
    assert_eq!(liked[0].0, Some(7));

    assert!(
        set(&pool, Kind::Track, user.id, "token-2", 7, false, 3000)
            .await
            .unwrap()
    );
    let written = ops(pool.clone()).await;
    let (_, again, operation, payload) = &written[1];
    assert_eq!((again, operation.as_str()), (entity_id, "delete"));
    assert!(payload.contains(r#""deleted":true"#) && payload.contains(r#""updatedAt":3000"#));
    assert!(
        self::liked(&pool, Kind::Track, user.id)
            .await
            .unwrap()
            .is_empty()
    );
    // Liked again: the same row comes back.
    set(&pool, Kind::Track, user.id, "token-2", 7, true, 4000)
        .await
        .unwrap();
    let written = ops(pool.clone()).await;
    assert_eq!(&written[2].1, entity_id);
    // A track that isn't in the library can't be liked; unliking it is nothing to do.
    assert!(
        !set(&pool, Kind::Track, user.id, "token-2", 99, true, 5000)
            .await
            .unwrap()
    );
    assert!(
        set(&pool, Kind::Track, user.id, "token-2", 99, false, 5000)
            .await
            .unwrap()
    );
    assert_eq!(ops(pool).await.len(), 3);
}

#[tokio::test]
async fn unliking_leaves_a_dislike_alone() {
    let pool = crate::db::open_in_memory().await;
    let user = crate::auth::create_user(&pool, "u", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let op = format!(
        r#"{{"opId":"o","entity":"likes","entityId":"D","operation":"upsert","payload":{},"createdAt":1}}"#,
        row("D", "7", false, 100, "phone", false)
    );
    let op = super::super::sync::validate(&serde_json::value::RawValue::from_string(op).unwrap())
        .unwrap();
    super::super::sync::push(&pool, user.id, "phone", vec![op], 1)
        .await
        .unwrap();
    assert!(
        set(&pool, Kind::Track, user.id, "web", 7, false, 200)
            .await
            .unwrap()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_ops")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "nothing written");
    assert!(liked(&pool, Kind::Track, user.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_unlike_in_the_same_millisecond_still_counts() {
    let pool = crate::db::open_in_memory().await;
    let user = crate::auth::create_user(&pool, "u", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/m');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
             VALUES (1, 1, 'k', 'Post', 'Björk', 'a', 1);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, genres)
             VALUES (7, 1, 'a/1', 1, 1, 1, 1, 1, 1, 'Isobel', '[]');",
    )
    .execute(&pool)
    .await
    .unwrap();
    set(&pool, Kind::Track, user.id, "web", 7, true, 1000)
        .await
        .unwrap();
    set(&pool, Kind::Track, user.id, "web", 7, false, 1000)
        .await
        .unwrap();
    assert!(liked(&pool, Kind::Track, user.id).await.unwrap().is_empty());
    let rows = rows(&pool, Kind::Track, user.id).await.unwrap();
    assert_eq!((rows[0].updated_at, rows[0].deleted), (1001, true));
}

#[test]
fn album_likes_name_their_album_and_a_track_row_isnt_one() {
    let album = r#"{"albumId":"3","contentKey":"a|b","deleted":false,"id":"A","liked":true,"originDevice":"d","updatedAt":1}"#;
    let rows = fold(
        Kind::Album,
        [
            ("A", "upsert", album),
            ("T", "upsert", row("T", "7", true, 1, "d", false).as_str()),
        ],
    );
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].id.as_str(), rows[0].item_id.as_str()), ("A", "3"));
    assert_eq!(rows[0].to_row(Kind::Album)["albumId"], "3");
    assert!(fold(Kind::Track, [("A", "upsert", album)]).is_empty());
}
