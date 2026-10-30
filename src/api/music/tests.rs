use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

struct Fixture {
    app: Router,
    pool: SqlitePool,
    token: String,
    library: tempfile::TempDir,
    covers: tempfile::TempDir,
}

/// Three albums: OK Computer (Radiohead, a folder cover and motion cover, matched to a
/// release group with an archive cover), Dummy (Portishead, a track with a picture) and Kid
/// A (Radiohead, nothing), plus a missing album.
async fn fixture(coverart: Option<Arc<coverart::Client>>) -> Fixture {
    let pool = crate::db::open_in_memory().await;
    let library = tempfile::tempdir().unwrap();
    let root = library.path();
    std::fs::create_dir_all(root.join("r/ok")).unwrap();
    std::fs::write(root.join("r/ok/cover.jpg"), b"\xff\xd8\xff folder cover").unwrap();
    std::fs::write(root.join("r/ok/cover.mp4"), vec![7u8; 1000]).unwrap();
    std::fs::create_dir_all(root.join("p/dummy")).unwrap();
    let tone = root.join("p/dummy/01.mp3");
    let png = root.join("p/dummy/art.png");
    // A tiny PNG made by ffmpeg, attached to an MP3 as its cover.
    for args in [
        vec![
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=8x8",
            "-frames:v",
            "1",
            png.to_str().unwrap(),
        ],
        vec![
            "-f",
            "lavfi",
            "-i",
            "sine=d=1",
            "-i",
            png.to_str().unwrap(),
            "-map",
            "0",
            "-map",
            "1",
            "-c:v",
            "copy",
            "-disposition:v",
            "attached_pic",
            tone.to_str().unwrap(),
        ],
    ] {
        let status = std::process::Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success());
    }
    std::fs::remove_file(&png).unwrap();
    let (size, modified) = {
        let m = std::fs::metadata(&tone).unwrap();
        (i64::try_from(m.len()).unwrap(), 1)
    };
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
        .bind(root.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            r#"INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES
                   (1, 'name:radiohead', 'Radiohead', 'a74b1b7f-71a5-4011-9441-d0b5e4122711'),
                   (2, 'name:portishead', 'Portishead', NULL), (3, 'name:gone', 'Gone', NULL);
               INSERT INTO mb_releases (id, release_group_id, title, artist_credit, cover_front,
                   fetched_at, date, country)
                   VALUES ('rel', 'b1392450-e666-3926-a536-22c65f834433', 'OK Computer', '[]', 1,
                   0, '1997-06-16', 'GB');
               INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, year,
                   folder, seen_generation, cover_file, motion_file, match_status,
                   match_release_id, match_release_group_id, missing_since) VALUES
                   (1, 1, 'k1', 'OK Computer', 1, 'Radiohead', 1997, 'r/ok', 1, 'r/ok/cover.jpg',
                    'r/ok/cover.mp4', 'matched', 'rel', 'b1392450-e666-3926-a536-22c65f834433', NULL),
                   (2, 1, 'k2', 'Dummy', 2, 'Portishead', 1994, 'p/dummy', 1, NULL, NULL, NULL,
                    NULL, NULL, NULL),
                   (3, 1, 'k3', 'Kid A', 1, 'Radiohead', 2000, 'r/kid', 1, NULL, NULL, 'matched',
                    'rel', 'b1392450-e666-3926-a536-22c65f834433', NULL),
                   (4, 1, 'k4', 'Gone', 3, 'Gone', NULL, 'g', 1, NULL, NULL, NULL, NULL, NULL, 5);
               INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                   first_seen_generation, seen_generation, album_id, tagged, title, artist,
                   track_number, genres, has_picture, mb_recording_id) VALUES
                   (1, 'r/ok/01.flac', 1, 1, 1, 1, 1, 1, 'Airbag', 'Radiohead', 1, '["Rock"]', 0,
                    'rec-1'),
                   (1, 'r/ok/02.flac', 1, 1, 1, 1, 1, 1, 'Paranoid Android', NULL, 2,
                    '["Rock","Art Rock"]', 0, NULL),
                   (1, 'p/dummy/01.mp3', {size}, {modified}, 1, 1, 2, 1, 'Mysterons',
                    'Portishead', 1, '["trip hop"]', 1, NULL),
                   (1, 'r/kid/01.flac', 1, 1, 1, 1, 3, 1, 'Everything', 'Radiohead', 1, '["rock"]',
                    0, NULL);"#
        )))
        .execute(&pool)
        .await
        .unwrap();
    let user = crate::auth::create_user(&pool, "u", "user-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    let (_, token) = crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
        .await
        .unwrap();
    let covers = tempfile::tempdir().unwrap();
    let app = routes(
        pool.clone(),
        covers.path().join("music"),
        covers.path().join("motion"),
        coverart,
    );
    Fixture {
        app,
        pool,
        token,
        library,
        covers,
    }
}

