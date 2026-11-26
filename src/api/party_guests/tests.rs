use axum::body::Body;
use axum::extract::Request;
use axum::http::HeaderValue;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

struct Fixture {
    app: Router,
    pool: SqlitePool,
    host: String,
    party: i64,
    _library: tempfile::TempDir,
}

const CONTENT: &[u8] = b"a film's bytes";

/// A household device in a party playing a real file; another file to move on to.
async fn fixture(cap: Option<i64>) -> Fixture {
    let library = tempfile::tempdir().unwrap();
    std::fs::create_dir(library.path().join("Film")).unwrap();
    std::fs::write(library.path().join("Film/a.mkv"), CONTENT).unwrap();
    std::fs::write(library.path().join("Film/b.mkv"), CONTENT).unwrap();
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'films', 'movies', '{}');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Film', 'Film', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation)
             VALUES (1, 1, 'Film/a.mkv', 14, 0, 1, 1, 1), (2, 1, 'Film/b.mkv', 14, 0, 1, 1, 1);",
        library.path().display()
    )))
    .execute(&pool)
    .await
    .unwrap();
    let alice = crate::auth::testing::user(&pool, "alice", false).await;
    let (token_id, host) =
        crate::auth::issue_token(&pool, alice.id, crate::auth::TokenKind::Session, "tv")
            .await
            .unwrap();
    let film = crate::watch_party::State {
        file_id: 1,
        position_ms: 0,
        playing: true,
    };
    let party = crate::watch_party::start(&pool, (alice.id, token_id), &film, cap)
        .await
        .unwrap()
        .unwrap();
    let app = super::super::watch_parties::routes(pool.clone())
        .merge(routes(pool.clone(), PathBuf::from("ffprobe")))
        .merge(super::super::playback::routes(
            pool.clone(),
            "ffprobe".into(),
        ));
    Fixture {
        app,
        pool,
        host,
        party,
        _library: library,
    }
}

