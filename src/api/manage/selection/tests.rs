use axum::body::Body;
use axum::extract::Request;
use axum::http::header;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;
use crate::musicbrainz::testing as mb;
use crate::tmdb::testing::{FakeTmdb, Reply};

struct Rig {
    pool: SqlitePool,
    app: Router,
    admin: String,
    viewer: String,
}

/// Two movies libraries; wanted movies 1 (monitored, in the library), 2 (not monitored) and 3
/// (with a download on its way); library-only folders 10 (matched, in the second library) and
/// 11 (unmatched); a library-only artist with a MusicBrainz id, and one without.
async fn rig(musicbrainz: bool) -> Rig {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES
             (1, 'Films', 'movies', '/m'), (2, 'More films', 'movies', '/n'), (3, 'Music', 'music', '/a');
         INSERT INTO quality_profiles (id, source, name, upgrade_allowed, cutoff, min_format_score,
                 cutoff_format_score, min_upgrade_format_score, updated_at) VALUES
             (7, 'radarr', 'HD', 1, 'Bluray-1080p', 0, 0, 1, 0),
             (8, 'sonarr', 'TV', 1, 'HDTV-720p', 0, 0, 1, 0);
         INSERT INTO tmdb_movies (tmdb_id, title, release_date, year, fetched_at) VALUES
             (603, 'The Matrix', '1999-03-30', 1999, 0);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation, match_tmdb_id,
                 match_status) VALUES
             (1, 1, 'Alien (1979)', 'Alien', 1, NULL, NULL),
             (10, 2, 'The Matrix (1999)', 'The Matrix', 1, 603, 'matched'),
             (11, 1, 'Home video', 'Home video', 1, NULL, NULL);
         INSERT INTO media_files (library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation) VALUES
             (1, 'Alien (1979)/Alien.mkv', 5, 0, 1, 1, 1),
             (2, 'The Matrix (1999)/The Matrix.mkv', 5, 0, 10, 1, 1),
             (1, 'Home video/Home video.mkv', 5, 0, 11, 1, 1);
         INSERT INTO wanted_movies (id, tmdb_id, title, monitored, library_id, movie_id, source,
                 added_at) VALUES
             (1, 348, 'Alien', 1, 1, 1, 'manual', 0),
             (2, 999, 'Brazil', 0, 1, NULL, 'manual', 0),
             (3, 998, 'Clue', 1, 1, NULL, 'manual', 0);
         INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, progress,
                 added_at, updated_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'Clue.1985.1080p', 3, '{}',
                     'downloading', 10, 0, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    music(&pool).await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603",
        vec![Reply::ok(
            json!({"id": 603, "title": "The Matrix", "release_date": "1999-03-30"}),
        )],
    );
    let tmdb = Arc::new(fake.start().await);
    let musicbrainz = if musicbrainz {
        Some(Arc::new(mb::start(0).await.0))
    } else {
        None
    };
    let token = |name: &'static str, admin: bool| {
        let pool = pool.clone();
        async move {
            let user = auth::create_user(&pool, name, format!("{name}-password"), admin)
                .await
                .unwrap()
                .unwrap();
            auth::issue_token(&pool, user.id, TokenKind::Session, "t")
                .await
                .unwrap()
                .1
        }
    };
    Rig {
        app: selection_routes(pool.clone(), Some(tmdb), musicbrainz),
        admin: token("alice", true).await,
        viewer: token("bob", false).await,
        pool,
    }
}