async fn get(f: &Fixture, uri: &str, range: Option<&str>) -> Response {
    let mut request =
        Request::get(uri).header(header::AUTHORIZATION, format!("Bearer {}", f.token));
    if let Some(range) = range {
        request = request.header(header::RANGE, range);
    }
    f.app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json_of(f: &Fixture, uri: &str) -> (StatusCode, Value) {
    let response = get(f, uri, None).await;
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn titles(page: &Value) -> Vec<&str> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn music_search_ignores_accents_and_sorting_a_leading_article() {
    let f = fixture(None).await;
    sqlx::raw_sql(
        "INSERT INTO music_artists (id, key, name) VALUES
             (4, 'name:bjork', 'Björk'), (5, 'name:the beatles', 'The Beatles');
         INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, year,
             folder, seen_generation) VALUES
             (5, 1, 'k5', 'Début', 4, 'Björk', 1993, 'b/debut', 1),
             (6, 1, 'k6', 'Abbey Road', 5, 'The Beatles', 1969, 'b/abbey', 1);
         INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, artist,
             track_number, genres, has_picture) VALUES
             (1, 'b/debut/01.flac', 1, 1, 1, 1, 5, 1, 'Human Behaviour', 'Björk', 1, '[]', 0),
             (1, 'b/abbey/01.flac', 1, 1, 1, 1, 6, 1, 'Come Together', 'The Beatles', 1, '[]',
              0);",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    crate::search_keys::fill(&f.pool).await.unwrap();
    let (_, albums) = json_of(&f, "/api/v1/music/albums?q=bjork%20").await;
    assert_eq!(titles(&albums), ["Début"]);
    let (_, albums) = json_of(&f, "/api/v1/music/albums?q=debut").await;
    assert_eq!(titles(&albums), ["Début"]);
    let (_, tracks) = json_of(&f, "/api/v1/music/tracks?q=BJORK").await;
    assert_eq!(titles(&tracks), ["Human Behaviour"]);
    let (_, artists) = json_of(&f, "/api/v1/music/artists?q=bjork").await;
    assert_eq!(artists["items"][0]["name"], "Björk");
    assert_eq!(artists["total"], 1);

    let (_, by_artist) = json_of(&f, "/api/v1/music/albums").await;
    assert_eq!(
        titles(&by_artist),
        ["Abbey Road", "Début", "Dummy", "OK Computer", "Kid A"],
        "The Beatles under B, Björk after them"
    );
    let (_, artists) = json_of(&f, "/api/v1/music/artists").await;
    let names: Vec<&str> = artists["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["The Beatles", "Björk", "Portishead", "Radiohead"]);
}

#[tokio::test]
async fn lists_filter_by_several_ids_genre_and_text() {
    let f = fixture(None).await;
    let (_, all) = json_of(&f, "/api/v1/music/albums").await;
    assert_eq!(all["total"], 3, "missing albums are left out");
    assert_eq!(titles(&all), ["Dummy", "OK Computer", "Kid A"]);
    let (_, radiohead) = json_of(&f, "/api/v1/music/albums?artist_id=1&sort=year").await;
    assert_eq!(titles(&radiohead), ["Kid A", "OK Computer"]);
    let (_, both) = json_of(&f, "/api/v1/music/albums?artist_id=1,2&q=mm").await;
    assert_eq!(titles(&both), ["Dummy"]);
    let (_, both) = json_of(&f, "/api/v1/music/albums?artist_id=2,1").await;
    assert_eq!(both["total"], 3);
    let (_, rock) = json_of(&f, "/api/v1/music/albums?genre=ROCK").await;
    assert_eq!(rock["total"], 2);

    // An id from matching stands in where the tags have none.
    sqlx::query("UPDATE music_tracks SET match_recording_id = 'rec-2' WHERE track_number = 2")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, tracks) = json_of(&f, "/api/v1/music/tracks?album_id=1,2").await;
    assert_eq!(titles(&tracks), ["Mysterons", "Airbag", "Paranoid Android"]);
    // A track without its own artist shows the album's; ids show where known.
    assert_eq!(tracks["items"][2]["artist"], "Radiohead");
    assert_eq!(tracks["items"][2]["mb_recording_id"], "rec-2");
    assert_eq!(tracks["items"][1]["mb_recording_id"], "rec-1");
    assert_eq!(tracks["items"][1]["mb_release_id"], "rel");
    let (_, art_rock) = json_of(&f, "/api/v1/music/tracks?genre=art%20rock&artist_id=1").await;
    assert_eq!(titles(&art_rock), ["Paranoid Android"]);

    let (_, genres) = json_of(&f, "/api/v1/music/genres").await;
    assert_eq!(
        genres[0],
        json!({ "name": "Rock", "tracks": 3, "albums": 2 })
    );

    let (_, artists) = json_of(&f, "/api/v1/music/artists").await;
    let names: Vec<&str> = artists["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Portishead", "Radiohead"],
        "an artist with no album here isn't listed"
    );
    let (_, radiohead) = json_of(&f, "/api/v1/music/artists/1").await;
    assert_eq!(radiohead["albums"], 2);
    assert_eq!(radiohead["album_list"][0]["title"], "OK Computer");
    assert_eq!(
        json_of(&f, "/api/v1/music/artists/3").await.0,
        StatusCode::NOT_FOUND
    );

    for bad in [
        "/api/v1/music/albums?artist_id=1,x",
        "/api/v1/music/albums?sort=size",
        "/api/v1/music/tracks?limit=501",
        "/api/v1/music/artists?offset=-1",
    ] {
        assert_eq!(json_of(&f, bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn every_item_carries_the_musicbrainz_ids_known_for_it() {
    let f = fixture(None).await;
    // Dummy is matched to a release crediting one artist; its artist's tags had no id.
    sqlx::raw_sql(
            r#"INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at)
                   VALUES ('rel-d', 'rg-d', 'Dummy',
                   '[{"name":"Portishead","join":"","id":"8f6bd1e4","sort_name":"Portishead"}]', 0);
               UPDATE music_albums SET match_release_id = 'rel-d', match_release_group_id = 'rg-d',
                   match_status = 'matched' WHERE id = 2;
               INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
                   first_seen_generation, seen_generation, album_id, tagged, title, artist,
                   track_number, genres) VALUES
                   (1, 'p/dummy/02.mp3', 1, 1, 1, 1, 2, 1, 'Guest spot', 'Someone Else', 2, '[]');"#,
        )
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, dummy) = json_of(&f, "/api/v1/music/albums/2").await;
    assert_eq!(dummy["mb_artist_id"], "8f6bd1e4");
    assert_eq!(dummy["mb_release_group_id"], "rg-d");
    let tracks = dummy["track_list"].as_array().unwrap();
    assert_eq!(
        (
            &tracks[0]["mb_release_group_id"],
            &tracks[0]["mb_artist_id"]
        ),
        (&json!("rg-d"), &json!("8f6bd1e4")),
        "a track by the album's artist takes the album's ids"
    );
    assert_eq!(
        (
            &tracks[1]["mb_release_group_id"],
            &tracks[1]["mb_artist_id"]
        ),
        (&json!("rg-d"), &Value::Null),
        "another artist's track isn't given the album artist's id"
    );
    let (_, portishead) = json_of(&f, "/api/v1/music/artists/2").await;
    assert_eq!(portishead["mb_artist_id"], "8f6bd1e4");
    assert_eq!(portishead["album_list"][0]["mb_artist_id"], "8f6bd1e4");
    // An artist's own id wins; an album with two artists credited gives none.
    let (_, ok) = json_of(&f, "/api/v1/music/albums/1").await;
    assert_eq!(ok["mb_artist_id"], "a74b1b7f-71a5-4011-9441-d0b5e4122711");
    sqlx::query(
        "UPDATE mb_releases SET artist_credit = '[{\"id\":\"x\"},{\"id\":\"y\"}]'
             WHERE id = 'rel-d'",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, artists) = json_of(&f, "/api/v1/music/artists").await;
    assert_eq!(artists["items"][0]["name"], "Portishead");
    assert_eq!(artists["items"][0]["mb_artist_id"], Value::Null);
}

#[tokio::test]
async fn album_pages_carry_release_genres_and_tracks() {
    let f = fixture(None).await;
    let (status, album) = json_of(&f, "/api/v1/music/albums/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(album["library"], "music");
    assert_eq!(album["genres"], json!(["Rock", "Art Rock"]));
    assert_eq!(album["release"]["country"], "GB");
    assert_eq!(album["track_list"].as_array().unwrap().len(), 2);
    assert_eq!(album["cover"], "/api/v1/music/albums/1/cover");
    assert_eq!(album["motion_cover"], "/api/v1/music/albums/1/motion");
    // With the archive off, Kid A's only cover would be the archive's: none.
    assert_eq!(
        json_of(&f, "/api/v1/music/albums/3").await.1["cover"],
        Value::Null
    );
    assert_eq!(
        json_of(&f, "/api/v1/music/albums/4").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn albums_carry_their_covers_colours_beside_the_cover() {
    let f = fixture(None).await;
    let colours = json!({ "background": "#102040", "text1": "#f0c040", "text2": "#f08080",
            "text3": "#9a8240", "text4": "#9a5c66" });
    sqlx::query(
        "INSERT INTO music_cover_colours (album_id, source, colours, measured_at)
             VALUES (1, 'file', ?1, 0), (3, 'rg', ?1, 0)",
    )
    .bind(colours.to_string())
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        json_of(&f, "/api/v1/music/albums/1").await.1["colours"],
        colours
    );
    let (_, page) = json_of(&f, "/api/v1/music/albums?sort=title").await;
    assert_eq!(titles(&page), ["Dummy", "Kid A", "OK Computer"]);
    assert_eq!(page["items"][2]["colours"], colours);
    assert_eq!(page["items"][1]["colours"], Value::Null);
    assert_eq!(
        page["items"][0]["colours"],
        Value::Null,
        "Dummy's aren't worked out yet"
    );
    // Kid A's only cover is the archive's, off here: no cover, so no colours either.
    assert_eq!(
        json_of(&f, "/api/v1/music/albums/3").await.1["colours"],
        Value::Null
    );
}

#[tokio::test]
async fn covers_come_from_the_folder_then_tags_then_the_archive() {
    let (archive, log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;

    let folder = get(&f, "/api/v1/music/albums/1/cover", None).await;
    assert_eq!(folder.status(), StatusCode::OK);
    assert_eq!(folder.headers()[header::CACHE_CONTROL], COVER_CACHE);
    let bytes = folder.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"\xff\xd8\xff folder cover");

    let tagged = get(&f, "/api/v1/music/albums/2/cover", None).await;
    assert_eq!(tagged.status(), StatusCode::OK);
    assert_eq!(tagged.headers()[header::CONTENT_TYPE], "image/png");
    // Pulled out once, then served from the cache even if the file goes.
    std::fs::remove_file(f.library.path().join("p/dummy/01.mp3")).unwrap();
    assert_eq!(
        get(&f, "/api/v1/music/albums/2/cover", None).await.status(),
        StatusCode::OK
    );

    for _ in 0..2 {
        let archived = get(&f, "/api/v1/music/albums/3/cover", None).await;
        assert_eq!(archived.status(), StatusCode::OK);
        let bytes = archived.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], coverart::testing::JPEG);
    }
    assert_eq!(log.lock().unwrap().len(), 1, "asked once, then cached");
    let cached: Vec<_> = std::fs::read_dir(f.covers.path().join("music"))
        .unwrap()
        .collect();
    assert_eq!(cached.len(), 2);
}

#[tokio::test]
async fn the_archive_is_asked_once_for_a_missing_cover_and_never_when_off() {
    let (archive, log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    // Album 1 has a folder cover: the archive isn't asked.
    get(&f, "/api/v1/music/albums/1/cover", None).await;
    assert!(log.lock().unwrap().is_empty());
    // A release group the archive has nothing for is asked about once a day at most.
    sqlx::query(
        "UPDATE music_albums SET match_release_group_id = '00000000-0000-0000-0000-000000000000'
             WHERE id = 3",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    for _ in 0..2 {
        let status = get(&f, "/api/v1/music/albums/3/cover", None).await.status();
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert_eq!(log.lock().unwrap().len(), 1);

    let off = fixture(None).await;
    let status = get(&off, "/api/v1/music/albums/3/cover", None)
        .await
        .status();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_release_group_that_isnt_a_musicbrainz_id_never_names_a_cache_file() {
    let (archive, log) = coverart::testing::start().await;
    let f = fixture(Some(Arc::new(archive))).await;
    // A cached image outside the covers cache, and a folder whose name a stored id could climb
    // out through.
    let cache = f.covers.path().join("music");
    std::fs::create_dir_all(cache.join("rg-x")).unwrap();
    std::fs::write(f.covers.path().join("outside.img"), b"\xff\xd8\xff outside").unwrap();
    sqlx::query("UPDATE music_albums SET match_release_group_id = 'x/../../outside' WHERE id = 3")
        .execute(&f.pool)
        .await
        .unwrap();
    let status = get(&f, "/api/v1/music/albums/3/cover", None).await.status();
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(log.lock().unwrap().is_empty(), "the archive isn't asked");
}

#[tokio::test]
async fn motion_covers_are_served_with_byte_ranges() {
    let f = fixture(None).await;
    let part = get(&f, "/api/v1/music/albums/1/motion", Some("bytes=0-99")).await;
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.headers()[header::CONTENT_TYPE], "video/mp4");
    let bytes = part.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.len(), 100);
    assert_eq!(
        get(&f, "/api/v1/music/albums/2/motion", None)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

async fn served(f: &Fixture, album: i64) -> StatusCode {
    get(f, &format!("/api/v1/music/albums/{album}/motion"), None)
        .await
        .status()
}

#[tokio::test]
async fn motion_covers_made_from_covers_are_shown_while_on_unless_hidden() {
    let f = fixture(None).await;
    // Dummy's cover (a track's picture) has a motion cover made from it.
    sqlx::raw_sql(
        "INSERT INTO music_cover_colours (album_id, source, colours, measured_at)
                 VALUES (2, 'tag:p/dummy/01.mp3:1:1#00', '{}', 0);
             INSERT INTO music_motion_covers (album_id, state, source, version, file, queued_at,
                 finished_at)
                 VALUES (2, 'done', 'tag:p/dummy/01.mp3:1:1#00', 1, '2.mp4', 0, 0);",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let videos = f.covers.path().join("motion");
    std::fs::create_dir_all(&videos).unwrap();
    std::fs::write(videos.join("2.mp4"), vec![9u8; 500]).unwrap();
    let motion_of = |album: Value| album["motion_cover"].clone();
    // Off by default: not shown, not served.
    assert_eq!(
        motion_of(json_of(&f, "/api/v1/music/albums/2").await.1),
        Value::Null
    );
    assert_eq!(served(&f, 2).await, StatusCode::NOT_FOUND);

    // On: shown in lists and details, and served.
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_MOTION_COVERS', 'on', 0)",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, dummy) = json_of(&f, "/api/v1/music/albums/2").await;
    assert_eq!(motion_of(dummy.clone()), "/api/v1/music/albums/2/motion");
    assert_eq!(
        dummy["motion_cover_generation"],
        json!({"state": "done", "reason": null})
    );
    let (_, all) = json_of(&f, "/api/v1/music/albums?q=dummy").await;
    assert_eq!(
        motion_of(all["items"][0].clone()),
        "/api/v1/music/albums/2/motion"
    );
    let full = get(&f, "/api/v1/music/albums/2/motion", None).await;
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(
        full.into_body().collect().await.unwrap().to_bytes().len(),
        500
    );

    // Hidden by this user: gone for them, but an album's own motion cover still shows.
    let user: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'u'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    crate::motion::choose(&f.pool, user, false).await.unwrap();
    assert_eq!(
        motion_of(json_of(&f, "/api/v1/music/albums/2").await.1),
        Value::Null
    );
    assert_eq!(served(&f, 2).await, StatusCode::NOT_FOUND);
    assert_eq!(
        motion_of(json_of(&f, "/api/v1/music/albums/1").await.1),
        "/api/v1/music/albums/1/motion"
    );
    crate::motion::choose(&f.pool, user, true).await.unwrap();

    // Made from a cover the album no longer has: not shown until it's made again.
    sqlx::query("UPDATE music_cover_colours SET source = 'tag:p/dummy/01.mp3:2:2#11'")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, dummy) = json_of(&f, "/api/v1/music/albums/2").await;
    assert_eq!(motion_of(dummy.clone()), Value::Null);
    assert_eq!(dummy["motion_cover_generation"], Value::Null);
    assert_eq!(served(&f, 2).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tracks_say_whether_theyre_dolby_atmos_or_multichannel() {
    let f = fixture(None).await;
    sqlx::raw_sql(
        "INSERT INTO music_track_probes (track_id, size, modified_at, codec, profile, channels,
                 atmos, duration_seconds, probed_at) VALUES
                 (1, 1, 1, 'eac3', 'Dolby Digital Plus + Dolby Atmos', 6, 1, 200.5, 0),
                 (4, 999, 1, 'truehd', 'Dolby TrueHD + Dolby Atmos', 8, 1, NULL, 0);
             UPDATE music_tracks SET channels = 6 WHERE id = 2;",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (status, page) = json_of(&f, "/api/v1/music/tracks?sort=title").await;
    assert_eq!(status, StatusCode::OK);
    let by_title = |title: &str| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["title"] == title)
            .unwrap()
            .clone()
    };
    let airbag = by_title("Airbag");
    assert_eq!(
        (
            &airbag["spatial"],
            &airbag["codec"],
            &airbag["channels"],
            &airbag["duration_seconds"]
        ),
        (&json!("atmos"), &json!("eac3"), &json!(6), &json!(200.5))
    );
    assert_eq!(by_title("Paranoid Android")["spatial"], "multichannel");
    assert_eq!(by_title("Mysterons")["spatial"], Value::Null);
    // A probe of an older version of the file says nothing about this one.
    assert_eq!(by_title("Everything")["spatial"], Value::Null);
    assert_eq!(by_title("Everything")["codec"], Value::Null);
}

fn names(page: &Value) -> Vec<&str> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn featured_artists_are_listed_with_the_albums_they_appear_on() {
    let f = fixture(None).await;
    // Airbag features a guest with no album of their own; Mysterons features Radiohead.
    sqlx::raw_sql(
        "INSERT INTO music_artists (id, key, name) VALUES (5, 'name:guest', 'Guest');
         INSERT INTO music_track_artists (track_id, position, artist_id, name) VALUES
             (1, 0, 1, 'Radiohead'), (1, 1, 5, 'The Guest'),
             (3, 0, 2, 'Portishead'), (3, 1, 1, 'Radiohead');",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, artists) = json_of(&f, "/api/v1/music/artists").await;
    assert_eq!(names(&artists), ["Guest", "Portishead", "Radiohead"]);
    assert_eq!(artists["total"], 3);
    assert_eq!(
        (
            &artists["items"][0]["albums"],
            &artists["items"][0]["appears_on"]
        ),
        (&json!(0), &json!(1))
    );
    assert_eq!(
        (
            &artists["items"][2]["albums"],
            &artists["items"][2]["appears_on"]
        ),
        (&json!(2), &json!(1)),
        "their own albums aren't ones they appear on"
    );
    let (_, guests) = json_of(&f, "/api/v1/music/artists?q=gue").await;
    assert_eq!(names(&guests), ["Guest"]);
    // Found by the folded search too, accents and punctuation aside.
    crate::search_keys::fill(&f.pool).await.unwrap();
    let (_, guests) = json_of(&f, "/api/v1/music/artists?q=G%C3%BC-est").await;
    assert_eq!(names(&guests), ["Guest"]);

    let (status, guest) = json_of(&f, "/api/v1/music/artists/5").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(guest["album_list"], json!([]));
    assert_eq!(guest["appears_on_list"][0]["title"], "OK Computer");
    let (_, radiohead) = json_of(&f, "/api/v1/music/artists/1").await;
    assert_eq!(radiohead["appears_on_list"][0]["title"], "Dummy");
    assert_eq!(radiohead["appears_on_list"].as_array().unwrap().len(), 1);

    let (_, tracks) = json_of(&f, "/api/v1/music/tracks?artist_id=5").await;
    assert_eq!(titles(&tracks), ["Airbag"]);
    assert_eq!(
        tracks["items"][0]["artists"],
        json!([
            { "id": 1, "name": "Radiohead", "mb_artist_id": "a74b1b7f-71a5-4011-9441-d0b5e4122711" },
            { "id": 5, "name": "The Guest", "mb_artist_id": null },
        ])
    );
    assert_eq!(tracks["items"][0]["artist"], "Radiohead", "shown as tagged");
    let (_, tracks) = json_of(&f, "/api/v1/music/tracks?artist_id=1").await;
    assert_eq!(
        titles(&tracks),
        ["Mysterons", "Airbag", "Paranoid Android", "Everything"]
    );
    assert_eq!(tracks["items"][2]["artists"], json!([]));

    // Only present tracks count.
    sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = 1")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, artists) = json_of(&f, "/api/v1/music/artists").await;
    assert_eq!(names(&artists), ["Portishead", "Radiohead"]);
    assert_eq!(
        json_of(&f, "/api/v1/music/artists/5").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn tracks_are_listed_by_their_ids() {
    let f = fixture(None).await;
    // In the list's own order, whatever order the ids came in.
    let (_, tracks) = json_of(&f, "/api/v1/music/tracks?id=4,1,3").await;
    assert_eq!(titles(&tracks), ["Mysterons", "Airbag", "Everything"]);
    assert_eq!(tracks["total"], 3);
    let (_, narrowed) = json_of(&f, "/api/v1/music/tracks?id=4,1,3&album_id=1").await;
    assert_eq!(titles(&narrowed), ["Airbag"]);
    // A track gone from the library, or an id that was never one, is left out.
    sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = 1")
        .execute(&f.pool)
        .await
        .unwrap();
    let (_, tracks) = json_of(&f, "/api/v1/music/tracks?id=1,3,99").await;
    assert_eq!(titles(&tracks), ["Mysterons"]);
    let many: Vec<String> = (1..=101).map(|i| i.to_string()).collect();
    for bad in [
        "/api/v1/music/tracks?id=1,x".to_owned(),
        format!("/api/v1/music/tracks?id={}", many.join(",")),
    ] {
        assert_eq!(json_of(&f, &bad).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
}
