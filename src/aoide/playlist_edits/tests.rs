use serde_json::value::RawValue;

use super::*;

async fn pool_with_user() -> (SqlitePool, i64) {
    let pool = crate::db::open_in_memory().await;
    let user = crate::auth::create_user(&pool, "u", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    (pool, user.id)
}

async fn device_push(pool: &SqlitePool, user: i64, entity: &str, payload: &str) {
    let id: Value = serde_json::from_str::<Value>(payload).unwrap()["id"].clone();
    let op = format!(
        r#"{{"opId":"d-{entity}-{}","entity":"{entity}","entityId":{id},"operation":"upsert","payload":{payload},"createdAt":1}}"#,
        id.as_str().unwrap_or_default()
    );
    let op = crate::aoide::sync::validate(&RawValue::from_string(op).unwrap()).unwrap();
    crate::aoide::sync::push(pool, user, "phone", vec![op], 1)
        .await
        .unwrap();
}

async fn last_payload(pool: &SqlitePool) -> Value {
    let payload: String =
        sqlx::query_scalar("SELECT payload FROM aoide_ops ORDER BY seq DESC LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
    serde_json::from_str(&payload).unwrap()
}

#[tokio::test]
async fn a_rename_keeps_every_field_and_stamps_only_the_name() {
    let (pool, user) = pool_with_user().await;
    device_push(
        &pool,
        user,
        "playlists",
        r#"{"id":"P","name":"Mix","notes":"n","isSmart":0,"sortIndex":"V","imageHash":"abc","futureField":[1],"updatedAt":100,"deleted":0,"originDevice":"phone","fieldUpdatedAt":{"name":100,"notes":90,"imageHash":100}}"#,
    )
    .await;
    let editor = Editor {
        user_id: user,
        device: "token-9",
        now: 50,
    };
    rename(&pool, &editor, "P", Some("Mix 2"), None)
        .await
        .unwrap();
    let row = last_payload(&pool).await;
    // The clock had gone past this server's: the write still comes after what it changes.
    assert_eq!(row["updatedAt"], 101);
    assert_eq!(
        row,
        serde_json::json!({
            "id": "P", "name": "Mix 2", "notes": "n", "isSmart": 0, "sortIndex": "V",
            "imageHash": "abc", "futureField": [1], "updatedAt": 101, "deleted": false,
            "originDevice": "token-9",
            "fieldUpdatedAt": { "name": 101, "notes": 90, "imageHash": 100 },
        })
    );
    // The same name again writes nothing.
    rename(&pool, &editor, "P", Some("Mix 2"), None)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_ops")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn clearing_notes_leaves_them_out_as_a_device_does() {
    let (pool, user) = pool_with_user().await;
    device_push(
        &pool,
        user,
        "playlists",
        r#"{"id":"P","name":"Mix","notes":"n","isSmart":false,"sortIndex":"V","updatedAt":100,"deleted":false,"originDevice":"phone","fieldUpdatedAt":{"name":100,"notes":100}}"#,
    )
    .await;
    let editor = Editor {
        user_id: user,
        device: "token-9",
        now: 1000,
    };
    rename(&pool, &editor, "P", None, Some(None)).await.unwrap();
    let row = last_payload(&pool).await;
    assert!(row.get("notes").is_none(), "{row}");
    assert_eq!(
        row["fieldUpdatedAt"],
        serde_json::json!({ "name": 100, "notes": 100 })
    );
    let held = playlists::visible(&pool, user).await.unwrap();
    assert!(!held[0].row.contains_key("notes"));
    // Already none: nothing more is written.
    rename(&pool, &editor, "P", None, Some(None)).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM aoide_ops")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn no_room_between_two_equal_keys_is_refused() {
    let (pool, user) = pool_with_user().await;
    device_push(
        &pool,
        user,
        "playlists",
        r#"{"id":"P","name":"Mix","isSmart":false,"sortIndex":"V","updatedAt":100,"deleted":false,"originDevice":"phone"}"#,
    )
    .await;
    for (id, position) in [("A", "a0"), ("B", "a0"), ("C", "a5")] {
        device_push(
            &pool,
            user,
            "playlist_items",
            &format!(
                r#"{{"id":"{id}","playlistId":"P","jellyfinId":"1","contentKey":"k","position":"{position}","updatedAt":100,"deleted":false,"originDevice":"phone"}}"#
            ),
        )
        .await;
    }
    let editor = Editor {
        user_id: user,
        device: "token-9",
        now: 1000,
    };
    // C between A and B, whose keys are the same.
    let refused = reorder(&pool, &editor, "P", "C", 1).await;
    assert!(matches!(refused, Err(EditError::NoGap)), "{refused:?}");
    // Before both, there's room.
    reorder(&pool, &editor, "P", "C", 0).await.unwrap();
    let row = last_payload(&pool).await;
    assert!(row["position"].as_str().unwrap() < "a0");
    assert_eq!(row["id"], "C");
}
