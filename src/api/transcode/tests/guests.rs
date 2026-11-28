//! A watch party guest's conversions, held to the party's cap.

use super::*;

/// A party on file 1, holding guests to `cap`, and a guest in it: their pass.
pub(super) async fn guest_in(f: &Fixture, cap: Option<i64>) -> String {
    let user: i64 = sqlx::query_scalar("SELECT user_id FROM tokens WHERE id = ?")
        .bind(f.token_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let state = crate::watch_party::State {
        file_id: 1,
        position_ms: 0,
        playing: true,
    };
    let party = crate::watch_party::start(&f.pool, (user, f.token_id), &state, cap)
        .await
        .unwrap()
        .unwrap();
    let (invite, _) = crate::watch_party::guests::invite(&f.pool, (user, f.token_id), party)
        .await
        .unwrap()
        .unwrap();
    let (pass, _) = crate::watch_party::guests::accept(&f.pool, &invite, "Sam")
        .await
        .unwrap()
        .unwrap();
    pass
}

/// A request with a guest's pass.
pub(super) async fn as_guest(f: &Fixture, pass: &str, uri: &str, body: Option<Value>) -> Response {
    let mut request = Request::builder()
        .method(if body.is_some() { "POST" } else { "GET" })
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {pass}"));
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    f.app.clone().oneshot(request).await.unwrap()
}

#[tokio::test]
async fn a_capped_guest_gets_the_video_copied_when_the_file_is_within_the_cap() {
    let f = fixture().await;
    let pass = guest_in(&f, Some(8_000_000)).await;
    let copy = json!({ "video_codec": "copy" });
    let made = as_guest(&f, &pass, "/api/v1/party/transcode", Some(copy)).await;
    assert_eq!(made.status(), StatusCode::OK);
    let made = json_body(made).await;
    let url = made["playlist_url"].as_str().unwrap();
    let playlist = as_guest(&f, &pass, url, None).await;
    assert_eq!(
        playlist.status(),
        StatusCode::OK,
        "copied, the file's own bitrate is held to the cap"
    );
    let segment = as_guest(&f, &pass, &url.replace("main.m3u8", "0.ts"), None).await;
    assert_eq!(segment.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_guests_copy_over_the_cap_is_refused() {
    let f = fixture().await;
    let pass = guest_in(&f, Some(1_000)).await;
    let copy = json!({ "video_codec": "copy" });
    let made = as_guest(&f, &pass, "/api/v1/party/transcode", Some(copy)).await;
    assert_eq!(made.status(), StatusCode::FORBIDDEN);
}
