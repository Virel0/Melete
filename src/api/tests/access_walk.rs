//! A user an admin has restricted reaches nothing outside their access by any route. Every
//! title they may not see is called "Zyxveil …", a word found nowhere else, and its artwork is
//! named so too; the walk asks every documented route a signed-in user may use, with every id
//! there is, and no answer may mention one.

use super::openapi::{full_server, spec};
use super::*;
use crate::access::{Restriction, set_restriction};

/// Routes whose answers aren't walked: streams that don't end, and the speed test's bytes.
const NOT_WALKED: &[&str] = &[
    "/api/v1/events",
    "/api/v1/requests/events",
    "/api/v1/speedtest",
];

/// Every id a route might be asked about: the visible titles' and files' and the hidden ones'.
const IDS: &[i64] = &[
    1, 2, 3, 11, 12, 13, 14, 21, 22, 23, 31, 32, 33, 34, 35, 36, 41,
];

/// Titles by their stable ids, for the routes that take those.
const STABLE_IDS: &[&str] = &[
    "tmdb:movie:101",
    "tmdb:movie:111",
    "tmdb:movie:112",
    "tmdb:movie:114",
    "tmdb:tv:201",
    "tmdb:tv:221",
    "tmdb:tv:222",
];

/// Extra query strings for routes that need some to answer.
const QUERIES: &[(&str, &str)] = &[
    ("/api/v1/movies", "?q=zyxveil"),
    ("/api/v1/series", "?q=zyxveil"),
    ("/api/v1/calendar", "?start=2030-01-01&end=2030-01-31"),
    ("/api/v1/music/albums", "?q=zyxveil"),
    ("/api/v1/music/tracks", "?q=zyxveil"),
    ("/api/v1/music/artists", "?q=zyxveil"),
];

/// Music: library 5 (open to the kid) and 6 (not). Artists 1 (open) and 2 (only in library 6).
/// Albums 1 (open) and 41 (hidden), tracks 1 and 41 on them; the kid has played both and pinned
/// both albums. The hidden track credits the open artist and a guest, 3, credited nowhere else,
/// so neither the guest nor the album the open artist appears on there may show.
const MUSIC: &str = r#"
INSERT INTO libraries (id, name, kind, path) VALUES (5, 'music', 'music', '/a'),
    (6, 'grownmusic', 'music', '/ga');
INSERT INTO music_artists (id, key, name) VALUES (1, 'open', 'Open Artist'),
    (2, 'zyxveil', 'Zyxveil Artist'), (3, 'zyxveil-guest', 'Zyxveil Guest');
INSERT INTO music_albums (id, library_id, key, title, artist_name, artist_id, folder,
    seen_generation) VALUES
    (1, 5, 'k1', 'Open Album', 'Open Artist', 1, 'open', 1),
    (41, 6, 'k41', 'Zyxveil Album', 'Zyxveil Artist', 2, 'zyxveil', 1);
INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at, first_seen_generation,
    seen_generation, album_id, tagged, title, genres) VALUES
    (1, 5, 'open/1.flac', 1, 1, 1, 1, 1, 1, 'Open Song', '["Pop"]'),
    (41, 6, 'zyxveil/1.flac', 1, 1, 1, 1, 41, 1, 'Zyxveil Song', '["Zyxveil Genre"]');
INSERT INTO music_track_artists (track_id, position, artist_id, name) VALUES
    (1, 0, 1, 'Open Artist'), (41, 0, 2, 'Zyxveil Artist'), (41, 1, 1, 'Open Artist'),
    (41, 2, 3, 'Zyxveil Guest');
INSERT INTO music_plays (user_id, source, event_id, track_key, track_id, device, started_at,
    completed, skipped)
SELECT u.id, 'api', 'p' || t.id, t.id, t.id, 'phone', 1, 1, 0
FROM users u, music_tracks t WHERE u.username = 'kid';
INSERT INTO music_pin_lists (user_id, version, updated_at)
SELECT id, 1, 0 FROM users WHERE username = 'kid';
INSERT INTO music_pins (user_id, position, kind, item)
SELECT u.id, a.id > 1, 'album', a.id FROM users u, music_albums a WHERE u.username = 'kid';
"#;

