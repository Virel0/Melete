//! The library's artists' portraits: a picture of their own beside their albums first, else
//! Commons', to those who may see the artist.

use super::*;
use crate::musicbrainz::testing::RADIOHEAD;
use crate::wikimedia::testing as commons;

const PORTRAIT: &str = "/api/v1/music/artists/1/portrait";

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

/// The fixture with Commons, and the address of the fake's picture.
async fn with_commons() -> (Fixture, Router, commons::Log, String) {
    let f = fixture(None).await;
    let (wiki, log) = commons::start().await;
    let picture = commons::picture_url(&wiki, "500px.jpg");
    let app = routes_with(
        f.pool.clone(),
        f.covers.path().join("music"),
        "ffmpeg".into(),
        None,
        Some(Arc::new(wiki)),
    );
    (f, app, log, picture)
}

/// As the background lookup finding Radiohead's portrait at `picture`: not listed by a search.
async fn looked_up(pool: &SqlitePool, picture: &str) {
    sqlx::query(
        "INSERT INTO artist_portraits (mb_artist_id, listed_at, looked_up_at, file, picture_url,
             author, licence) VALUES (?, 0, ?, 'X.jpg', ?, 'Some & One', 'CC BY-SA 4.0')",
    )
    .bind(RADIOHEAD)
    .bind(crate::db::unix_now())
    .bind(picture)
    .execute(pool)
    .await
    .unwrap();
}

/// A green JPEG, 300 pixels square, at `rel` in the library, noted as the picture above
/// Radiohead's two albums (both in `r/`).
fn own_picture(f: &Fixture, rel: &str) -> Vec<u8> {
    let path = f.library.path().join(rel);
    let made = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .args(["color=c=green:s=300x300", "-frames:v", "1", "-f", "mjpeg"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    std::fs::read(path).unwrap()
}

async fn note_own(pool: &SqlitePool, rel: Option<&str>, albums: &str) {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE music_albums SET artist_file = ? WHERE id IN ({albums})"
    )))
    .bind(rel)
    .execute(pool)
    .await
    .unwrap();
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

/// Radiohead's and Portishead's portrait and credit, in the list and on Radiohead's page.
async fn shown(app: &Router, token: &str) -> (Value, Value, Value) {
    let (_, list) = ask(app, token, "/api/v1/music/artists").await;
    let list: Value = serde_json::from_slice(&list).unwrap();
    let item = |name: &str| {
        let item = list["items"].as_array().unwrap().iter();
        let found = item.clone().find(|a| a["name"] == name).unwrap();
        json!([found["portrait"], found["portrait_credit"]])
    };
    let (_, page) = ask(app, token, "/api/v1/music/artists/1").await;
    let page: Value = serde_json::from_slice(&page).unwrap();
    let page = json!([page["portrait"], page["portrait_credit"]]);
    (item("Radiohead"), item("Portishead"), page)
}

