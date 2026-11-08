//! Music results not in the library get their release group's cover only with covers of music
//! not in the library on, offered on discovery posters' terms.

use super::*;
use crate::musicbrainz::testing::OK_COMPUTER;

const ALBUMS: &str = "/api/v1/requests/search?kind=album&q=OK%20Computer";
const SONGS: &str = "/api/v1/requests/search?kind=song&q=Paranoid%20Android";
const ARTISTS: &str = "/api/v1/requests/search?kind=artist&q=Radiohead";

fn first_poster(found: &Value) -> Value {
    found[0]["poster"].clone()
}

async fn noted(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar("SELECT release_group_id FROM search_covers")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn music_not_in_the_library_gets_its_cover_only_when_on() {
    let pool = db::open_in_memory().await;
    let alice = token(&pool, "alice", false).await;
    let (mb, _) = crate::musicbrainz::testing::start(0).await;
    let app = routes(pool.clone(), Some(Arc::new(mb)), Sources::default());
    for uri in [ALBUMS, SONGS, ARTISTS] {
        let (status, found) = send(&app, "GET", uri, &alice, None).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        assert_eq!(first_poster(&found), Value::Null, "off by default: {uri}");
    }
    assert!(noted(&pool).await.is_empty(), "off: nothing noted");

    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_MUSICBRAINZ', 'on', 0),
             ('MELETE_SEARCH_COVERS', 'on', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let cover = json!(format!("/api/v1/music/release-groups/{OK_COMPUTER}/cover"));
    let (_, albums) = send(&app, "GET", ALBUMS, &alice, None).await;
    assert_eq!(first_poster(&albums), cover);
    // Paranoid Android is on OK Computer (the fake lists no single of it): the album's cover.
    let (_, songs) = send(&app, "GET", SONGS, &alice, None).await;
    assert_eq!(first_poster(&songs), cover);
    let (_, artists) = send(&app, "GET", ARTISTS, &alice, None).await;
    assert_eq!(first_poster(&artists), Value::Null, "artists have no cover");
    assert_eq!(noted(&pool).await, [OK_COMPUTER]);
}

#[tokio::test]
async fn an_album_in_a_hidden_library_gets_no_cover_from_outside() {
    let pool = db::open_in_memory().await;
    let alice = token(&pool, "alice", false).await;
    let kid = token(&pool, "kid", false).await;
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_MUSICBRAINZ', 'on', 0),
             ('MELETE_SEARCH_COVERS', 'on', 0);
         INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/m'),
             (5, 'private', 'music', '/p');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
             seen_generation, mb_release_group_id)
             VALUES (7, 5, 'k', 'OK Computer', 'Radiohead', 'x', 1, ?)",
    )
    .bind(OK_COMPUTER)
    .execute(&pool)
    .await
    .unwrap();
    let kid_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'kid'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let only_open = crate::access::Restriction {
        libraries: Some(vec![4]),
        max_age: None,
    };
    crate::access::set_restriction(&pool, kid_id, Some(&only_open))
        .await
        .unwrap();
    let (mb, _) = crate::musicbrainz::testing::start(0).await;
    let app = routes(pool.clone(), Some(Arc::new(mb)), Sources::default());

    let (_, found) = send(&app, "GET", ALBUMS, &kid, None).await;
    assert_eq!(found[0]["in_library"], json!(false));
    assert_eq!(first_poster(&found), Value::Null);
    assert!(noted(&pool).await.is_empty());
    // Who may see it gets the library's own cover, and nothing is offered from outside.
    let (_, found) = send(&app, "GET", ALBUMS, &alice, None).await;
    assert_eq!(first_poster(&found), json!("/api/v1/music/albums/7/cover"));
    assert!(noted(&pool).await.is_empty());
}
