use axum::body::Body;
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::auth::{self, TokenKind};
use crate::db;
use crate::tmdb::testing::{FakeTmdb, Reply};

async fn token(pool: &SqlitePool, name: &str, admin: bool) -> String {
    let user = auth::create_user(pool, name, "a-long-password".into(), admin)
        .await
        .unwrap()
        .unwrap();
    auth::issue_token(pool, user.id, TokenKind::Session, "t")
        .await
        .unwrap()
        .1
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let request = request
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn admins_undo_a_decline_and_waiting_series_requests_change_season() {
    let pool = db::open_in_memory().await;
    let (admin, alice, bob) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
        token(&pool, "bob", false).await,
    );
    // Alice's: 10 a declined movie, 11 a series waiting for season 1, 12 the same series'
    // season 2 waiting, 13 an approved series, 14 a declined one she has asked for again
    // (15).
    sqlx::raw_sql(
            "INSERT INTO requests (id, user_id, kind, tmdb_id, season, title, state, reason, created_at, decided_at, decided_by) VALUES
                 (10, 2, 'movie', 603, NULL, 'The Matrix', 'declined', 'We have it on disc.', 0, 1, 1),
                 (11, 2, 'series', 1399, 1, 'Game of Thrones', 'requested', NULL, 0, NULL, NULL),
                 (12, 2, 'series', 1399, 2, 'Game of Thrones', 'requested', NULL, 0, NULL, NULL),
                 (13, 2, 'series', 1400, NULL, 'Seinfeld', 'approved', NULL, 0, 1, 1),
                 (14, 2, 'movie', 604, NULL, 'The Matrix Reloaded', 'declined', 'No.', 0, 1, 1),
                 (15, 2, 'movie', 604, NULL, 'The Matrix Reloaded', 'requested', NULL, 0, NULL, NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
    let app = routes(pool.clone(), None, Sources::default());

    // Reopening: admins only, declined ones only, and not when asked for again.
    let (status, _) = send(&app, "POST", "/api/v1/requests/10/reopen", &alice, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = send(&app, "POST", "/api/v1/requests/10/reopen", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (
            &body["state"],
            &body["reason"],
            &body["decided_at"],
            &body["decided_by"]
        ),
        (
            &json!("requested"),
            &Value::Null,
            &Value::Null,
            &Value::Null
        )
    );
    let (status, body) = send(&app, "POST", "/api/v1/requests/10/reopen", &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "only a declined request can be reopened");
    let (status, body) = send(&app, "POST", "/api/v1/requests/14/reopen", &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body["error"],
        "alice has asked for it again since; decide on that request instead"
    );
    let (status, _) = send(&app, "POST", "/api/v1/requests/99/reopen", &admin, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Changing the season: the requester or an admin, while it waits, for a series.
    let change = |id: i64, token: &str, season: Value| {
        let (app, token) = (app.clone(), token.to_owned());
        async move {
            send(
                &app,
                "PUT",
                &format!("/api/v1/requests/{id}"),
                &token,
                Some(json!({ "season": season })),
            )
            .await
        }
    };
    let (status, _) = change(11, &bob, json!(3)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "someone else's");
    let (status, body) = change(11, &alice, json!(3)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["season"], 3);
    let (status, body) = change(11, &alice, json!(2)).await;
    assert_eq!(status, StatusCode::CONFLICT, "season 2 is request 12");
    assert_eq!(body["error"], "you've asked for it already (request 12)");
    let (status, body) = change(11, &admin, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["season"], Value::Null, "the whole series");
    let (status, _) = change(11, &alice, json!(-1)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = change(10, &alice, json!(1)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a movie has no seasons");
    let (status, body) = change(13, &alice, json!(1)).await;
    assert_eq!(status, StatusCode::CONFLICT, "decided already");
    assert_eq!(body["error"], "it isn't waiting for a decision any more");
    let (status, _) = change(99, &admin, json!(1)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn removing_an_import_takes_back_only_its_own_waiting_requests() {
    let pool = db::open_in_memory().await;
    let (admin, alice, bob) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
        token(&pool, "bob", false).await,
    );
    // Alice's import 1 asked for songs 20 (waiting), 21 (approved) and 22 (waiting, but
    // import 2 lists it too), and found 23 asked for already.
    sqlx::raw_sql(
            "INSERT INTO requests (id, user_id, kind, title, artist, state, created_at, decided_at) VALUES
                 (20, 2, 'song', 'Airbag', 'Radiohead', 'requested', 0, NULL),
                 (21, 2, 'song', 'Lucky', 'Radiohead', 'approved', 0, 1),
                 (22, 2, 'song', 'Karma Police', 'Radiohead', 'requested', 0, NULL),
                 (23, 2, 'song', 'No Surprises', 'Radiohead', 'requested', 0, NULL);
             INSERT INTO playlist_imports (id, user_id, name, state, created_at, finished_at) VALUES
                 (1, 2, 'Road trip', 'done', 0, 1), (2, 2, 'Evening', 'done', 0, 1);
             INSERT INTO playlist_import_tracks (import_id, position, title, state, why, request_id) VALUES
                 (1, 1, 'Airbag', 'asked', NULL, 20), (1, 2, 'Lucky', 'asked', NULL, 21),
                 (1, 3, 'Karma Police', 'asked', NULL, 22), (1, 4, 'No Surprises', 'already', NULL, 23),
                 (1, 5, 'Nowhere', 'unmatched', 'not found', NULL),
                 (2, 1, 'Karma Police', 'already', NULL, 22);",
        )
        .execute(&pool)
        .await
        .unwrap();
    let app = routes(pool.clone(), None, Sources::default());

    let (status, _) = send(&app, "DELETE", "/api/v1/requests/imports/1", &bob, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "someone else's");
    let (status, body) = send(&app, "DELETE", "/api/v1/requests/imports/1", &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "taken_back": 1 }));
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM requests ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![21, 22, 23]);
    let tracks: i64 =
        sqlx::query_scalar("SELECT count(*) FROM playlist_import_tracks WHERE import_id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tracks, 0);
    let (status, _) = send(&app, "DELETE", "/api/v1/requests/imports/1", &alice, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "gone");

    // An admin removes anyone's.
    let (status, body) = send(&app, "DELETE", "/api/v1/requests/imports/2", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "taken_back": 0 }), "it only found 22 already");
}

#[tokio::test]
async fn users_ask_and_follow_their_own_and_admins_decide() {
    let pool = db::open_in_memory().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'), (2, 'shows', 'shows', '/s')")
            .execute(&pool)
            .await
            .unwrap();
    let (admin, alice, bob) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
        token(&pool, "bob", false).await,
    );
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603",
        vec![Reply::ok(
            json!({"id": 603, "title": "The Matrix", "release_date": "1999-03-30"}),
        )],
    );
    let sources = Sources {
        tmdb: Some(Arc::new(fake.start().await)),
        ..Sources::default()
    };
    let app = routes(pool.clone(), None, sources);

    let (status, asked) = send(
        &app,
        "POST",
        "/api/v1/requests",
        &alice,
        Some(json!({"kind": "movie", "tmdb_id": 603})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    assert_eq!(asked["state"], "requested");
    let id = asked["id"].as_i64().unwrap();
    let mine = format!("/api/v1/requests/{id}");

    assert_eq!(
        send(&app, "GET", "/api/v1/requests", &alice, None).await.1["total"],
        1
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/requests", &bob, None).await.1["total"],
        0
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/requests?all=true", &alice, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/requests?all=true", &admin, None)
            .await
            .1["total"],
        1
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/requests?state=nope", &alice, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send(&app, "GET", &mine, &bob, None).await.0,
        StatusCode::NOT_FOUND,
        "someone else's"
    );
    assert_eq!(
        send(&app, "DELETE", &mine, &bob, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "GET", &mine, &admin, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        send(&app, "POST", &format!("{mine}/approve"), &alice, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // Approved with no body: the defaults, here the only movies library.
    let (status, approved) = send(&app, "POST", &format!("{mine}/approve"), &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(
        (&approved["state"], &approved["progress"]["step"]),
        (&json!("approved"), &json!("searching"))
    );
    let (status, again) = send(
        &app,
        "POST",
        &format!("{mine}/decline"),
        &admin,
        Some(json!({"reason": "late"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(
        send(&app, "DELETE", &mine, &alice, None).await.0,
        StatusCode::CONFLICT,
        "decided already"
    );
}

#[tokio::test]
async fn admins_set_defaults_and_whose_requests_go_through() {
    let pool = db::open_in_memory().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'), (2, 'shows', 'shows', '/s')")
            .execute(&pool)
            .await
            .unwrap();
    let admin = token(&pool, "admin", true).await;
    let alice = token(&pool, "alice", false).await;
    let app = routes(pool.clone(), None, Sources::default());

    let (status, settings) = send(&app, "GET", "/api/v1/requests/settings", &admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["defaults"].as_array().unwrap().len(), 4);
    assert_eq!(settings["users"][1]["username"], "alice");
    assert_eq!(settings["users"][1]["requests_approved"], false);
    assert_eq!(
        send(&app, "GET", "/api/v1/requests/settings", &alice, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let put = |uri: &'static str, body: Value| {
        let (app, admin) = (app.clone(), admin.clone());
        async move { send(&app, "PUT", uri, &admin, Some(body)).await }
    };
    assert_eq!(
        put(
            "/api/v1/requests/settings/defaults/series",
            json!({"library_id": 1})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "a movies library"
    );
    assert_eq!(
        put("/api/v1/requests/settings/defaults/books", json!({}))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        put(
            "/api/v1/requests/settings/defaults/movies",
            json!({"music_profile_id": 1})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let (status, set) = put(
        "/api/v1/requests/settings/defaults/series",
        json!({"library_id": 2}),
    )
    .await;
    assert_eq!((status, &set["library_id"]), (StatusCode::OK, &json!(2)));
    let alice_id = settings["users"][1]["id"].as_i64().unwrap();
    let uri = format!("/api/v1/requests/settings/users/{alice_id}");
    assert_eq!(
        send(
            &app,
            "PUT",
            &uri,
            &admin,
            Some(json!({"requests_approved": true}))
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let after = send(&app, "GET", "/api/v1/requests/settings", &admin, None)
        .await
        .1;
    assert_eq!(after["users"][1]["requests_approved"], true);
    assert_eq!(after["defaults"][1]["library_id"], 2);

    // Without TMDB, movies and series can't be looked up.
    let (status, said) = send(
        &app,
        "POST",
        "/api/v1/requests",
        &alice,
        Some(json!({"kind": "movie", "tmdb_id": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        said["error"]
            .as_str()
            .unwrap()
            .contains("MELETE_TMDB_TOKEN")
    );
}

#[tokio::test]
async fn music_is_asked_for_by_its_musicbrainz_id_and_approved_with_a_music_profile() {
    use crate::musicbrainz::testing::{self, OK_COMPUTER};
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (admin, alice) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
    );
    let (mb, _) = testing::start(0).await;
    let app = routes(pool.clone(), Some(Arc::new(mb)), Sources::default());

    let (status, asked) = send(
        &app,
        "POST",
        "/api/v1/requests",
        &alice,
        Some(json!({"kind": "album", "mb_release_group_id": OK_COMPUTER})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
    assert_eq!(
        (&asked["title"], &asked["artist"], &asked["state"]),
        (
            &json!("OK Computer"),
            &json!("Radiohead"),
            &json!("requested")
        )
    );
    let (status, wrong) = send(
        &app,
        "POST",
        &format!("/api/v1/requests/{}/approve", asked["id"]),
        &admin,
        Some(json!({"music_profile_id": 999})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{wrong}");
    let (status, approved) = send(
        &app,
        "POST",
        &format!("/api/v1/requests/{}/approve", asked["id"]),
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["state"], "approved");

    // Without MusicBrainz, music can't be looked up.
    let without = routes(pool.clone(), None, Sources::default());
    let (status, refused) = send(
        &without,
        "POST",
        "/api/v1/requests",
        &alice,
        Some(json!({"kind": "artist", "mb_artist_id": testing::RADIOHEAD})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("MELETE_MUSICBRAINZ")
    );
}

#[tokio::test]
async fn each_user_follows_their_own_requests_live() {
    let pool = db::open_in_memory().await;
    let (alice, _bob) = (
        token(&pool, "alice", false).await,
        token(&pool, "bob", false).await,
    );
    sqlx::query(
        "INSERT INTO requests (user_id, kind, title, artist, state, created_at)
             SELECT id, 'song', 'Creep', username, 'requested', 0 FROM users",
    )
    .execute(&pool)
    .await
    .unwrap();
    let response = routes(pool.clone(), None, Sources::default())
        .oneshot(
            Request::get("/api/v1/requests/events")
                .header(header::AUTHORIZATION, format!("Bearer {alice}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    let first = std::str::from_utf8(&first).unwrap();
    let data = first
        .strip_prefix("event: requests\ndata: ")
        .and_then(|d| d.strip_suffix("\n\n"))
        .unwrap_or_else(|| panic!("not a requests event: {first}"));
    let sent: Vec<Value> = serde_json::from_str(data).unwrap();
    let whose: Vec<&str> = sent.iter().map(|r| r["artist"].as_str().unwrap()).collect();
    assert_eq!(whose, ["alice"], "only her own");

    // Declined: sent again with its new state.
    sqlx::query(
        "UPDATE requests SET state = 'declined', reason = 'no', decided_at = 1
             WHERE artist = 'alice'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let next = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
        .await
        .expect("the change was sent")
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    assert!(std::str::from_utf8(&next).unwrap().contains("\"declined\""));
}

#[tokio::test]
async fn anyone_imports_a_playlist_and_admins_approve_it_whole() {
    use crate::musicbrainz::testing;
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (admin, alice, bob) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
        token(&pool, "bob", false).await,
    );
    let (mb, _) = testing::start(0).await;
    let mb = Arc::new(mb);
    let app = routes(pool.clone(), Some(mb.clone()), Sources::default());
    let csv = "Track Name,Artist Name(s),ISRC\nParanoid Android,Radiohead,GBAYE9700245\n";

    let (status, made) = send(
        &app,
        "POST",
        "/api/v1/requests/imports",
        &alice,
        Some(json!({"name": "Road trip", "csv": csv})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{made}");
    assert_eq!(
        (&made["state"], &made["tracks"]),
        (&json!("queued"), &json!(1))
    );
    let id = made["id"].as_i64().unwrap();
    crate::playlist_import::run(&pool, &mb, &Sources::default(), id)
        .await
        .unwrap();
    let mine = format!("/api/v1/requests/imports/{id}");
    let (status, view) = send(&app, "GET", &mine, &alice, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&view["state"], &view["waiting"], &view["items"][0]["state"]),
        (&json!("done"), &json!(1), &json!("asked"))
    );
    assert_eq!(
        send(&app, "GET", &mine, &bob, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, "GET", "/api/v1/requests/imports", &alice, None)
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/api/v1/requests/imports?all=true",
            &alice,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    let approve = format!("{mine}/approve");
    assert_eq!(
        send(&app, "POST", &approve, &alice, None).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, approved) = send(&app, "POST", &approve, &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["approved"].as_array().unwrap().len(), 1);
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/v1/requests/imports/99/approve",
            &admin,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Not a playlist, no name, or no MusicBrainz to match with.
    for (body, status) in [
        (
            json!({"name": "x", "csv": "Artist\nRadiohead\n"}),
            StatusCode::BAD_REQUEST,
        ),
        (json!({"name": " ", "csv": csv}), StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            send(&app, "POST", "/api/v1/requests/imports", &alice, Some(body))
                .await
                .0,
            status
        );
    }
    let without = routes(pool.clone(), None, Sources::default());
    assert_eq!(
        send(
            &without,
            "POST",
            "/api/v1/requests/imports",
            &alice,
            Some(json!({"name": "x", "csv": csv}))
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
}

/// Two movie libraries, a shows library, a profile of each kind, and TMDB knowing The Matrix,
/// its sequel and Game of Thrones (seasons 0 to 3).
async fn choosing() -> (SqlitePool, Router) {
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path, category) VALUES
             (1, 'Films', 'movies', '/m', NULL), (2, 'Shows', 'shows', '/s', NULL),
             (3, 'Kids', 'movies', '/k', 'kids');
         INSERT INTO quality_profiles (id, source, name, upgrade_allowed, cutoff, min_format_score,
                 cutoff_format_score, min_upgrade_format_score, updated_at) VALUES
             (5, 'radarr', 'HD', 1, 'Bluray-1080p', 0, 0, 1, 0),
             (6, 'sonarr', 'HD', 1, 'Bluray-1080p', 0, 0, 1, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603",
        vec![Reply::ok(
            json!({"id": 603, "title": "The Matrix", "release_date": "1999-03-30"}),
        )],
    )
    .reply(
        "/movie/604",
        vec![Reply::ok(
            json!({"id": 604, "title": "The Matrix Reloaded", "release_date": "2003-05-15"}),
        )],
    )
    .reply(
        "/tv/1399?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 1399, "name": "Game of Thrones", "first_air_date": "2011-04-17",
            "original_language": "en", "genres": [{"id": 1, "name": "Drama"}],
            "seasons": [{"season_number": 0}, {"season_number": 1}, {"season_number": 2},
                        {"season_number": 3}],
            "external_ids": {"tvdb_id": 121361, "imdb_id": "tt0944947"},
        }))],
    );
    let sources = Sources {
        tmdb: Some(Arc::new(fake.start().await)),
        ..Sources::default()
    };
    let app = routes(pool.clone(), None, sources);
    (pool, app)
}

#[tokio::test]
async fn admins_add_with_their_choices_and_others_cannot_choose() {
    let (pool, app) = choosing().await;
    let (admin, alice) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
    );
    let ask = |token: &str, body: Value| {
        let (app, token) = (app.clone(), token.to_owned());
        async move { send(&app, "POST", "/api/v1/requests", &token, Some(body)).await }
    };

    // Anyone else choosing is refused, not ignored.
    for chosen in [
        json!({"kind": "movie", "tmdb_id": 603, "library_id": 3}),
        json!({"kind": "movie", "tmdb_id": 603, "search_now": false}),
        json!({"kind": "movie", "tmdb_id": 603, "monitored": true}),
    ] {
        let (status, body) = ask(&alice, chosen.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{chosen}: {body}");
    }
    let asked: i64 = sqlx::query_scalar("SELECT count(*) FROM requests")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(asked, 0, "nothing is kept from a refused request");

    // An admin's movie: the kids library, its profile, not monitored, not searched yet.
    let (status, movie) = ask(
        &admin,
        json!({"kind": "movie", "tmdb_id": 603, "library_id": 3, "profile_id": 5,
               "monitored": false, "search_now": false}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{movie}");
    assert_eq!(
        (&movie["state"], &movie["search_started"]),
        (&json!("approved"), &json!(false))
    );
    let placed: (i64, Option<i64>, bool) = sqlx::query_as(
        "SELECT library_id, profile_id, monitored FROM wanted_movies WHERE tmdb_id = 603",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(placed, (3, Some(5), false));

    // Without choices: the defaults, monitored, searched at once.
    let (status, sequel) = ask(&admin, json!({"kind": "movie", "tmdb_id": 604})).await;
    assert_eq!(status, StatusCode::CREATED, "{sequel}");
    assert_eq!(sequel["search_started"], true);
    let placed: (i64, Option<i64>, bool) = sqlx::query_as(
        "SELECT library_id, profile_id, monitored FROM wanted_movies WHERE tmdb_id = 604",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        placed,
        (1, None, true),
        "the only library without a category"
    );

    // A series with the seasons chosen.
    let (status, show) = ask(
        &admin,
        json!({"kind": "series", "tmdb_id": 1399, "seasons": [1, 3], "search_now": false}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{show}");
    let seasons: Vec<(i64, bool)> =
        sqlx::query_as("SELECT season, monitored FROM wanted_seasons ORDER BY season")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(seasons, [(0, false), (1, true), (2, false), (3, true)]);

    // Choices that don't fit are refused, and nothing is kept.
    for misfit in [
        json!({"kind": "movie", "tmdb_id": 604, "seasons": [1]}),
        json!({"kind": "movie", "tmdb_id": 604, "music_profile_id": 1}),
        json!({"kind": "series", "tmdb_id": 1399, "seasons": [-1]}),
        json!({"kind": "movie", "tmdb_id": 603, "library_id": 2}),
        json!({"kind": "series", "tmdb_id": 1399, "profile_id": 5}),
    ] {
        let (status, body) = ask(&admin, misfit.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{misfit}: {body}");
    }
}

#[tokio::test]
async fn approving_takes_the_same_choices() {
    let (pool, app) = choosing().await;
    let (admin, alice) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
    );
    let (_, asked) = send(
        &app,
        "POST",
        "/api/v1/requests",
        &alice,
        Some(json!({"kind": "series", "tmdb_id": 1399, "season": 2})),
    )
    .await;
    let approve = format!("/api/v1/requests/{}/approve", asked["id"]);
    let (status, approved) = send(
        &app,
        "POST",
        &approve,
        &admin,
        Some(json!({"seasons": [2, 3], "monitored": true, "search_now": false, "profile_id": 6})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["search_started"], false);
    let seasons: Vec<(i64, bool)> =
        sqlx::query_as("SELECT season, monitored FROM wanted_seasons ORDER BY season")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(seasons, [(0, false), (1, false), (2, true), (3, true)]);
    let profile: Option<i64> = sqlx::query_scalar("SELECT profile_id FROM wanted_series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(profile, Some(6));
}

#[tokio::test]
async fn a_rating_limited_user_finds_titles_within_their_rating_to_ask_for() {
    let pool = db::open_in_memory().await;
    let (alice, kid) = (
        token(&pool, "alice", false).await,
        token(&pool, "kid", false).await,
    );
    let kid_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'kid'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let limit = crate::access::Restriction {
        libraries: None,
        max_age: Some(12),
    };
    crate::access::set_restriction(&pool, kid_id, Some(&limit))
        .await
        .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/search/movie?include_adult=false&query=Toy",
        vec![Reply::ok(json!({"results": [
            {"id": 862, "title": "Toy Story", "release_date": "1995-10-30"},
            {"id": 863, "title": "Toy Story 2", "release_date": "1999-11-24"},
            {"id": 864, "title": "Toy Soldiers", "release_date": "1991-04-26"},
            {"id": 865, "title": "Toy Box", "release_date": "2001-01-01"},
        ]}))],
    )
    .reply(
        "/movie/863",
        vec![Reply::ok(
            json!({"id": 863, "title": "Toy Story 2", "release_date": "1999-11-24"}),
        )],
    );
    for (id, cert) in [(862, "G"), (863, "PG"), (864, "R")] {
        fake.reply(
            &format!("/movie/{id}/release_dates"),
            vec![Reply::ok(json!({"id": id, "results": [
                {"iso_3166_1": "US", "release_dates": [{"certification": cert, "type": 3}]}
            ]}))],
        );
    }
    let sources = Sources {
        tmdb: Some(Arc::new(fake.start().await)),
        ..Sources::default()
    };
    let app = routes(pool.clone(), None, sources);
    let search = "/api/v1/requests/search?kind=movie&q=Toy";
    let ids = |found: &Value| -> Vec<i64> {
        found
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["tmdb_id"].as_i64().unwrap())
            .collect()
    };
    let rating_requests = || {
        fake.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.ends_with("/release_dates"))
            .count()
    };
    // Without a highest rating: everything, and no rating asked for.
    let (status, found) = send(&app, "GET", search, &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(ids(&found), [862, 863, 864, 865]);
    assert_eq!(rating_requests(), 0);
    // Rated up to 12: only what's rated that or under; R and the unrated one are left out.
    let (status, found) = send(&app, "GET", search, &kid, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(ids(&found), [862, 863]);
    assert_eq!(rating_requests(), 4);
    // A second search asks for no rating again.
    let (_, found) = send(&app, "GET", search, &kid, None).await;
    assert_eq!(ids(&found), [862, 863]);
    assert_eq!(rating_requests(), 4);
    // And what's found can be asked for.
    let (status, asked) = send(
        &app,
        "POST",
        "/api/v1/requests",
        &kid,
        Some(json!({"kind": "movie", "tmdb_id": 863})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{asked}");
}

#[tokio::test]
async fn music_in_a_library_a_user_cant_see_isnt_in_the_library_for_them() {
    use crate::musicbrainz::testing::{OK_COMPUTER, PARANOID_ANDROID, RADIOHEAD};
    let pool = db::open_in_memory().await;
    // OK Computer, by Radiohead, with Paranoid Android on it, only in the second music library.
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (5, 'music', 'music', '/a'),
             (6, 'more music', 'music', '/b');
         INSERT INTO music_artists (id, key, name, mb_artist_id)
             VALUES (1, 'radiohead', 'Radiohead', ?1);
         INSERT INTO music_albums (id, library_id, key, title, artist_name, artist_id, folder,
             seen_generation, mb_release_group_id)
             VALUES (1, 6, 'k1', 'OK Computer', 'Radiohead', 1, 'ok', 1, ?2);
         INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
             first_seen_generation, seen_generation, album_id, tagged, title, mb_recording_id)
             VALUES (1, 6, 'ok/2.flac', 1, 1, 1, 1, 1, 1, 'Paranoid Android', ?3);",
    )
    .bind(RADIOHEAD)
    .bind(OK_COMPUTER)
    .bind(PARANOID_ANDROID)
    .execute(&pool)
    .await
    .unwrap();
    let (everything, first, both) = (
        token(&pool, "alice", false).await,
        token(&pool, "kid", false).await,
        token(&pool, "teen", false).await,
    );
    for (name, libraries) in [("kid", vec![5]), ("teen", vec![5, 6])] {
        let id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap();
        let limit = crate::access::Restriction {
            libraries: Some(libraries),
            max_age: None,
        };
        crate::access::set_restriction(&pool, id, Some(&limit))
            .await
            .unwrap();
    }
    let (mb, _) = crate::musicbrainz::testing::start(0).await;
    let app = routes(pool.clone(), Some(Arc::new(mb)), Sources::default());
    let searches = [
        (
            "album",
            "/api/v1/requests/search?kind=album&q=OK%20Computer",
            OK_COMPUTER,
        ),
        (
            "song",
            "/api/v1/requests/search?kind=song&q=Paranoid%20Android",
            PARANOID_ANDROID,
        ),
        (
            "artist",
            "/api/v1/requests/search?kind=artist&q=Radiohead",
            RADIOHEAD,
        ),
    ];
    for (who, token, seen) in [
        ("alice", &everything, true),
        ("kid", &first, false),
        ("teen", &both, true),
    ] {
        for (kind, uri, id) in searches {
            let (status, found) = send(&app, "GET", uri, token, None).await;
            assert_eq!(status, StatusCode::OK, "{found}");
            let it = found
                .as_array()
                .unwrap()
                .iter()
                .find(|c| {
                    [
                        &c["mb_release_group_id"],
                        &c["mb_recording_id"],
                        &c["mb_artist_id"],
                    ]
                    .iter()
                    .any(|v| v.as_str() == Some(id))
                        && c["kind"] == kind
                })
                .unwrap_or_else(|| panic!("{kind} not found: {found}"));
            assert_eq!(it["in_library"], json!(seen), "{who}'s {kind}");
        }
    }
}

#[tokio::test]
async fn each_request_says_its_service_and_a_list_narrows_to_one() {
    let pool = db::open_in_memory().await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603",
        vec![Reply::ok(
            json!({"id": 603, "title": "The Matrix", "release_date": "1999-03-30"}),
        )],
    )
    .reply(
        "/tv/1399?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 1399, "name": "Game of Thrones", "first_air_date": "2011-04-17",
            "original_language": "en", "genres": [{"id": 16, "name": "Animation"}],
        }))],
    )
    .reply(
        "/tv/209867?append_to_response=external_ids",
        vec![Reply::ok(json!({
            "id": 209867, "name": "Frieren", "first_air_date": "2023-09-29",
            "original_language": "ja", "genres": [{"id": 16, "name": "Animation"}],
        }))],
    );
    let sources = Sources {
        tmdb: Some(Arc::new(fake.start().await)),
        ..Sources::default()
    };
    let app = routes(pool.clone(), None, sources);
    let (admin, alice) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
    );
    for (kind, id) in [("movie", 603), ("series", 1399), ("series", 209867)] {
        let body = json!({"kind": kind, "tmdb_id": id});
        let (status, _) = send(&app, "POST", "/api/v1/requests", &alice, Some(body)).await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (_, all) = send(&app, "GET", "/api/v1/requests?all=true", &admin, None).await;
    let mut said: Vec<(&str, &str)> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["title"].as_str().unwrap(), r["service"].as_str().unwrap()))
        .collect();
    said.sort();
    // Animated but not Japanese isn't anime, as Sonarr's anime type means.
    assert_eq!(
        said,
        [
            ("Frieren", "anime"),
            ("Game of Thrones", "series"),
            ("The Matrix", "movies")
        ]
    );
    let uri = "/api/v1/requests?all=true&state=requested&service=anime";
    let (status, anime) = send(&app, "GET", uri, &admin, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(anime["total"], 1);
    assert_eq!(anime["items"][0]["title"], "Frieren");
    let (status, _) = send(&app, "GET", "/api/v1/requests?service=films", &admin, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// Music's Add new: songs found on MusicBrainz by title and artist, and one added as an admin
/// adds, with where it goes and its profile, and not monitored (a song is searched for once).
#[tokio::test]
async fn admins_find_songs_and_add_one_as_add_new_does() {
    use crate::musicbrainz::testing::{self, PARANOID_ANDROID};
    let pool = db::open_in_memory().await;
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (4, 'music', 'music', '/music')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let profile = crate::music_profiles::lossless(&pool).await.unwrap();
    let admin = token(&pool, "admin", true).await;
    let (mb, log) = testing::start(0).await;
    let app = routes(pool.clone(), Some(Arc::new(mb)), Sources::default());

    let (status, found) = send(
        &app,
        "GET",
        "/api/v1/requests/search?kind=song&q=Paranoid%20Android&artist=Radiohead",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(
        (
            &found[0]["kind"],
            &found[0]["mb_recording_id"],
            &found[0]["artist"]
        ),
        (
            &json!("song"),
            &json!(PARANOID_ANDROID),
            &json!("Radiohead")
        )
    );
    let searched = log.lock().unwrap().join("\n");
    assert!(searched.contains("search recording"), "{searched}");

    let add = |monitored: Option<bool>| {
        let mut body = json!({"kind": "song", "mb_recording_id": PARANOID_ANDROID,
            "library_id": 4, "music_profile_id": profile, "search_now": false});
        if let Some(on) = monitored {
            body["monitored"] = json!(on);
        }
        body
    };
    let (status, refused) = send(
        &app,
        "POST",
        "/api/v1/requests",
        &admin,
        Some(add(Some(true))),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a song isn't monitored: {refused}"
    );
    let (status, added) = send(&app, "POST", "/api/v1/requests", &admin, Some(add(None))).await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert_eq!(
        (&added["state"], added["search_started"].as_bool()),
        (&json!("approved"), Some(false))
    );
    let wanted: (i64, String, String, i64, i64) = sqlx::query_as(
        "SELECT id, title, mb_recording_id, library_id, music_profile_id FROM wanted_songs",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        wanted,
        (
            added["wanted_id"].as_i64().unwrap(),
            "Paranoid Android".into(),
            PARANOID_ANDROID.into(),
            4,
            profile
        )
    );
}

/// A movie or series found gets its poster as discovery gives posters: one Melete knows always;
/// one it doesn't only with posters of titles not in the library on, and then noted as listed so
/// the artwork route serves it on the same terms.
#[tokio::test]
async fn results_get_posters_on_discoverys_terms() {
    let pool = db::open_in_memory().await;
    let alice = token(&pool, "alice", false).await;
    sqlx::query(
        "INSERT INTO tmdb_movies (tmdb_id, title, poster_path, fetched_at)
             VALUES (603, 'The Matrix', '/matrix.jpg', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let fake = FakeTmdb::default();
    fake.reply(
        "/search/movie?include_adult=false&query=Matrix",
        vec![Reply::ok(json!({"page": 1, "total_pages": 1, "results": [
            {"id": 603, "title": "The Matrix", "release_date": "1999-03-30", "poster_path": "/matrix.jpg"},
            {"id": 9999, "title": "Matrix", "release_date": "", "poster_path": "/sweden.jpg"},
            {"id": 9998, "title": "Matrix II", "release_date": ""},
        ]}))],
    );
    let sources = Sources {
        tmdb: Some(Arc::new(fake.start().await)),
        ..Sources::default()
    };
    let app = routes(pool.clone(), None, sources);
    let posters = |found: &Value| {
        found
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["poster"].clone())
            .collect::<Vec<_>>()
    };
    let uri = "/api/v1/requests/search?kind=movie&q=Matrix";
    let (status, found) = send(&app, "GET", uri, &alice, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(
        posters(&found),
        [
            json!("/api/v1/artwork/w342/matrix.jpg"),
            Value::Null,
            Value::Null
        ]
    );
    let listed: i64 = sqlx::query_scalar("SELECT count(*) FROM discover_posters")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(listed, 0, "off: nothing noted");

    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY_POSTERS', 'on', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, found) = send(&app, "GET", uri, &alice, None).await;
    assert_eq!(
        posters(&found),
        [
            json!("/api/v1/artwork/w342/matrix.jpg"),
            json!("/api/v1/artwork/w342/sweden.jpg"),
            Value::Null
        ]
    );
    let noted = crate::discover_posters::servable(&pool, "/sweden.jpg", crate::db::unix_now())
        .await
        .unwrap();
    assert!(noted, "served on discovery's terms");
}

/// When MusicBrainz finds nothing, Add new looks on Soulseek: only the words go to slskd, and
/// what its users share comes back as songs. Admins only.
#[tokio::test]
async fn admins_look_on_soulseek_with_only_the_words() {
    let pool = db::open_in_memory().await;
    let (admin, alice) = (
        token(&pool, "admin", true).await,
        token(&pool, "alice", false).await,
    );
    let answers = json!([{"username": "peer", "hasFreeUploadSlot": true, "files": [
        {"filename": "Music\\Unsigned Band\\Demo\\01 - Garage Song.flac", "size": 1000},
        {"filename": "Music\\Unsigned Band\\Demo\\folder.jpg", "size": 10},
    ]}]);
    let (slskd, seen) = crate::slskd::testing::start(answers, 1).await;
    let sources = Sources {
        slskd: Some(Arc::new(slskd.quick())),
        ..Sources::default()
    };
    let app = routes(pool.clone(), None, sources);
    let uri = "/api/v1/requests/soulseek?q=unsigned%20band%20garage%20song";
    let (status, found) = send(&app, "GET", uri, &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(
        found,
        json!([{"title": "Garage Song", "artist": "Unsigned Band", "users": 1, "formats": ["FLAC"]}])
    );
    assert_eq!(seen.lock().unwrap().texts, ["unsigned band garage song"]);
    let (status, _) = send(&app, "GET", uri, &alice, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let blank = "/api/v1/requests/soulseek?q=%20";
    let (status, _) = send(&app, "GET", blank, &admin, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let without = routes(pool, None, Sources::default());
    let (status, body) = send(&without, "GET", uri, &admin, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}
