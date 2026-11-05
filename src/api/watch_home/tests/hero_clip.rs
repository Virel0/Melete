//! The clip of itself each hero pick offers beside its trailer.

use super::*;

/// The hero pick `kind:id`'s clip.
fn clip_of<'a>(home: &'a Value, title: &str) -> &'a Value {
    let pick = home["hero"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| format!("{}:{}", p["kind"].as_str().unwrap(), p["id"]) == title)
        .unwrap_or_else(|| panic!("{title} isn't a pick in {home}"));
    &pick["clip"]
}

#[tokio::test]
async fn each_pick_offers_a_clip_of_itself_from_where_play_would_start() {
    let f = fixture().await;
    let (me, token) = user(&f, "viewer").await;
    let home = home_of(&f, &token).await;
    // A film an eighth of the way into its running time (136 minutes).
    assert_eq!(
        clip_of(&home, "movie:1"),
        &json!({ "file_id": 6, "start_seconds": 1020.0, "duration_seconds": 40.0 })
    );
    // Neither a running time nor a probed length: an eighth of ten minutes.
    assert_eq!(clip_of(&home, "movie:2")["start_seconds"], 75.0);
    // A series' first episode on disk, its special last.
    assert_eq!(clip_of(&home, "series:1")["file_id"], 1);

    // Partway through the film: from where it was left.
    progress(&f, me, 6, 300.0, 10).await;
    let home = home_of(&f, &token).await;
    assert_eq!(
        clip_of(&home, "movie:1"),
        &json!({ "file_id": 6, "start_seconds": 300.0, "duration_seconds": 40.0 })
    );
}

#[tokio::test]
async fn a_probed_length_is_used_when_there_is_no_running_time() {
    let f = fixture().await;
    sqlx::query(
        "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
         VALUES (7, 1, 1, 1, '{\"duration_seconds\": 4000}')",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, token) = user(&f, "viewer").await;
    let home = home_of(&f, &token).await;
    assert_eq!(clip_of(&home, "movie:2")["start_seconds"], 500.0);
}
