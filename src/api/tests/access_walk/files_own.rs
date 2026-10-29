//! The walk's checks of what's made from each file beyond the file itself: its seek-bar
//! previews, subtitles fetched for it, and offline copies of it. A hidden title's are there, made as an open one's are; they're asked for directly,
//! and through a link made before the title was hidden.

use super::*;

/// Previews made for the open movie's file (1) and a hidden one's (32), as the job leaves them.
fn previews(w: &World) {
    let index = json!({ "width": 320, "height": 180, "columns": 10, "rows": 10, "sheets": 1,
        "times": [0.0, 10.0] });
    for id in [1, 32] {
        let dir = crate::trickplay::version_dir(&crate::trickplay::folder(w._dir.path()), id, 5, 1);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.json"), index.to_string()).unwrap();
        std::fs::write(dir.join("0.jpg"), b"jpeg").unwrap();
    }
}

#[tokio::test]
async fn a_hidden_files_previews_cant_be_had() {
    let w = world().await;
    sqlx::raw_sql(
        "INSERT INTO media_trickplay (file_id, size, modified_at, made_at) VALUES
             (1, 5, 1, 0), (32, 5, 1, 0);",
    )
    .execute(&w.pool)
    .await
    .unwrap();
    previews(&w);
    assert_eq!(
        ask(&w, "GET", "/api/v1/files/1/trickplay").await.0,
        StatusCode::OK
    );
    assert_eq!(
        ask(&w, "GET", "/api/v1/files/32/trickplay").await.0,
        StatusCode::NOT_FOUND
    );

    // A link made while the title was open stops serving its previews once it's hidden.
    lift(&w).await;
    let (status, play) = ask(&w, "POST", "/api/v1/files/32/play").await;
    assert_eq!(status, StatusCode::OK, "{play}");
    let play: Value = serde_json::from_str(&play).unwrap();
    let sheet = format!("{}/trickplay/0.jpg", play["stream_url"].as_str().unwrap());
    assert_eq!(ask(&w, "GET", &sheet).await.0, StatusCode::OK);
    restrict(
        &w,
        Some(Restriction {
            libraries: Some(vec![1, 2, 5]),
            max_age: Some(12),
        }),
    )
    .await;
    assert_eq!(ask(&w, "GET", &sheet).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_hidden_files_subtitles_cant_be_searched_for_or_had() {
    let w = world().await;
    let search = |id: i64| format!("/api/v1/files/{id}/subtitles/search?language=fr");
    assert_eq!(
        ask(&w, "POST", &search(32)).await.0,
        StatusCode::NOT_FOUND,
        "hidden, before anything else"
    );
    assert_eq!(
        ask(&w, "POST", &search(1)).await.0,
        StatusCode::CONFLICT,
        "open, and OpenSubtitles isn't set up"
    );
    assert_eq!(
        ask(&w, "POST", &search(1_099_511_627_777)).await.0,
        StatusCode::NOT_FOUND,
        "a hidden title's extra"
    );
    assert_eq!(
        ask(&w, "POST", &search(1_099_511_627_776)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an open title's extra, which OpenSubtitles doesn't index"
    );
    // A subtitle fetched for the hidden title while it was open, through a link made then.
    let folder = w._dir.path().join("subtitles/fetched/32-5-1");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("fr.srt"),
        "1\n00:00:01,000 --> 00:00:02,000\nSalut\n",
    )
    .unwrap();
    sqlx::query(
        "INSERT INTO opensubtitles_fetched (file_id, size, modified_at, language, subtitle_file_id,
             hearing_impaired, fetched_at) VALUES (32, 5, 1, 'fr', 1, 0, 0)",
    )
    .execute(&w.pool)
    .await
    .unwrap();
    lift(&w).await;
    let (_, play) = ask(&w, "POST", "/api/v1/files/32/play").await;
    let play: Value = serde_json::from_str(&play).unwrap();
    let track = crate::opensubtitles::track_id("fr");
    let uri = format!(
        "{}/subtitles/{track}.vtt",
        play["stream_url"].as_str().unwrap()
    );
    // Found while open: the walk's files aren't on disk, so it isn't served, but it's there.
    assert_ne!(ask(&w, "GET", &uri).await.0, StatusCode::NOT_FOUND);
    restrict(
        &w,
        Some(Restriction {
            libraries: Some(vec![1, 2, 5]),
            max_age: Some(12),
        }),
    )
    .await;
    assert_eq!(ask(&w, "GET", &uri).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        ask(&w, "GET", "/api/v1/files/32/subtitles").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_hidden_files_offline_copies_cant_be_asked_for_or_seen() {
    let w = world().await;
    // Open: answered (the walk's files aren't on disk to be read, but it's not a 404).
    assert_ne!(
        ask(&w, "GET", "/api/v1/files/1/offline").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        ask(&w, "GET", "/api/v1/files/32/offline").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send_json(&w, "POST", "/api/v1/files/32/offline", json!({})).await,
        StatusCode::NOT_FOUND
    );
    // Copies the kid asked for while both were open.
    sqlx::query(
        "INSERT INTO offline_copies (id, user_id, file_id, settings, subtitles, status,
             estimated_bytes, created_at, ready_at)
         SELECT c.id, u.id, c.file, '{}', '[]', 'ready', 1, 0, 0
         FROM users u, (SELECT 7 AS id, 1 AS file UNION ALL SELECT 8, 32) c
         WHERE u.username = 'kid'",
    )
    .execute(&w.pool)
    .await
    .unwrap();
    let (_, mine) = ask(&w, "GET", "/api/v1/offline").await;
    let ids: Vec<i64> = serde_json::from_str::<Value>(&mine)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [7], "{mine}");
    assert_eq!(
        ask(&w, "GET", "/api/v1/offline/8").await.0,
        StatusCode::NOT_FOUND
    );
    lift(&w).await;
    assert_eq!(ask(&w, "GET", "/api/v1/offline/8").await.0, StatusCode::OK);
    assert_eq!(
        send_json(&w, "POST", "/api/v1/files/1099511627776/offline", json!({})).await,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an extra isn't copied"
    );

    // A download link got while it was open stops once the title is hidden, and no new one is
    // given.
    let copies = crate::offline::folder(&w._dir.path().join("transcode"));
    std::fs::create_dir_all(&copies).unwrap();
    std::fs::write(crate::offline::video_path(&copies, 8), b"mp4").unwrap();
    let (status, link) = ask(&w, "POST", "/api/v1/offline/8/link").await;
    assert_eq!(status, StatusCode::OK, "{link}");
    let link: Value = serde_json::from_str(&link).unwrap();
    let url = link["download_url"].as_str().unwrap().to_owned();
    assert_eq!(ask(&w, "GET", &url).await.0, StatusCode::OK);
    restrict(
        &w,
        Some(Restriction {
            libraries: Some(vec![1, 2, 5]),
            max_age: Some(12),
        }),
    )
    .await;
    assert_eq!(ask(&w, "GET", &url).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        ask(&w, "POST", "/api/v1/offline/8/link").await.0,
        StatusCode::NOT_FOUND
    );
}
