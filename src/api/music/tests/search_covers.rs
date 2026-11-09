//! Covers of music not in the library, served by release group on discovery posters' terms.

use super::*;
use crate::access::{Restriction, set_restriction};

/// A release group the fake archive has a cover for, in no library.
const ELSEWHERE: &str = "b1392450-0000-4000-8000-000000000000";
/// OK Computer's, which the fixture's albums are matched to.
const OK_COMPUTER: &str = "b1392450-e666-3926-a536-22c65f834433";
/// One the fake archive has no cover for.
const BARE: &str = "00000000-0000-4000-8000-0000000000aa";

fn uri(group: &str) -> String {
    format!("/api/v1/music/release-groups/{group}/cover")
}

async fn turn(pool: &SqlitePool, on: bool) {
    for (name, value) in [
        ("MELETE_MUSICBRAINZ", "on"),
        ("MELETE_SEARCH_COVERS", if on { "on" } else { "off" }),
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

/// The fixture's user.
async fn user(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT id FROM users WHERE username = 'u'")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// As a search by the fixture's user for a request offering it.
async fn offered(pool: &SqlitePool, group: &str) {
    let now = crate::db::unix_now();
    let url = crate::search_covers::offer(pool, Some(group), user(pool).await, now)
        .await
        .unwrap();
    assert_eq!(url, Some(uri(group)));
}

async fn body(response: Response) -> (StatusCode, Vec<u8>) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

#[tokio::test]
async fn only_an_offered_cover_is_fetched_once_and_none_while_off() {
    let (archive, log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    turn(&f.pool, true).await;
    let (status, _) = body(get(&f, &uri(ELSEWHERE), None).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no search offered it");
    assert!(log.lock().unwrap().is_empty(), "nothing asked");

    offered(&f.pool, ELSEWHERE).await;
    for _ in 0..2 {
        let response = get(&f, &uri(ELSEWHERE), None).await;
        assert_eq!(response.headers()[header::CACHE_CONTROL], COVER_CACHE);
        let (status, bytes) = body(response).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, coverart::testing::JPEG);
    }
    assert_eq!(
        *log.lock().unwrap(),
        [format!("{ELSEWHERE} front-500")],
        "only the release group's id, asked once, then cached"
    );
    let cached = f.covers.path().join("music/search").join(ELSEWHERE);
    assert!(cached.join("cover.img").exists());

    turn(&f.pool, false).await;
    let (status, _) = body(get(&f, &uri(ELSEWHERE), None).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "off: not served");

    // Without MusicBrainz there's no archive to ask.
    let off = fixture(None).await;
    turn(&off.pool, true).await;
    offered(&off.pool, ELSEWHERE).await;
    let (status, _) = body(get(&off, &uri(ELSEWHERE), None).await).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn none_found_or_a_failed_fetch_is_believed_a_day_and_keeps_nothing() {
    let (archive, log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    turn(&f.pool, true).await;
    let asked = |group: &str| {
        let log = log.lock().unwrap();
        log.iter().filter(|l| l.starts_with(group)).count()
    };
    offered(&f.pool, BARE).await;
    for _ in 0..2 {
        let (status, _) = body(get(&f, &uri(BARE), None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert_eq!(asked(BARE), 1, "asked once");
    let now = crate::db::unix_now();
    let again = crate::search_covers::offer(&f.pool, Some(BARE), user(&f.pool).await, now)
        .await
        .unwrap();
    assert_eq!(again, None, "a search doesn't offer it again for a while");

    for group in [coverart::testing::FAILING, coverart::testing::OVERSIZED] {
        offered(&f.pool, group).await;
        let (status, bytes) = body(get(&f, &uri(group), None).await).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY, "{group}");
        let said = String::from_utf8_lossy(&bytes);
        assert!(!said.contains("127.0.0.1"), "{said}");
        assert!(!f.covers.path().join("music/search").join(group).exists());
        // The failure is believed a day: not asked again, nor offered.
        let (status, _) = body(get(&f, &uri(group), None).await).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{group}");
        assert_eq!(asked(group), 1, "{group} asked once");
        let again = crate::search_covers::offer(&f.pool, Some(group), user(&f.pool).await, now)
            .await
            .unwrap();
        assert_eq!(again, None, "{group}");
    }
}

#[tokio::test]
async fn sizes_are_made_from_the_kept_cover() {
    let (archive, log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    turn(&f.pool, true).await;
    offered(&f.pool, ELSEWHERE).await;
    let dir = f.covers.path().join("music/search").join(ELSEWHERE);
    std::fs::create_dir_all(&dir).unwrap();
    let made = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .args(["color=c=blue:s=500x500", "-frames:v", "1", "-f", "mjpeg"])
        .arg(dir.join("cover.img"))
        .status()
        .unwrap();
    assert!(made.success());
    let response = get(&f, &format!("{}?size=160", uri(ELSEWHERE)), None).await;
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(body(response).await.0, StatusCode::OK);
    assert!(dir.join("160.jpg").exists(), "kept");
    assert!(log.lock().unwrap().is_empty(), "the kept cover was used");
    let (status, _) = body(get(&f, &format!("{}?size=123", uri(ELSEWHERE)), None).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_album_in_a_hidden_library_is_served_like_one_in_none() {
    let (archive, _) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    turn(&f.pool, true).await;
    offered(&f.pool, OK_COMPUTER).await;
    offered(&f.pool, ELSEWHERE).await;
    let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'u'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let none = Restriction {
        libraries: Some(Vec::new()),
        max_age: None,
    };
    set_restriction(&f.pool, user, Some(&none)).await.unwrap();
    // To a user who may not see its library, OK Computer answers as music in no library does,
    // so nothing tells them it's there.
    let hidden = body(get(&f, &uri(OK_COMPUTER), None).await).await;
    let absent = body(get(&f, &uri(ELSEWHERE), None).await).await;
    assert_eq!(hidden, absent);
    assert_eq!(hidden.0, StatusCode::OK);
    set_restriction(&f.pool, user, None).await.unwrap();
    let (status, _) = body(get(&f, &uri(OK_COMPUTER), None).await).await;
    assert_eq!(status, StatusCode::OK);

    let anyone = Request::get(uri(ELSEWHERE)).body(Body::empty()).unwrap();
    let status = f.app.clone().oneshot(anyone).await.unwrap().status();
    assert_eq!(status, StatusCode::UNAUTHORIZED, "signed in only");
}

#[tokio::test]
async fn a_cover_is_served_only_to_whom_a_search_listed_it_for() {
    let (archive, _log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    turn(&f.pool, true).await;
    offered(&f.pool, ELSEWHERE).await;
    let (status, _) = body(get(&f, &uri(ELSEWHERE), None).await).await;
    assert_eq!(status, StatusCode::OK, "the one who searched");
    // Someone else, who didn't search for it, can't tell from the route that anyone did.
    let other = listener(&f, "bob", None).await;
    let (status, _) = json_as(&f, &other, &uri(ELSEWHERE)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