/// Artist 1 has Radiohead's MusicBrainz id (the fake's); artist 2 none.
async fn music(pool: &SqlitePool) {
    sqlx::raw_sql(
        "INSERT INTO music_artists (id, key, name, mb_artist_id) VALUES
             (1, 'radiohead', 'Radiohead', 'a74b1b7f-71a5-4011-9441-d0b5e4122711'),
                    (2, 'tapes', 'Tapes', NULL);
         INSERT INTO music_albums (id, library_id, key, title, artist_id, artist_name, folder,
                 seen_generation) VALUES
             (1, 3, 'k:1', 'Pablo Honey', 1, 'Radiohead', 'Radiohead/Pablo Honey', 1),
             (2, 3, 'k:2', 'One', 2, 'Tapes', 'Tapes/One', 1);
         INSERT INTO music_tracks (library_id, rel_path, size, modified_at, first_seen_generation,
                 seen_generation, album_id, tagged, title) VALUES
             (3, 'Radiohead/Pablo Honey/01.flac', 5, 0, 1, 1, 1, 1, 'Creep'),
             (3, 'Tapes/One/01.flac', 5, 0, 1, 1, 2, 1, 'Side A');",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn post(r: &Rig, service: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::post(format!("/api/v1/manage/{service}/selection"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = r.app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn wanted(pool: &SqlitePool) -> Vec<(i64, bool, Option<i64>, i64, Option<i64>)> {
    sqlx::query_as(
        "SELECT tmdb_id, monitored, profile_id, library_id, movie_id FROM wanted_movies
         ORDER BY tmdb_id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn wanted_titles_are_changed_together_and_each_failure_says_why() {
    let r = rig(false).await;
    let (status, body) = post(
        &r,
        "movies",
        &r.admin,
        json!({"action": "unmonitor", "wanted_ids": [1, 2, 3, 99]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], 3);
    assert_eq!(
        body["unchanged"],
        json!([{"wanted_id": 99, "folder": null, "why": "no such wanted entry"}])
    );
    let (_, body) = post(
        &r,
        "movies",
        &r.admin,
        json!({"action": "monitor", "wanted_ids": [1, 2]}),
    )
    .await;
    assert_eq!(body["changed"], 2);
    let (_, body) = post(
        &r,
        "movies",
        &r.admin,
        json!({"action": "profile", "profile_id": 7, "wanted_ids": [1, 2]}),
    )
    .await;
    assert_eq!(body["changed"], 2);
    assert_eq!(
        wanted(&r.pool).await,
        [
            (348, true, Some(7), 1, Some(1)),
            (998, false, None, 1, None),
            (999, true, Some(7), 1, None)
        ]
    );
    // Taken off, files kept; the one with a download on its way stays, with why.
    let (_, body) = post(
        &r,
        "movies",
        &r.admin,
        json!({"action": "remove", "wanted_ids": [1, 3]}),
    )
    .await;
    assert_eq!(body["changed"], 1);
    assert_eq!(
        body["unchanged"][0]["why"],
        "1 download(s) for it are on their way; remove them under Downloads first"
    );
    assert_eq!(
        wanted(&r.pool)
            .await
            .iter()
            .map(|w| w.0)
            .collect::<Vec<_>>(),
        [998, 999]
    );
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM media_files")
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(files, 3);
}

#[tokio::test]
async fn monitor_wants_titles_only_in_the_library_where_they_are() {
    let r = rig(false).await;
    let (status, body) = post(
        &r,
        "movies",
        &r.admin,
        json!({"action": "monitor", "profile_id": 7, "wanted_ids": [2], "folders": [10, 11, 1]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], 2, "{body}");
    let matrix = sqlx::query_scalar::<_, i64>("SELECT id FROM wanted_movies WHERE tmdb_id = 603")
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(body["wanted"], json!([{"folder": 10, "wanted_id": matrix}]));
    assert_eq!(
        body["unchanged"],
        json!([
            {"wanted_id": null, "folder": 11, "why": "Melete doesn't know which title it is; fix its match first"},
            {"wanted_id": null, "folder": 1, "why": "it isn't a title only in the library"}
        ])
    );
    // In the second library, where its folder is, with the profile, monitored, and no
    // request of its own.
    assert!(
        wanted(&r.pool)
            .await
            .contains(&(603, true, Some(7), 2, Some(10)))
    );
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM requests")
        .fetch_one(&r.pool)
        .await
        .unwrap();
    assert_eq!(requests, 0);
    // Once wanted, it's no longer a title only in the library.
    let (_, body) = post(
        &r,
        "movies",
        &r.admin,
        json!({"action": "monitor", "folders": [10]}),
    )
    .await;
    assert_eq!(body["changed"], 0);
}

#[tokio::test]
async fn artists_only_in_the_library_are_wanted_through_musicbrainz() {
    let r = rig(false).await;
    let (_, body) = post(
        &r,
        "artists",
        &r.admin,
        json!({"action": "monitor", "folders": [1, 2]}),
    )
    .await;
    assert_eq!(body["changed"], 0);
    assert_eq!(
        body["unchanged"],
        json!([
            {"wanted_id": null, "folder": 1, "why": "music is looked up on MusicBrainz, which isn't set up (MELETE_MUSICBRAINZ=on)"},
            {"wanted_id": null, "folder": 2, "why": "its files don't say which MusicBrainz artist it is"}
        ])
    );
    let r = rig(true).await;
    let (_, body) = post(
        &r,
        "artists",
        &r.admin,
        json!({"action": "monitor", "folders": [1]}),
    )
    .await;
    assert_eq!(body["changed"], 1, "{body}");
    let artist: (String, bool, i64, String) = sqlx::query_as(
        "SELECT mb_artist_id, monitored, library_id, monitor_new FROM wanted_artists",
    )
    .fetch_one(&r.pool)
    .await
    .unwrap();
    assert_eq!(artist, (mb::RADIOHEAD.into(), true, 3, "all".into()));
}

#[tokio::test]
async fn selections_are_checked_and_for_admins_only() {
    let r = rig(false).await;
    let many: Vec<i64> = (1..=101).collect();
    for (service, body, status) in [
        (
            "movies",
            json!({"action": "monitor", "wanted_ids": many}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "movies",
            json!({"action": "monitor"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "movies",
            json!({"action": "remove", "folders": [10]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "movies",
            json!({"action": "profile", "wanted_ids": [1]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "movies",
            json!({"action": "remove", "profile_id": 7, "wanted_ids": [1]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "movies",
            json!({"action": "watch", "wanted_ids": [1]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            "movies",
            json!({"action": "profile", "profile_id": 8, "wanted_ids": [1]}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "series",
            json!({"action": "profile", "profile_id": 7, "wanted_ids": [1]}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "music",
            json!({"action": "monitor", "wanted_ids": [1]}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (got, answer) = post(&r, service, &r.admin, body.clone()).await;
        assert_eq!(got, status, "{service} {body}: {answer}");
    }
    let (status, _) = post(
        &r,
        "movies",
        &r.viewer,
        json!({"action": "monitor", "wanted_ids": [1]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(wanted(&r.pool).await[0], (348, true, None, 1, Some(1)));
}
