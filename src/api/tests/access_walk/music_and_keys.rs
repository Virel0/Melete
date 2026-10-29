//! The walk's checks of music and of API keys: hidden music can't be opened, what can't be
//! narrowed to some music libraries is closed, and a key keeps its scope and its user's limits.

use super::*;

#[tokio::test]
async fn hidden_music_cant_be_opened_and_what_cant_be_narrowed_is_closed() {
    let w = world().await;
    for (method, uri) in [
        ("GET", "/api/v1/music/albums/41"),
        ("GET", "/api/v1/music/albums/41/cover"),
        ("GET", "/api/v1/music/albums/41/motion"),
        ("GET", "/api/v1/music/artists/2"),
        ("GET", "/api/v1/music/tracks/41/lyrics"),
        ("GET", "/api/v1/music/tracks/41/stems"),
        ("POST", "/api/v1/music/tracks/41/play"),
    ] {
        assert_eq!(
            ask(&w, method, uri).await.0,
            StatusCode::NOT_FOUND,
            "{method} {uri}"
        );
    }
    let (_, open) = ask(&w, "GET", "/api/v1/music/albums/1").await;
    assert!(open.contains("Open Album"), "{open}");
    // Aoide's sync and what listeners share carry tracks Melete doesn't read, so someone who
    // sees only some music libraries can't use them; someone who sees them all can.
    let shared = [
        "/aoide/sync/pull",
        "/api/v1/playlists",
        "/api/v1/music/household",
        "/api/v1/music/sessions",
        "/api/v1/music/transitions",
        "/api/v1/music/likes",
    ];
    for uri in shared {
        assert_eq!(ask(&w, "GET", uri).await.0, StatusCode::FORBIDDEN, "{uri}");
    }
    // So are edits to playlists from the web.
    for (method, uri, body) in [
        ("POST", "/api/v1/playlists", json!({ "name": "Mine" })),
        ("PUT", "/api/v1/playlists/P", json!({ "name": "Mine" })),
        (
            "POST",
            "/api/v1/playlists/P/items",
            json!({ "track_ids": [1] }),
        ),
        ("PUT", "/api/v1/playlists/P/items/I", json!({ "index": 0 })),
    ] {
        let status = send_json(&w, method, uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    for uri in ["/api/v1/playlists/P", "/api/v1/playlists/P/items/I"] {
        assert_eq!(
            ask(&w, "DELETE", uri).await.0,
            StatusCode::FORBIDDEN,
            "DELETE {uri}"
        );
    }
    // Likes are written to that log too, an open track's as much as a hidden one's.
    for track in [1, 41] {
        for method in ["PUT", "DELETE"] {
            let uri = format!("/api/v1/music/tracks/{track}/like");
            assert_eq!(
                ask(&w, method, &uri).await.0,
                StatusCode::FORBIDDEN,
                "{method} {uri}"
            );
        }
    }
    restrict(
        &w,
        Some(Restriction {
            libraries: Some(vec![1, 2, 5, 6]),
            max_age: None,
        }),
    )
    .await;
    for uri in shared {
        assert_ne!(ask(&w, "GET", uri).await.0, StatusCode::FORBIDDEN, "{uri}");
    }
}

/// A key limited to reading, of a user limited to some titles, keeps both limits at once.
#[tokio::test]
async fn a_scoped_key_keeps_its_scope_and_its_users_access() {
    let mut w = world().await;
    let kid: i64 = sqlx::query_scalar("SELECT id FROM users WHERE username = 'kid'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let (_, key) = crate::auth::issue_scoped(
        &w.pool,
        kid,
        crate::auth::TokenKind::ApiKey,
        "tv",
        crate::auth::Scope::Read,
    )
    .await
    .unwrap();
    w.kid = key;
    let (status, movies) = ask(&w, "GET", "/api/v1/movies").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        movies.contains("Open Movie") && !movies.contains("Zyxveil"),
        "{movies}"
    );
    assert_eq!(
        ask(&w, "GET", "/api/v1/movies/12").await.0,
        StatusCode::NOT_FOUND
    );
    // Reading only: adding an open favourite is past the key's scope.
    assert_eq!(
        ask(&w, "PUT", "/api/v1/me/favourites/movies/1").await.0,
        StatusCode::FORBIDDEN
    );
}

/// Request search names no hidden album, but says whether one is in the library: it says so
/// only of what the kid may see.
#[tokio::test]
async fn request_search_doesnt_say_hidden_music_is_in_the_library() {
    use crate::musicbrainz::testing::{self, OK_COMPUTER, PARANOID_ANDROID, RADIOHEAD};
    let (mb, _) = testing::start(0).await;
    let w =
        world_with(super::super::openapi::full_server_with(None, Some(Arc::new(mb))).await).await;
    // The hidden album and its track are MusicBrainz's OK Computer and Paranoid Android, by
    // the artist found only in the hidden library.
    sqlx::query(
        "UPDATE music_albums SET mb_release_group_id = ?1 WHERE id = 41;
         UPDATE music_tracks SET mb_recording_id = ?2 WHERE id = 41;
         UPDATE music_artists SET mb_artist_id = ?3 WHERE id = 2;",
    )
    .bind(OK_COMPUTER)
    .bind(PARANOID_ANDROID)
    .bind(RADIOHEAD)
    .execute(&w.pool)
    .await
    .unwrap();
    let flags = |body: &str| -> Vec<bool> {
        let found: Value = serde_json::from_str(body).unwrap();
        found
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| {
                [OK_COMPUTER, PARANOID_ANDROID, RADIOHEAD].iter().any(|id| {
                    [
                        &c["mb_release_group_id"],
                        &c["mb_recording_id"],
                        &c["mb_artist_id"],
                    ]
                    .iter()
                    .any(|v| v.as_str() == Some(id))
                })
            })
            .map(|c| c["in_library"].as_bool().unwrap())
            .collect()
    };
    let searches = [
        "/api/v1/requests/search?kind=album&q=OK%20Computer",
        "/api/v1/requests/search?kind=song&q=Paranoid%20Android",
        "/api/v1/requests/search?kind=artist&q=Radiohead",
    ];
    for uri in searches {
        let (status, body) = ask(&w, "GET", uri).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let flags = flags(&body);
        assert!(
            !flags.is_empty() && flags.iter().all(|f| !f),
            "{uri}: {body}"
        );
    }
    lift(&w).await;
    for uri in searches {
        let (_, body) = ask(&w, "GET", uri).await;
        assert!(flags(&body).contains(&true), "{uri}: {body}");
    }
}
