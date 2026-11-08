//! Artists' portraits, served by MusicBrainz id on discovery posters' terms.

use super::*;
use crate::access::{Restriction, set_restriction};
use crate::musicbrainz::testing::RADIOHEAD;
use crate::wikimedia::testing as commons;

fn uri(artist: &str) -> String {
    format!("/api/v1/music/artist-portraits/{artist}")
}

async fn turn(pool: &SqlitePool, on: bool) {
    for (name, value) in [
        ("MELETE_MUSICBRAINZ", "on"),
        ("MELETE_ARTIST_PORTRAITS", if on { "on" } else { "off" }),
    ] {
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES (?, ?, 0)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
        )
        .bind(name)
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// As a search showing the artist with its portrait at `picture`.
async fn listed(pool: &SqlitePool, artist: &str, picture: &str) {
    let now = crate::db::unix_now();
    sqlx::query(
        "INSERT INTO artist_portraits (mb_artist_id, listed_at, looked_up_at, file, picture_url,
             author, licence) VALUES (?, ?, ?, 'X.jpg', ?, 'Some & One', 'CC BY-SA 4.0')",
    )
    .bind(artist)
    .bind(now)
    .bind(now)
    .bind(picture)
    .execute(pool)
    .await
    .unwrap();
}

/// The fixture with Commons, and the address of the fake's picture for `file`.
async fn with_commons(any_host: bool) -> (Fixture, Router, commons::Log, String) {
    let f = fixture(None).await;
    let (wiki, log) = commons::start_with(any_host).await;
    let picture = commons::picture_url(&wiki, "500px.jpg");
    let app = routes_with(
        f.pool.clone(),
        f.covers.path().join("music"),
        "ffmpeg".into(),
        None,
        Some(Arc::new(wiki)),
    );
    turn(&f.pool, true).await;
    (f, app, log, picture)
}

async fn ask(app: &Router, token: &str, uri: &str) -> (StatusCode, Vec<u8>) {
    let request = Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn only_a_listed_portrait_is_fetched_once_and_none_while_off() {
    let (f, app, log, picture) = with_commons(true).await;
    let (status, _) = ask(&app, &f.token, &uri(RADIOHEAD)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no search listed it");
    listed(&f.pool, RADIOHEAD, &picture).await;
    for _ in 0..2 {
        let (status, bytes) = ask(&app, &f.token, &uri(RADIOHEAD)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, commons::JPEG);
    }
    let pictures = log
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.starts_with("picture"))
        .count();
    assert_eq!(pictures, 1, "fetched once, then kept");
    let kept = crate::artist_portraits::cached(&f.covers.path().join("music"), RADIOHEAD, &picture);
    assert!(kept.exists());

    turn(&f.pool, false).await;
    let (status, _) = ask(&app, &f.token, &uri(RADIOHEAD)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "off: not served");
    // Without Commons (MusicBrainz off) there's nothing to serve.
    turn(&f.pool, true).await;
    let (status, _) = ask(&f.app, &f.token, &uri(RADIOHEAD)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_failed_large_garbled_or_elsewhere_picture_keeps_nothing_and_is_believed_a_day() {
    let (f, app, log, picture) = with_commons(true).await;
    let large = picture.replace("500px.jpg", "large.jpg");
    let garbled = picture.replace("500px.jpg", "garbled.jpg");
    let artists = [
        (
            "00000000-0000-4000-8000-000000000001",
            large.as_str(),
            StatusCode::BAD_GATEWAY,
        ),
        (
            "00000000-0000-4000-8000-000000000002",
            "http://127.0.0.1:9/x.jpg",
            StatusCode::BAD_GATEWAY,
        ),
        (
            "00000000-0000-4000-8000-000000000003",
            garbled.as_str(),
            StatusCode::NOT_FOUND,
        ),
    ];
    for (artist, picture, first) in artists {
        listed(&f.pool, artist, picture).await;
        let (status, bytes) = ask(&app, &f.token, &uri(artist)).await;
        assert_eq!(status, first, "{artist}");
        assert!(!String::from_utf8_lossy(&bytes).contains("127.0.0.1"));
        let folder = crate::artist_portraits::folder(&f.covers.path().join("music"), artist);
        assert!(!folder.exists());
        // Believed a day: not fetched again until then.
        let asked = log.lock().unwrap().len();
        let (status, _) = ask(&app, &f.token, &uri(artist)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{artist}");
        assert_eq!(log.lock().unwrap().len(), asked, "{artist} not asked again");
        let now = crate::db::unix_now();
        let in_a_day = crate::artist_portraits::servable(&f.pool, artist, now + 24 * 3600 + 1)
            .await
            .unwrap();
        assert!(in_a_day.is_some(), "{artist} tried again after a day");
    }

    // As the real client: a picture not on Commons' picture hosts isn't fetched.
    let (f, app, log, picture) = with_commons(false).await;
    listed(&f.pool, RADIOHEAD, &picture).await;
    let (status, _) = ask(&app, &f.token, &uri(RADIOHEAD)).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(log.lock().unwrap().is_empty(), "nothing asked of the fake");
}

#[tokio::test]
async fn sizes_are_made_from_the_kept_portrait() {
    let (f, app, log, picture) = with_commons(true).await;
    listed(&f.pool, RADIOHEAD, &picture).await;
    let kept = crate::artist_portraits::cached(&f.covers.path().join("music"), RADIOHEAD, &picture);
    std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
    let made = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .args(["color=c=green:s=500x600", "-frames:v", "1", "-f", "mjpeg"])
        .arg(&kept)
        .status()
        .unwrap();
    assert!(made.success());
    let (status, _) = ask(&app, &f.token, &format!("{}?size=160", uri(RADIOHEAD))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(kept.with_extension("160.jpg").exists());
    assert!(log.lock().unwrap().is_empty(), "the kept portrait was used");
    let (status, _) = ask(&app, &f.token, &format!("{}?size=7", uri(RADIOHEAD))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_artist_only_in_a_hidden_library_is_served_like_one_in_none() {
    let (f, app, _, picture) = with_commons(true).await;
    // The fixture's Radiohead has albums in its one library.
    listed(&f.pool, RADIOHEAD, &picture).await;
    let elsewhere = "00000000-0000-4000-8000-000000000003";
    listed(&f.pool, elsewhere, &picture).await;
    let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'u'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let none = Restriction {
        libraries: Some(Vec::new()),
        max_age: None,
    };
    set_restriction(&f.pool, user, Some(&none)).await.unwrap();
    // To a user who may not see its library, Radiohead answers as an artist in no library does,
    // so nothing tells them it's there.
    let hidden = ask(&app, &f.token, &uri(RADIOHEAD)).await;
    let absent = ask(&app, &f.token, &uri(elsewhere)).await;
    assert_eq!(hidden, absent);
    assert_eq!(hidden.0, StatusCode::OK);
    set_restriction(&f.pool, user, None).await.unwrap();
    let (status, _) = ask(&app, &f.token, &uri(RADIOHEAD)).await;
    assert_eq!(status, StatusCode::OK);
    let anyone = Request::get(uri(RADIOHEAD)).body(Body::empty()).unwrap();
    let status = app.clone().oneshot(anyone).await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "signed in only");
}
