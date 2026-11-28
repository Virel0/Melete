//! A watch party guest's conversions, held to the party's cap, and the conversions a party
//! shares.

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

/// Segment 0 of a playlist, asked for by `pass` (a guest) or the household's sign-in.
async fn first_segment(f: &Fixture, pass: Option<&str>, playlist: &str) {
    let url = playlist.replace("main.m3u8", "0.ts");
    let response = match pass {
        Some(pass) => as_guest(f, pass, &url, None).await,
        None => call(f, "GET", &url, None).await,
    };
    assert_eq!(response.status(), StatusCode::OK);
}

async fn household_playlist(f: &Fixture, ask: Value) -> String {
    let made = call(f, "POST", "/api/v1/files/1/transcode", Some(ask)).await;
    assert_eq!(made.status(), StatusCode::OK);
    json_body(made).await["playlist_url"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn guest_playlist(f: &Fixture, pass: &str, ask: Value) -> String {
    let made = as_guest(f, pass, "/api/v1/party/transcode", Some(ask)).await;
    assert_eq!(made.status(), StatusCode::OK);
    json_body(made).await["playlist_url"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a_party_asking_alike_shares_one_conversion() {
    let f = fixture().await;
    let pass = guest_in(&f, None).await;
    let host = household_playlist(&f, json!({})).await;
    let guest = guest_playlist(&f, &pass, json!({})).await;
    assert_ne!(host, guest, "each has a link of their own");
    first_segment(&f, None, &host).await;
    first_segment(&f, Some(&pass), &guest).await;
    assert_eq!(f.transcoder.running_count(), 1, "one ffmpeg for the party");
}

#[tokio::test]
async fn different_audio_or_a_lower_rung_gets_a_conversion_of_its_own() {
    let f = fixture().await;
    let pass = guest_in(&f, None).await;
    let host = household_playlist(&f, json!({})).await;
    first_segment(&f, None, &host).await;
    let other_sound = guest_playlist(&f, &pass, json!({ "audio_mode": "dialogue" })).await;
    first_segment(&f, Some(&pass), &other_sound).await;
    assert_eq!(f.transcoder.running_count(), 2, "another sound");
    let lower = guest_playlist(&f, &pass, json!({ "max_bitrate": 300_000 })).await;
    first_segment(&f, Some(&pass), &lower).await;
    assert_eq!(f.transcoder.running_count(), 3, "a lower bitrate");
}

#[tokio::test]
async fn a_capped_guest_never_shares_a_better_conversion() {
    let f = fixture().await;
    let pass = guest_in(&f, Some(1_000_000)).await;
    let host = household_playlist(&f, json!({})).await;
    first_segment(&f, None, &host).await;
    let guest = guest_playlist(&f, &pass, json!({})).await;
    first_segment(&f, Some(&pass), &guest).await;
    assert_eq!(
        f.transcoder.running_count(),
        2,
        "the guest's is held to the cap"
    );
}

#[tokio::test]
async fn outside_a_party_nobody_shares() {
    let f = fixture().await;
    let host = household_playlist(&f, json!({})).await;
    let again = household_playlist(&f, json!({})).await;
    first_segment(&f, None, &host).await;
    first_segment(&f, None, &again).await;
    assert_eq!(f.transcoder.running_count(), 2, "two links, two runs");
}
