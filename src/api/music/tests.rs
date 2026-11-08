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
    let user = crate::auth::testing::user(&pool, "u", false).await;
    let (_, token) = crate::auth::issue_token(&pool, user.id, crate::auth::TokenKind::Session, "t")
        .await
        .unwrap();
    let covers = tempfile::tempdir().unwrap();
    let app = routes(
        pool.clone(),
        covers.path().join("music"),
        "ffmpeg".into(),
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
            "text3": "#9a8240", "text4": "#9a5c66", "palette": ["#102040", "#f0c040"] });
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
    // Colours worked out before palettes were have an empty one until worked out again.
    sqlx::query(
        "UPDATE music_cover_colours SET colours = json_remove(colours, '$.palette')
         WHERE album_id = 1",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        json_of(&f, "/api/v1/music/albums/1").await.1["colours"]["palette"],
        json!([])
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

#[tokio::test]
async fn an_artist_that_joined_another_answers_as_that_one() {
    let f = fixture(None).await;
    // Gone has no album in the library: an address that leads nowhere until it joins Radiohead.
    let (status, _) = json_of(&f, "/api/v1/music/artists/3").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    sqlx::query("UPDATE music_artists SET merged_into = 1 WHERE id = 3")
        .execute(&f.pool)
        .await
        .unwrap();
    let (status, joined) = json_of(&f, "/api/v1/music/artists/3").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (joined["id"].as_i64(), &joined["name"]),
        (Some(1), &json!("Radiohead"))
    );
    assert_eq!(joined["albums"], 2);
    // A pin of the old address shows the artist it joined.
    let Ok(by_id) = artists_by_id(&f.pool, &crate::access::Access::everything(), &[3, 2]).await
    else {
        panic!("couldn't read the artists");
    };
    assert_eq!(by_id[&3]["id"], 1);
    assert_eq!(by_id[&2]["name"], "Portishead");
}

#[tokio::test]
async fn songs_are_found_by_their_own_title_and_artist() {
    let f = fixture(None).await;
    crate::search_keys::fill(&f.pool).await.unwrap();
    let songs = |q: &str| format!("/api/v1/music/tracks?q_in=track&q={q}");
    // An album's name finds its tracks in the full search, but not as songs.
    let (_, all) = json_of(&f, "/api/v1/music/tracks?q=computer").await;
    assert_eq!(titles(&all), ["Airbag", "Paranoid Android"]);
    let (_, none) = json_of(&f, &songs("computer")).await;
    assert_eq!(none["total"], 0);
    let (_, airbag) = json_of(&f, &songs("AIRBAG")).await;
    assert_eq!(titles(&airbag), ["Airbag"]);
    // The artist counts, the album's own when the track names none.
    let (_, radiohead) = json_of(&f, &songs("radiohead")).await;
    assert_eq!(
        titles(&radiohead),
        ["Airbag", "Paranoid Android", "Everything"]
    );
    let (_, folded) = json_of(&f, &songs("radio%20head")).await;
    assert_eq!(folded["total"], 3, "spaces aside, as search keys have it");
    let bad = "/api/v1/music/tracks?q=x&q_in=album";
    assert_eq!(json_of(&f, bad).await.0, StatusCode::BAD_REQUEST);

    // Someone who may see no music library finds no song and no artist.
    sqlx::raw_sql(
        "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             SELECT id, 0, NULL, 0 FROM users WHERE username = 'u';",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, hidden) = json_of(&f, &songs("airbag")).await;
    assert_eq!(hidden["total"], 0);
    let (_, artists) = json_of(&f, "/api/v1/music/artists?q=radiohead").await;
    assert_eq!(artists["total"], 0);
}

/// The width a JPEG says it is, from its frame header.
fn jpeg_width(bytes: &[u8]) -> Option<u16> {
    let mut i = 2;
    while i + 9 < bytes.len() {
        let marker = bytes[i + 1];
        let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if (0xc0..=0xc2).contains(&marker) {
            return Some(u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]));
        }
        i += 2 + length;
    }
    None
}