/// Libraries: 1 movies and 2 shows (open to the kid), 3 grown-up movies and 4 grown-up shows
/// (not). Movies: 1 open (rated 7); 11 in the grown-up library (rated 0); 12 rated 17; 13 never
/// matched; 14 rated with a certification Melete can't read. Series: 2 open (rated 7); 21 in
/// the grown-up library; 22 rated 16. Files: 1 of movie 1, 2 an episode of series 2, 31 of
/// movie 11, 32 of movie 12, 33 an episode of series 21, 34 of series 22.
const WORLD: &str = r#"
INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
    (2, 'shows', 'shows', '/s'), (3, 'grown', 'movies', '/g'), (4, 'grownshows', 'shows', '/gs');
INSERT INTO tmdb_movies (tmdb_id, title, year, poster_path, backdrop_path, genres, fetched_at) VALUES
    (101, 'Open Movie', 2001, '/open.jpg', '/open-b.jpg', '["Drama"]', 0),
    (111, 'Zyxveil Grown Movie', 2001, '/zyxveil-111.jpg', '/zyxveil-111-b.jpg', '["Zyxveil Genre"]', 0),
    (112, 'Zyxveil Rated Movie', 2001, '/zyxveil-112.jpg', NULL, '["Zyxveil Genre"]', 0),
    (114, 'Zyxveil Unreadable Movie', 2001, '/zyxveil-114.jpg', NULL, '[]', 0);
INSERT INTO tmdb_series (tmdb_id, name, year, poster_path, genres, fetched_at) VALUES
    (201, 'Open Series', 2001, '/open-s.jpg', '["Drama"]', 0),
    (221, 'Zyxveil Grown Series', 2001, '/zyxveil-221.jpg', '["Zyxveil Genre"]', 0),
    (222, 'Zyxveil Rated Series', 2001, '/zyxveil-222.jpg', '["Zyxveil Genre"]', 0);
INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, still_path) VALUES
    (201, 1, 1, 9201, 'Open Pilot', '/open-e.jpg'),
    (221, 1, 1, 9221, 'Zyxveil Pilot', '/zyxveil-e221.jpg'),
    (222, 1, 1, 9222, 'Zyxveil Rated Pilot', '/zyxveil-e222.jpg');
INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at) VALUES
    (0, 101, '{"DE": "6"}', 6, 0), (0, 111, '{"DE": "0"}', 0, 0), (0, 112, '{"US": "R"}', 17, 0),
    (0, 114, '{"US": "NR"}', NULL, 0), (1, 201, '{"US": "TV-Y7"}', 7, 0),
    (1, 221, '{"US": "TV-G"}', 0, 0), (1, 222, '{"DE": "16"}', 16, 0);
INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, match_status,
    match_tmdb_id) VALUES
    (1, 1, 'Open Movie (2001)', 'Open Movie', 1, 'matched', 101),
    (11, 3, 'Zyxveil Grown Movie (2001)', 'Zyxveil Grown Movie', 1, 'matched', 111),
    (12, 1, 'Zyxveil Rated Movie (2001)', 'Zyxveil Rated Movie', 1, 'matched', 112),
    (13, 1, 'Zyxveil Unmatched Movie (2001)', 'Zyxveil Unmatched Movie', 1, NULL, NULL),
    (14, 1, 'Zyxveil Unreadable Movie (2001)', 'Zyxveil Unreadable Movie', 1, 'matched', 114);
INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation, match_status,
    match_tmdb_id) VALUES
    (2, 2, 'Open Series', 'Open Series', 1, 'matched', 201),
    (21, 4, 'Zyxveil Grown Series', 'Zyxveil Grown Series', 1, 'matched', 221),
    (22, 2, 'Zyxveil Rated Series', 'Zyxveil Rated Series', 1, 'matched', 222);
INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id, series_id, season,
    episode, first_seen_generation, seen_generation) VALUES
    (1, 1, 'Open Movie (2001)/open.mkv', 5, 1, 1, NULL, NULL, NULL, 1, 1),
    (2, 2, 'Open Series/S01E01.mkv', 5, 1, NULL, 2, 1, 1, 1, 1),
    (31, 3, 'Zyxveil Grown Movie (2001)/zyxveil.mkv', 5, 1, 11, NULL, NULL, NULL, 1, 1),
    (32, 1, 'Zyxveil Rated Movie (2001)/zyxveil.mkv', 5, 1, 12, NULL, NULL, NULL, 1, 1),
    (33, 4, 'Zyxveil Grown Series/S01E01.mkv', 5, 1, NULL, 21, 1, 1, 1, 1),
    (34, 2, 'Zyxveil Rated Series/S01E01.mkv', 5, 1, NULL, 22, 1, 1, 1, 1),
    (35, 2, 'Zyxveil Rated Series/S01E02.mkv', 5, 1, NULL, 22, 1, 2, 1, 1),
    (36, 4, 'Zyxveil Grown Series/S01E02.mkv', 5, 1, NULL, 21, 1, 2, 1, 1);
INSERT INTO tmdb_episodes (series_tmdb_id, season, episode, tmdb_id, name, air_date) VALUES
    (221, 1, 2, 9223, 'Zyxveil Airing', '2030-01-10'),
    (222, 1, 2, 9224, 'Zyxveil Rated Airing', '2030-01-11');
UPDATE tmdb_movies SET digital_release = '2030-01-12' WHERE tmdb_id IN (101, 111, 112, 114);
INSERT INTO wanted_movies (id, tmdb_id, title, monitored, movie_id, source, added_at) VALUES
    (1, 101, 'Open Movie', 1, 1, 'manual', 0),
    (11, 111, 'Zyxveil Grown Movie', 1, 11, 'manual', 0),
    (12, 112, 'Zyxveil Rated Movie', 1, 12, 'manual', 0),
    (14, 114, 'Zyxveil Unreadable Movie', 1, NULL, 'manual', 0);
INSERT INTO wanted_series (id, tvdb_id, tmdb_id, title, monitored, series_type, series_id,
    source, added_at) VALUES
    (21, 5221, 221, 'Zyxveil Grown Series', 1, 'standard', 21, 'manual', 0),
    (22, 5222, 222, 'Zyxveil Rated Series', 1, 'standard', 22, 'manual', 0);
"#;

/// The kid has watched part of every movie, and the first episode of every series, so what to
/// continue and watch next would show them all.
const WATCHED: &str = "
INSERT INTO playback_progress (user_id, file_id, position_seconds, duration_seconds, watched,
    play_count, updated_at)
SELECT u.id, f.id, CASE WHEN f.episode = 1 THEN 0 ELSE 30 END, 100,
    coalesce(f.episode = 1, 0), coalesce(f.episode = 1, 0), 1
FROM users u, media_files f
WHERE u.username = 'kid' AND coalesce(f.episode, 1) = 1;";

struct World {
    pool: SqlitePool,
    app: Router,
    /// The kid's token: libraries 1 and 2, rated up to 12.
    kid: String,
    _dir: tempfile::TempDir,
}

async fn world() -> World {
    world_with(full_server().await).await
}