async fn send(f: &Fixture, request: Request) -> (StatusCode, HeaderMap, Value) {
    let response = f.app.clone().oneshot(request).await.unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn json_request(method: &str, uri: &str, auth: Option<(&str, &str)>, body: Value) -> Request {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some((name, value)) = auth {
        builder = builder.header(name, value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

/// An invite made by the host, and a guest in by it: their pass and cookie.
async fn guest_in(f: &Fixture, name: &str) -> (String, String) {
    let bearer = format!("Bearer {}", f.host);
    let uri = format!("/api/v1/watch-parties/{}/invites", f.party);
    let (status, _, made) = send(
        f,
        json_request("POST", &uri, Some(("authorization", &bearer)), Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let invite = made["invite"].as_str().unwrap().to_owned();
    assert_eq!(made["path"], format!("/#/guest/{invite}"));
    let (status, headers, joined) = send(
        f,
        json_request(
            "POST",
            "/api/v1/party/join",
            None,
            json!({"invite": invite, "name": name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{joined}");
    let set = headers[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(
        set.contains("; Path=/api/v1/party; HttpOnly; SameSite=Strict"),
        "{set}"
    );
    let cookie = set.split(';').next().unwrap().to_owned();
    (joined["pass"].as_str().unwrap().to_owned(), cookie)
}

#[tokio::test]
async fn a_guest_follows_plays_and_streams_their_party_and_reaches_nothing_else() {
    let f = fixture(None).await;
    let (pass, cookie) = guest_in(&f, "Sam").await;
    let with_cookie = Some(("cookie", cookie.as_str()));

    let (status, _, seen) = send(
        &f,
        json_request("GET", "/api/v1/party", with_cookie, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["party"]["title"], "Film");
    assert!(!seen.to_string().contains("alice"));
    // Exactly the documented fields, so nothing of the household slips in unnoticed.
    let spec: Value = serde_json::from_str(include_str!("../openapi.json")).unwrap();
    let fields = |schema: &str| -> std::collections::BTreeSet<String> {
        spec["components"]["schemas"][schema]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    let keys = |v: &Value| -> std::collections::BTreeSet<String> {
        v.as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(keys(&seen), fields("GuestPartyAnswer"));
    assert_eq!(keys(&seen["party"]), fields("GuestParty"));

    // A change by cookie needs the web header; with it, the guest pauses the party.
    let pause = json!({"version": 1, "position_ms": 4000, "playing": false});
    let (status, _, _) = send(
        &f,
        json_request("PUT", "/api/v1/party", with_cookie, pause.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let mut request = json_request("PUT", "/api/v1/party", with_cookie, pause);
    request
        .headers_mut()
        .insert(WEB_HEADER, HeaderValue::from_static("1"));
    let (status, _, paused) = send(&f, request).await;
    assert_eq!(
        (status, paused["party"]["playing"].clone()),
        (StatusCode::OK, json!(false))
    );

    // The stream link opens the file, as a guest's link.
    let bearer = format!("Bearer {pass}");
    let with_pass = Some(("authorization", bearer.as_str()));
    let (_, _, link) = send(
        &f,
        json_request("POST", "/api/v1/party/play", with_pass, Value::Null),
    )
    .await;
    let url = link["stream_url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("/api/v1/stream/mls_g"), "{url}");
    let stream = f
        .app
        .clone()
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(
        &stream.into_body().collect().await.unwrap().to_bytes()[..],
        CONTENT
    );

    // Hidden from whoever let the guest in, the file stops for the guest too.
    let alice: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'alice'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let hidden = crate::access::Restriction {
        libraries: Some(vec![]),
        max_age: None,
    };
    crate::access::set_restriction(&f.pool, alice, Some(&hidden))
        .await
        .unwrap();
    let refused = f
        .app
        .clone()
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    crate::access::set_restriction(&f.pool, alice, None)
        .await
        .unwrap();

    // A pass is no sign-in: the household's own routes refuse it.
    let (status, _, _) = send(
        &f,
        json_request("GET", "/api/v1/watch-parties", with_pass, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = send(
        &f,
        json_request("GET", "/api/v1/files/1", with_pass, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The host moves the party on: the old link stops; a new one opens the new file.
    let host = format!("Bearer {}", f.host);
    let uri = format!("/api/v1/watch-parties/{}", f.party);
    let next = json!({"version": 2, "file_id": 2, "position_ms": 0, "playing": true});
    let (status, _, _) = send(
        &f,
        json_request("PUT", &uri, Some(("authorization", &host)), next),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stale = f
        .app
        .clone()
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::NOT_FOUND);
    let (_, _, link) = send(
        &f,
        json_request("POST", "/api/v1/party/play", with_pass, Value::Null),
    )
    .await;
    assert_eq!(link["file_id"], 2);

    // Sent away: the pass ends.
    let guest: i64 = sqlx::query_scalar("SELECT id FROM watch_party_guests")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let away = format!("/api/v1/watch-parties/{}/guests/{guest}", f.party);
    let (status, _, _) = send(
        &f,
        json_request("DELETE", &away, Some(("authorization", &host)), Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(
        &f,
        json_request("GET", "/api/v1/party", with_pass, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_capped_party_refuses_guests_a_stream_over_its_cap_and_invites_are_spent_once() {
    let f = fixture(Some(1_000)).await;
    let (pass, _) = guest_in(&f, "Sam").await;
    let bearer = format!("Bearer {pass}");
    let with_pass = Some(("authorization", bearer.as_str()));
    let (_, _, link) = send(
        &f,
        json_request("POST", "/api/v1/party/play", with_pass, Value::Null),
    )
    .await;
    assert_eq!(link["max_bitrate"], 1_000);
    let url = link["stream_url"].as_str().unwrap().to_owned();
    let stream = f
        .app
        .clone()
        .oneshot(Request::get(&url).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        stream.status(),
        StatusCode::FORBIDDEN,
        "the guest is asked to convert"
    );
    // The household's own stream isn't held to the guests' cap.
    let host = format!("Bearer {}", f.host);
    let (_, _, own) = send(
        &f,
        json_request(
            "POST",
            "/api/v1/files/1/play",
            Some(("authorization", &host)),
            Value::Null,
        ),
    )
    .await;
    let own = own["stream_url"].as_str().unwrap().to_owned();
    let stream = f
        .app
        .clone()
        .oneshot(Request::get(&own).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);

    // A wrong or reused invite lets no one in.
    for invite in ["mpi_nope", "nonsense"] {
        let (status, _, _) = send(
            &f,
            json_request(
                "POST",
                "/api/v1/party/join",
                None,
                json!({"invite": invite, "name": "X"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