#[tokio::test]
async fn covers_come_at_the_size_asked_for_made_once_and_never_enlarged() {
    let f = fixture(None).await;
    let folder_cover = f.library.path().join("r/ok/cover.jpg");
    let made = std::process::Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .args(["color=c=blue:s=1000x1000", "-frames:v", "1"])
        .arg(&folder_cover)
        .status()
        .unwrap();
    assert!(made.success());
    let body = |r: Response| async { r.into_body().collect().await.unwrap().to_bytes().to_vec() };

    let sized = get(&f, "/api/v1/music/albums/1/cover?size=400", None).await;
    assert_eq!(sized.status(), StatusCode::OK);
    assert_eq!(sized.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(jpeg_width(&body(sized).await), Some(400));
    let kept: Vec<_> = std::fs::read_dir(f.covers.path().join("music/sized"))
        .unwrap()
        .collect();
    assert_eq!(kept.len(), 1, "made once and kept");
    let again = get(&f, "/api/v1/music/albums/1/cover?size=400", None).await;
    assert_eq!(jpeg_width(&body(again).await), Some(400));
    assert_eq!(
        std::fs::read_dir(f.covers.path().join("music/sized"))
            .unwrap()
            .count(),
        1
    );
    // Without a size, the cover as it is.
    let whole = get(&f, "/api/v1/music/albums/1/cover", None).await;
    assert_eq!(jpeg_width(&body(whole).await), Some(1000));
    // A cover smaller than the size asked for isn't enlarged: the tags' picture is 8 wide.
    let small = get(&f, "/api/v1/music/albums/2/cover?size=800", None).await;
    assert_eq!(small.status(), StatusCode::OK);
    assert_eq!(jpeg_width(&body(small).await), Some(8));
    // Only the sizes apps draw at.
    assert_eq!(
        get(&f, "/api/v1/music/albums/1/cover?size=123", None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

/// A signed-in user `name`'s token; `libraries`: the only libraries they may see, or `None`
/// for every one.
async fn listener(f: &Fixture, name: &str, libraries: Option<&[i64]>) -> String {
    let user = crate::auth::testing::user(&f.pool, name, false).await;
    if let Some(libraries) = libraries {
        sqlx::query(
            "INSERT INTO user_access (user_id, all_libraries, max_age, updated_at)
             VALUES (?, 0, NULL, 0)",
        )
        .bind(user.id)
        .execute(&f.pool)
        .await
        .unwrap();
        for library in libraries {
            sqlx::query("INSERT INTO user_libraries (user_id, library_id) VALUES (?, ?)")
                .bind(user.id)
                .bind(library)
                .execute(&f.pool)
                .await
                .unwrap();
        }
    }
    crate::auth::issue_token(&f.pool, user.id, crate::auth::TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

async fn json_as(f: &Fixture, token: &str, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = f.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A second music library with one album of one track, Selected (Aphex Twin).
async fn second_library(f: &Fixture) {
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (2, 'more', 'music', '/nowhere');
         INSERT INTO music_albums (id, library_id, key, title, artist_name, year, folder,
             seen_generation) VALUES (5, 2, 'k5', 'Selected', 'Aphex Twin', 1992, 's', 1);
         INSERT INTO music_tracks (library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, genres,
             has_picture) VALUES (2, 's/01.flac', 1, 1, 1, 1, 5, 1, 'Xtal', '[\"Ambient\"]', 0);",
    )
    .execute(&f.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_random_order_pages_through_one_shuffle_by_its_seed() {
    let f = fixture(None).await;
    second_library(&f).await;
    // Without a seed, one is chosen and answered, to page on with.
    let (status, first) = json_of(&f, "/api/v1/music/tracks?sort=random&limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["total"], 5);
    let seed = first["seed"].as_u64().unwrap();
    let page = |offset: u32| {
        format!("/api/v1/music/tracks?sort=random&seed={seed}&limit=2&offset={offset}")
    };
    let (_, again) = json_of(&f, &page(0)).await;
    assert_eq!(
        titles(&again),
        titles(&first),
        "the same seed, the same order"
    );
    let mut all: Vec<String> = Vec::new();
    for offset in [0, 2, 4] {
        let (_, next) = json_of(&f, &page(offset)).await;
        assert_eq!(next["seed"], seed);
        all.extend(titles(&next).iter().map(|t| (*t).to_owned()));
    }
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "Airbag",
            "Everything",
            "Mysterons",
            "Paranoid Android",
            "Xtal"
        ],
        "every track once, none repeated"
    );
    // Other filters still narrow it; other sorts answer no seed.
    let (_, rock) = json_of(&f, "/api/v1/music/tracks?sort=random&genre=rock").await;
    assert_eq!(rock["total"], 3);
    let (_, titled) = json_of(&f, "/api/v1/music/tracks?sort=title").await;
    assert!(titled.get("seed").is_none());
    let bad = "/api/v1/music/tracks?sort=random&seed=-1";
    assert_eq!(json_of(&f, bad).await.0, StatusCode::BAD_REQUEST);

    // Albums shuffle the same way.
    let (_, albums) = json_of(&f, "/api/v1/music/albums?sort=random&limit=10").await;
    assert_eq!(albums["total"], 4, "the missing album stays out");
    let seed = albums["seed"].as_u64().unwrap();
    let (_, two) = json_of(
        &f,
        &format!("/api/v1/music/albums?sort=random&seed={seed}&limit=2"),
    )
    .await;
    let (_, rest) = json_of(
        &f,
        &format!("/api/v1/music/albums?sort=random&seed={seed}&limit=2&offset=2"),
    )
    .await;
    let paged: Vec<&str> = titles(&two).into_iter().chain(titles(&rest)).collect();
    assert_eq!(paged, titles(&albums));
}

#[tokio::test]
async fn a_random_order_keeps_to_what_the_listener_sees_and_wants() {
    let f = fixture(None).await;
    second_library(&f).await;
    // Not interested in Mysterons: it stays out of a shuffle, though the list still shows it.
    sqlx::query(
        "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
             payload, created_at, received_at)
         SELECT 'f1', u.id, 'phone', 'track_flags', 'flag', 'upsert',
             json_object('jellyfinId', CAST(t.id AS TEXT), 'notInterested', json('true')), 1, 1
         FROM users u, music_tracks t WHERE u.username = 'u' AND t.title = 'Mysterons'",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, shuffled) = json_of(&f, "/api/v1/music/tracks?sort=random").await;
    assert_eq!(shuffled["total"], 4);
    assert!(!titles(&shuffled).contains(&"Mysterons"));
    let (_, listed) = json_of(&f, "/api/v1/music/tracks?sort=title").await;
    assert!(titles(&listed).contains(&"Mysterons"));

    // Someone else's flag is theirs alone; and someone who may see only the second library
    // shuffles only what's in it.
    let other = listener(&f, "v", Some(&[2])).await;
    let (_, theirs) = json_as(&f, &other, "/api/v1/music/tracks?sort=random").await;
    assert_eq!(titles(&theirs), ["Xtal"]);
    let (_, albums) = json_as(&f, &other, "/api/v1/music/albums?sort=random").await;
    assert_eq!(titles(&albums), ["Selected"]);
    let everyone = listener(&f, "w", None).await;
    let (_, all) = json_as(&f, &everyone, "/api/v1/music/tracks?sort=random").await;
    assert_eq!(all["total"], 5, "u's Not interested isn't w's");
    let nobody = listener(&f, "x", Some(&[])).await;
    let (_, none) = json_as(&f, &nobody, "/api/v1/music/tracks?sort=random").await;
    assert_eq!(none["total"], 0);
    let (_, none) = json_as(&f, &nobody, "/api/v1/music/albums?sort=random").await;
    assert_eq!(none["total"], 0);
}

#[tokio::test]
async fn songs_and_albums_narrow_to_several_genres_a_span_of_years_and_likes() {
    let f = fixture(None).await;
    second_library(&f).await;
    // Any of several genres, in any case.
    let (_, two) = json_of(
        &f,
        "/api/v1/music/tracks?genres=ART%20ROCK;Trip%20Hop&sort=title",
    )
    .await;
    assert_eq!(titles(&two), ["Mysterons", "Paranoid Android"]);
    let (_, albums) = json_of(
        &f,
        "/api/v1/music/albums?genres=trip%20hop;ambient&sort=title",
    )
    .await;
    assert_eq!(titles(&albums), ["Dummy", "Selected"]);
    // A span of years, either end open: Dummy (1994), OK Computer (1997), Kid A (2000),
    // Selected (1992).
    let (_, nineties) = json_of(
        &f,
        "/api/v1/music/albums?year_from=1993&year_to=1999&sort=title",
    )
    .await;
    assert_eq!(titles(&nineties), ["Dummy", "OK Computer"]);
    let (_, late) = json_of(&f, "/api/v1/music/tracks?year_from=1997&sort=title").await;
    assert_eq!(titles(&late), ["Airbag", "Everything", "Paranoid Android"]);
    let (_, early) = json_of(
        &f,
        "/api/v1/music/tracks?year_to=1994&genres=rock;trip%20hop",
    )
    .await;
    assert_eq!(titles(&early), ["Mysterons"], "filters combine");
    let bad = "/api/v1/music/tracks?year_from=2000&year_to=1990";
    assert_eq!(json_of(&f, bad).await.0, StatusCode::BAD_REQUEST);

    // Only the songs the listener likes, theirs alone.
    let me: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'u'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let airbag: i64 = sqlx::query_scalar("SELECT id FROM music_tracks WHERE title = 'Airbag'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let kind = crate::aoide::likes::Kind::Track;
    assert!(
        crate::aoide::likes::set(&f.pool, kind, me, "web", airbag, true, 1)
            .await
            .unwrap()
    );
    let (_, liked) = json_of(&f, "/api/v1/music/tracks?liked=true").await;
    assert_eq!(titles(&liked), ["Airbag"]);
    let (_, all) = json_of(&f, "/api/v1/music/tracks?liked=false").await;
    assert_eq!(all["total"], 5);
    let other = listener(&f, "v", None).await;
    let (_, none) = json_as(&f, &other, "/api/v1/music/tracks?liked=true").await;
    assert_eq!(none["total"], 0, "u's likes aren't v's");

    // As with every list, a song marked Not interested is still listed.
    sqlx::query(
        "INSERT INTO aoide_ops (op_id, user_id, device_id, entity, entity_id, operation,
             payload, created_at, received_at)
         VALUES ('f1', ?, 'phone', 'track_flags', 'flag', 'upsert',
             json_object('jellyfinId', ?, 'notInterested', json('true')), 1, 1)",
    )
    .bind(me)
    .bind(airbag.to_string())
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, still) = json_of(&f, "/api/v1/music/tracks?liked=true&genres=rock").await;
    assert_eq!(titles(&still), ["Airbag"]);

    // Someone who may see only the second library finds only what's in it.
    let limited = listener(&f, "w", Some(&[2])).await;
    let (_, theirs) = json_as(&f, &limited, "/api/v1/music/tracks?genres=ambient;rock").await;
    assert_eq!(titles(&theirs), ["Xtal"]);
    let (_, theirs) = json_as(&f, &limited, "/api/v1/music/albums?year_to=1999").await;
    assert_eq!(titles(&theirs), ["Selected"]);
}

#[tokio::test]
async fn albums_carry_their_release_groups_types_from_the_match() {
    let f = fixture(None).await;
    sqlx::raw_sql(
        "INSERT INTO mb_releases (id, release_group_id, title, artist_credit, fetched_at,
             primary_type, secondary_types)
             VALUES ('ep', 'group-ep', 'Kid A', '[]', 0, 'EP', '[\"Live\"]');
         UPDATE music_albums SET match_release_id = 'ep' WHERE title = 'Kid A';
         UPDATE mb_releases SET primary_type = 'Album' WHERE id = 'rel';",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    let (_, page) = json_of(&f, "/api/v1/music/albums?sort=title").await;
    let types: Vec<(&str, &Value, &Value)> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["title"].as_str().unwrap(),
                &a["primary_type"],
                &a["secondary_types"],
            )
        })
        .collect();
    assert_eq!(
        types,
        [
            ("Dummy", &Value::Null, &json!([])),
            ("Kid A", &json!("EP"), &json!(["Live"])),
            ("OK Computer", &json!("Album"), &json!([])),
        ],
        "an album not matched has no type"
    );
    // On an artist's page too, and on the album's own.
    let (_, artist) = json_of(&f, "/api/v1/music/artists/1").await;
    let kid_a = artist["album_list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["title"] == "Kid A")
        .unwrap();
    assert_eq!(kid_a["primary_type"], "EP");
    let (_, album) = json_of(&f, "/api/v1/music/albums/3").await;
    assert_eq!(album["secondary_types"], json!(["Live"]));
}

mod search_covers;
