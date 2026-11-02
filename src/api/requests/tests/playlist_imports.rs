//! An import that makes an Aoide playlist, as the apps see it: the import says which, and the
//! playlist shows the songs not in the library yet as pending, with how their requests stand.

use super::*;

#[tokio::test]
async fn an_import_makes_a_playlist_whose_missing_songs_show_pending() {
    use crate::musicbrainz::testing;
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (alice, bob) = (
        token(&pool, "alice", false).await,
        token(&pool, "bob", false).await,
    );
    // Bob may see only some music libraries: Aoide's playlists are closed to him.
    sqlx::query("INSERT INTO user_access (user_id, all_libraries, updated_at) VALUES (2, 0, 0)")
        .execute(&pool)
        .await
        .unwrap();
    let (mb, _) = testing::start(0).await;
    let mb = Arc::new(mb);
    let app = routes(pool.clone(), Some(mb.clone()), Sources::default());
    let csv = "Track Name,Artist Name(s),Track Duration (ms),ISRC\n\
               Paranoid Android,Radiohead,383000,GBAYE9700245\n\
               Karma Police,Radiohead,264000,\n";
    let body = json!({"name": "Road trip", "csv": csv, "make_playlist": true});

    let (status, refused) = send(
        &app,
        "POST",
        "/api/v1/requests/imports",
        &bob,
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, made) = send(&app, "POST", "/api/v1/requests/imports", &alice, Some(body)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{made}");
    let playlist = made["playlist_id"].as_str().unwrap().to_owned();
    assert_eq!(made["in_library"], 0);
    let (_, plain) = send(
        &app,
        "POST",
        "/api/v1/requests/imports",
        &alice,
        Some(json!({"name": "Just asking", "csv": csv})),
    )
    .await;
    assert_eq!(
        plain["playlist_id"],
        Value::Null,
        "no playlist unless asked"
    );

    crate::playlist_import::run(
        &pool,
        &mb,
        &Sources::default(),
        made["id"].as_i64().unwrap(),
    )
    .await
    .unwrap();
    let lists = crate::api::playlists::routes(pool.clone());
    let (status, one) = send(
        &lists,
        "GET",
        &format!("/api/v1/playlists/{playlist}"),
        &alice,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(
        (&one["name"], &one["tracks"]),
        (&json!("Road trip"), &json!(2))
    );
    let asked = &one["items"][0];
    assert_eq!(
        (&asked["track"], &asked["content_key"]),
        (&Value::Null, &Value::Null)
    );
    let request = asked["pending"]["request_id"].as_i64().unwrap();
    assert_eq!(
        asked["pending"],
        json!({"request_id": request, "title": "Paranoid Android", "artist": "Radiohead",
               "album": null, "duration_ms": 383000, "state": "requested", "why": null})
    );
    let unmatched = &one["items"][1]["pending"];
    assert_eq!(
        (&unmatched["request_id"], &unmatched["state"]),
        (&Value::Null, &json!("unmatched"))
    );
    assert!(unmatched["why"].is_string(), "{unmatched}");
}