/// The world, on a server already made.
async fn world_with((pool, app, dir): (SqlitePool, Router, tempfile::TempDir)) -> World {
    sqlx::raw_sql(WORLD).execute(&pool).await.unwrap();
    let kid = crate::auth::create_user(&pool, "kid", "a-long-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    sqlx::raw_sql(MUSIC).execute(&pool).await.unwrap();
    sqlx::raw_sql(titles_own::ROWS)
        .execute(&pool)
        .await
        .unwrap();
    let restriction = Restriction {
        libraries: Some(vec![1, 2, 5]),
        max_age: Some(12),
    };
    set_restriction(&pool, kid.id, Some(&restriction))
        .await
        .unwrap();
    // Every hidden title is someone's favourite, so lists of favourites would show them.
    sqlx::raw_sql(
        "INSERT INTO favourites (user_id, movie_id, series_id, added_at)
         SELECT u.id, f.movie, f.series, 1 FROM users u, (SELECT 1 AS movie, NULL AS series
             UNION ALL SELECT 11, NULL UNION ALL SELECT 12, NULL UNION ALL SELECT 13, NULL
             UNION ALL SELECT NULL, 2 UNION ALL SELECT NULL, 21 UNION ALL SELECT NULL, 22) f
         WHERE u.username = 'kid';",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(WATCHED).execute(&pool).await.unwrap();
    let (_, kid) = crate::auth::issue_token(&pool, kid.id, crate::auth::TokenKind::Session, "t")
        .await
        .unwrap();
    World {
        pool,
        app,
        kid,
        _dir: dir,
    }
}

/// The status and body, as text, of a request as the kid.
async fn ask(w: &World, method: &str, uri: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", w.kid))
        .body(Body::empty())
        .unwrap();
    let response = w.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Every concrete address to ask for a documented path: one per id where it takes one.
fn addresses(path: &str) -> Vec<String> {
    let query = QUERIES
        .iter()
        .find(|(p, _)| *p == path)
        .map_or("", |(_, q)| *q);
    let fill = |id: &str| {
        path.replace("{id}", id)
            .replace("{season}", "1")
            .replace("{size}", "w342")
            .replace("{file}", "zyxveil-111.jpg")
            .split('/')
            .map(|part| {
                if part.starts_with('{') {
                    "x".to_owned()
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("/")
            + query
    };
    if path.starts_with("/api/v1/items/") || path.starts_with("/api/v1/discover/similar/") {
        STABLE_IDS.iter().map(|id| fill(id)).collect()
    } else if path.contains("{id}") {
        IDS.iter().map(|id| fill(&id.to_string())).collect()
    } else {
        vec![fill("")]
    }
}

#[tokio::test]
async fn no_route_shows_a_restricted_user_what_they_may_not_see() {
    let w = world().await;
    let spec = spec();
    let mut asked = 0;
    for (path, operations) in spec["paths"].as_object().unwrap() {
        if NOT_WALKED.contains(&path.as_str()) {
            continue;
        }
        let Some(op) = operations.get("get") else {
            continue;
        };
        if op["tags"][0] == "Admin" {
            continue;
        }
        for uri in addresses(path) {
            let (status, body) = ask(&w, "GET", &uri).await;
            assert!(
                !body.to_lowercase().contains("zyxveil"),
                "GET {uri} ({status}) shows a hidden title: {body}"
            );
            asked += 1;
        }
    }
    assert!(asked > 50, "{asked}");
}

#[tokio::test]
async fn hidden_titles_are_not_there_and_open_ones_are() {
    let w = world().await;
    for id in [11, 12, 13, 14] {
        for uri in [
            format!("/api/v1/movies/{id}"),
            format!("/api/v1/movies/{id}/artwork"),
            format!("/api/v1/me/favourites/movies/{id}"),
        ] {
            assert_eq!(ask(&w, "GET", &uri).await.0, StatusCode::NOT_FOUND, "{uri}");
        }
        let uri = format!("/api/v1/me/favourites/movies/{id}");
        assert_eq!(ask(&w, "PUT", &uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    for id in [21, 22] {
        for uri in [
            format!("/api/v1/series/{id}"),
            format!("/api/v1/series/{id}/artwork"),
            format!("/api/v1/series/{id}/seasons/1"),
            format!("/api/v1/me/favourites/series/{id}"),
        ] {
            assert_eq!(ask(&w, "GET", &uri).await.0, StatusCode::NOT_FOUND, "{uri}");
        }
    }
    let (_, movies) = ask(&w, "GET", "/api/v1/movies").await;
    assert!(movies.contains("Open Movie"), "{movies}");
    let (_, series) = ask(&w, "GET", "/api/v1/series/2/seasons/1").await;
    assert!(series.contains("Open Pilot"), "{series}");
    let (_, libraries) = ask(&w, "GET", "/api/v1/browse/libraries").await;
    let libraries: Value = serde_json::from_str(&libraries).unwrap();
    let names: Vec<&str> = libraries
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["movies", "shows"]);
    assert_eq!(libraries[0]["titles"], 1);
    let (_, favourites) = ask(&w, "GET", "/api/v1/me/favourites").await;
    let favourites: Value = serde_json::from_str(&favourites).unwrap();
    assert_eq!(favourites["total"], 2);
    // An admin sees them all.
    let admin = crate::auth::create_user(&w.pool, "parent", "a-long-password".into(), true)
        .await
        .unwrap()
        .unwrap();
    let access = crate::access::Access::of(&w.pool, admin.id, true)
        .await
        .unwrap();
    assert!(access.sees_movie(&w.pool, 12).await.unwrap());
}

/// The walk means something only if the same routes show these titles to someone who may see
/// them: the kid, once nothing is restricted.
#[tokio::test]
async fn without_the_restriction_the_same_routes_show_them() {
    let w = world().await;
    lift(&w).await;
    for uri in [
        "/api/v1/movies?q=zyxveil",
        "/api/v1/series/22/seasons/1",
        "/api/v1/movies/12",
        "/api/v1/me/continue",
        "/api/v1/me/next-up",
        "/api/v1/me/favourites",
        "/api/v1/calendar?start=2030-01-01&end=2030-01-31",
        "/api/v1/items/tmdb:movie:112",
        "/api/v1/music/albums/41",
        "/api/v1/music/artists?q=zyxveil",
        "/api/v1/music/tracks?q=zyxveil",
        "/api/v1/music/genres",
        "/api/v1/music/plays",
        "/api/v1/music/pins",
        "/api/v1/music/home",
        "/api/v1/home",
    ] {
        let (status, body) = ask(&w, "GET", uri).await;
        assert!(
            body.to_lowercase().contains("zyxveil"),
            "GET {uri} ({status}): {body}"
        );
    }
}

/// Takes the kid's restriction away, or with `max_age` puts one back.
async fn lift(w: &World) {
    restrict(w, None).await;
}

async fn restrict(w: &World, restriction: Option<Restriction>) {
    let kid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'kid'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    set_restriction(&w.pool, kid, restriction.as_ref())
        .await
        .unwrap();
}

/// The status of a request with a JSON body, as the kid.
async fn send_json(w: &World, method: &str, uri: &str, body: Value) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {}", w.kid))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    w.app.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn hidden_files_cant_be_played_or_changed_and_their_links_stop() {
    let w = world().await;
    for id in [31, 32, 33, 34, 35] {
        for (method, uri) in [
            ("GET", format!("/api/v1/files/{id}")),
            ("GET", format!("/api/v1/files/{id}/subtitles")),
            ("GET", format!("/api/v1/files/{id}/segments")),
            ("POST", format!("/api/v1/files/{id}/play")),
        ] {
            let status = ask(&w, method, &uri).await.0;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        }
        let changes = [
            ("progress", json!({ "position_seconds": 1.0 })),
            ("watched", json!({ "watched": true })),
            (
                "track-choice",
                json!({ "audio_language": null, "subtitle": null }),
            ),
        ];
        for (route, body) in changes {
            let uri = format!("/api/v1/files/{id}/{route}");
            let status = send_json(&w, "PUT", &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "PUT {uri}");
        }
        let uri = format!("/api/v1/files/{id}/transcode");
        let status = send_json(&w, "POST", &uri, json!({ "video_codec": "h264" })).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "POST {uri}");
    }
    // A link made while the file was open stops once it isn't.
    lift(&w).await;
    let (status, play) = ask(&w, "POST", "/api/v1/files/32/play").await;
    assert_eq!(status, StatusCode::OK, "{play}");
    let play: Value = serde_json::from_str(&play).unwrap();
    restrict(
        &w,
        Some(Restriction {
            libraries: None,
            max_age: Some(12),
        }),
    )
    .await;
    let link = play["stream_url"].as_str().unwrap();
    assert_eq!(ask(&w, "GET", link).await.0, StatusCode::NOT_FOUND);
    let subtitles = format!("{link}/subtitles/x1.vtt");
    assert_eq!(ask(&w, "GET", &subtitles).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn only_artwork_of_what_they_may_see_is_served_cached_or_not() {
    let w = world().await;
    let cached = w._dir.path().join("artwork").join("w342");
    std::fs::create_dir_all(&cached).unwrap();
    for file in ["open.jpg", "zyxveil-111.jpg", "zyxveil-112.jpg"] {
        std::fs::write(cached.join(file), b"jpeg").unwrap();
    }
    let open = ask(&w, "GET", "/api/v1/artwork/w342/open.jpg").await.0;
    assert_eq!(open, StatusCode::OK);
    for file in ["zyxveil-111.jpg", "zyxveil-112.jpg", "zyxveil-221.jpg"] {
        let uri = format!("/api/v1/artwork/w342/{file}");
        assert_eq!(ask(&w, "GET", &uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    let still = "/api/v1/artwork/w300/zyxveil-e221.jpg";
    assert_eq!(ask(&w, "GET", still).await.0, StatusCode::NOT_FOUND);
}

mod files_own;
mod music_and_keys;
mod outside;
mod titles_own;