#[tokio::test]
async fn a_picture_of_their_own_comes_first_whatever_the_settings() {
    let (f, app, log, picture) = with_commons().await;
    let bytes = own_picture(&f, "r/artist.jpg");
    note_own(&f.pool, Some("r/artist.jpg"), "1, 3").await;
    let own = json!([PORTRAIT, null]);
    let none = json!([null, null]);
    assert_eq!(
        shown(&app, &f.token).await,
        (own.clone(), none.clone(), own.clone())
    );
    assert_eq!(ask(&app, &f.token, PORTRAIT).await, (StatusCode::OK, bytes));
    let (status, small) = ask(&app, &f.token, &format!("{PORTRAIT}?size=160")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jpeg_width(&small), Some(160));
    let (status, _) = ask(&app, &f.token, &format!("{PORTRAIT}?size=7")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // With Commons' known too, their own still wins, with no credit, and Commons isn't asked.
    turn(&f.pool, true).await;
    looked_up(&f.pool, &picture).await;
    assert_eq!(shown(&app, &f.token).await.0, own);
    assert_eq!(ask(&app, &f.token, PORTRAIT).await.0, StatusCode::OK);
    assert!(log.lock().unwrap().is_empty());

    // An album in another folder: the picture isn't theirs alone, so Commons' is shown, and
    // with portraits off, none.
    note_own(&f.pool, None, "3").await;
    assert_eq!(shown(&app, &f.token).await.0[1]["author"], "Some & One");
    let served = ask(&app, &f.token, PORTRAIT).await;
    assert_eq!(served, (StatusCode::OK, commons::JPEG.to_vec()));
    turn(&f.pool, false).await;
    assert_eq!(shown(&app, &f.token).await.0, none);
    assert_eq!(ask(&app, &f.token, PORTRAIT).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_picture_outside_the_library_is_never_served() {
    let f = fixture(None).await;
    let outside = f.covers.path().join("elsewhere.jpg");
    std::fs::write(&outside, b"\xff\xd8\xff not in the library").unwrap();
    std::os::unix::fs::symlink(&outside, f.library.path().join("r/artist.jpg")).unwrap();
    note_own(&f.pool, Some("r/artist.jpg"), "1, 3").await;
    for uri in [PORTRAIT.to_owned(), format!("{PORTRAIT}?size=160")] {
        let (status, bytes) = ask(&f.app, &f.token, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(!String::from_utf8_lossy(&bytes).contains("not in the library"));
    }
}

#[tokio::test]
async fn commons_portraits_come_with_the_setting_and_their_credit() {
    let (f, app, log, picture) = with_commons().await;
    looked_up(&f.pool, &picture).await;
    let none = json!([null, null]);
    assert_eq!(shown(&app, &f.token).await.0, none, "off");
    assert_eq!(ask(&app, &f.token, PORTRAIT).await.0, StatusCode::NOT_FOUND);

    turn(&f.pool, true).await;
    let credit = json!({ "author": "Some & One", "licence": "CC BY-SA 4.0",
                         "licence_url": null, "source_url": null });
    let found = json!([PORTRAIT, credit]);
    assert_eq!(shown(&app, &f.token).await, (found.clone(), none, found));
    for _ in 0..2 {
        let served = ask(&app, &f.token, PORTRAIT).await;
        assert_eq!(served, (StatusCode::OK, commons::JPEG.to_vec()));
    }
    let pictures = log.lock().unwrap().len();
    assert_eq!(pictures, 1, "fetched once, then kept");
    // A lookup for the library isn't a search listing it: not served by its MusicBrainz id.
    let by_id = format!("/api/v1/music/artist-portraits/{RADIOHEAD}");
    assert_eq!(ask(&app, &f.token, &by_id).await.0, StatusCode::NOT_FOUND);
    // Without Commons (MusicBrainz off) there's nothing to serve.
    assert_eq!(
        ask(&f.app, &f.token, PORTRAIT).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn a_picture_commons_failed_to_give_isnt_offered_for_a_day() {
    let (f, app, log, picture) = with_commons().await;
    turn(&f.pool, true).await;
    looked_up(&f.pool, &picture.replace("500px.jpg", "garbled.jpg")).await;
    assert_eq!(ask(&app, &f.token, PORTRAIT).await.0, StatusCode::NOT_FOUND);
    let asked = log.lock().unwrap().len();
    assert_eq!(shown(&app, &f.token).await.0, json!([null, null]));
    assert_eq!(ask(&app, &f.token, PORTRAIT).await.0, StatusCode::NOT_FOUND);
    assert_eq!(log.lock().unwrap().len(), asked, "not asked again");
    let tomorrow = crate::db::unix_now() + 24 * 3600 + 1;
    let credits = crate::artist_portraits::credits(&f.pool, &[RADIOHEAD], tomorrow)
        .await
        .unwrap();
    assert!(credits.contains_key(RADIOHEAD), "offered again after a day");
}

#[tokio::test]
async fn only_who_may_see_the_artist_sees_their_portrait_and_from_their_albums() {
    let f = fixture(None).await;
    let bytes = own_picture(&f, "r/artist.jpg");
    note_own(&f.pool, Some("r/artist.jpg"), "1, 3").await;
    let nobody = listener(&f, "nobody", Some(&[])).await;
    let artist = json_as(&f, &nobody, "/api/v1/music/artists/1").await.0;
    assert_eq!(artist, StatusCode::NOT_FOUND);
    let (status, body) = ask(&f.app, &nobody, PORTRAIT).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "as the artist's page");
    assert!(!String::from_utf8_lossy(&body).contains("portrait"));

    // Another album of theirs in a second library: their albums are in two folders, so they
    // have none of their own, but to someone who sees only the first they do.
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (2, 'more', 'music', '/nowhere');
         INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
             seen_generation) VALUES (5, 2, 'k5', 'Amnesiac', 1, 'Radiohead', 'x/amnesiac', 1);",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        ask(&f.app, &f.token, PORTRAIT).await.0,
        StatusCode::NOT_FOUND
    );
    let first = listener(&f, "first", Some(&[1])).await;
    assert_eq!(ask(&f.app, &first, PORTRAIT).await, (StatusCode::OK, bytes));
    let (_, page) = json_as(&f, &first, "/api/v1/music/artists/1").await;
    assert_eq!(page["portrait"], PORTRAIT);
}
