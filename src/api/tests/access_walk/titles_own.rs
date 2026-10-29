//! The walk's checks of what titles carry beyond TMDB's data: their own artwork from their
//! folders, their extras, who's in them and the collections they belong to. A hidden title's is named "zyxveil" like everything hidden, so the walk would see
//! it leak; here it's also asked for directly, cached or not.

use super::*;

/// The open movie's and a hidden one's own posters.
pub(super) const ROWS: &str = r#"
UPDATE movies SET local_poster = 'Open Movie (2001)/poster.jpg', local_poster_key = '/local-open1.jpg'
    WHERE id = 1;
UPDATE movies SET local_poster = 'Zyxveil Rated Movie (2001)/poster.jpg',
    local_poster_key = '/local-zyxveil12.jpg' WHERE id = 12;
INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, series_id, kind,
    title, first_seen_generation, seen_generation) VALUES
    (1099511627776, 1, 'Open Movie (2001)/Trailers/Open Trailer.mkv', 5, 1, 1, NULL, 'trailer',
     'Open Trailer', 1, 1),
    (1099511627777, 1, 'Zyxveil Rated Movie (2001)/Trailers/Zyxveil Trailer.mkv', 5, 1, 12, NULL,
     'trailer', 'Zyxveil Trailer', 1, 1),
    (1099511627778, 4, 'Zyxveil Grown Series/Extras/Zyxveil Reel.mkv', 5, 1, NULL, 21, 'other',
     'Zyxveil Reel', 1, 1);
INSERT INTO tmdb_people (id, name, profile_path) VALUES (1, 'Open Actor', '/open-p1.jpg'),
    (11, 'Zyxveil Actor', '/zyxveil-p11.jpg'), (12, 'Zyxveil Creator', NULL);
INSERT INTO tmdb_credits (movie_tmdb_id, series_tmdb_id, person_id, role, character, job,
    position) VALUES
    (101, NULL, 1, 'cast', 'The Lead', NULL, 0),
    (112, NULL, 1, 'cast', 'Zyxveil Part', NULL, 0),
    (112, NULL, 11, 'cast', 'Zyxveil Villain', NULL, 1),
    (NULL, 221, 12, 'crew', NULL, 'Creator', 0);
INSERT INTO tmdb_collections (id, name, poster_path) VALUES (1, 'Open Collection', '/open-c1.jpg'),
    (12, 'Zyxveil Collection', '/zyxveil-c12.jpg');
UPDATE tmdb_movies SET collection_id = 1 WHERE tmdb_id IN (101, 111);
UPDATE tmdb_movies SET collection_id = 12 WHERE tmdb_id = 112;
"#;

/// The hidden titles' extras, by id.
const HIDDEN_EXTRAS: &[i64] = &[1099511627777, 1099511627778];

#[tokio::test]
async fn a_hidden_titles_extras_cant_be_listed_or_played() {
    let w = world().await;
    let (status, open) = ask(&w, "GET", "/api/v1/movies/1/extras").await;
    assert_eq!(status, StatusCode::OK);
    assert!(open.contains("Open Trailer"), "{open}");
    for uri in ["/api/v1/movies/12/extras", "/api/v1/series/21/extras"] {
        assert_eq!(ask(&w, "GET", uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    for id in HIDDEN_EXTRAS {
        for (method, uri) in [
            ("GET", format!("/api/v1/files/{id}")),
            ("GET", format!("/api/v1/files/{id}/subtitles")),
            ("POST", format!("/api/v1/files/{id}/play")),
        ] {
            assert_eq!(
                ask(&w, method, &uri).await.0,
                StatusCode::NOT_FOUND,
                "{method} {uri}"
            );
        }
        let uri = format!("/api/v1/files/{id}/transcode");
        let body = json!({ "max_height": 480 });
        assert_eq!(
            send_json(&w, "POST", &uri, body).await,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    // A link made before the title was hidden stops with the restriction.
    lift(&w).await;
    let (status, play) = ask(&w, "POST", "/api/v1/files/1099511627777/play").await;
    assert_eq!(status, StatusCode::OK, "{play}");
    let link: Value = serde_json::from_str(&play).unwrap();
    let link = link["stream_url"].as_str().unwrap().to_owned();
    restrict(
        &w,
        Some(Restriction {
            libraries: Some(vec![1, 2, 5]),
            max_age: Some(12),
        }),
    )
    .await;
    assert_eq!(ask(&w, "GET", &link).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_titles_own_artwork_is_served_only_to_who_may_see_it() {
    let w = world().await;
    let cached = w._dir.path().join("artwork").join("w342");
    std::fs::create_dir_all(&cached).unwrap();
    for file in ["local-open1.jpg", "local-zyxveil12.jpg"] {
        std::fs::write(cached.join(file), b"jpeg").unwrap();
    }
    let (_, movies) = ask(&w, "GET", "/api/v1/movies").await;
    assert!(
        movies.contains("/api/v1/artwork/w342/local-open1.jpg"),
        "{movies}"
    );
    let open = ask(&w, "GET", "/api/v1/artwork/w342/local-open1.jpg")
        .await
        .0;
    assert_eq!(open, StatusCode::OK);
    let hidden = ask(&w, "GET", "/api/v1/artwork/w342/local-zyxveil12.jpg")
        .await
        .0;
    assert_eq!(
        hidden,
        StatusCode::NOT_FOUND,
        "cached, but its title is hidden"
    );

    lift(&w).await;
    let hidden = ask(&w, "GET", "/api/v1/artwork/w342/local-zyxveil12.jpg")
        .await
        .0;
    assert_eq!(hidden, StatusCode::OK, "seen once nothing is restricted");
}

#[tokio::test]
async fn hidden_titles_cast_and_collections_stay_hidden() {
    let w = world().await;
    let (_, credits) = ask(&w, "GET", "/api/v1/movies/1/credits").await;
    assert!(
        credits.contains("Open Actor") && credits.contains("Open Collection"),
        "{credits}"
    );
    let (status, actor) = ask(&w, "GET", "/api/v1/people/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        actor.contains("Open Movie"),
        "only the title they may see: {actor}"
    );
    let (_, collection) = ask(&w, "GET", "/api/v1/collections/1").await;
    assert!(collection.contains("Open Movie"), "{collection}");
    for uri in [
        "/api/v1/movies/12/credits",
        "/api/v1/series/21/credits",
        "/api/v1/people/11",
        "/api/v1/people/12",
        "/api/v1/collections/12",
    ] {
        assert_eq!(ask(&w, "GET", uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
    let cached = w._dir.path().join("artwork");
    for (size, file) in [
        ("w185", "open-p1.jpg"),
        ("w185", "zyxveil-p11.jpg"),
        ("w342", "open-c1.jpg"),
        ("w342", "zyxveil-c12.jpg"),
    ] {
        std::fs::create_dir_all(cached.join(size)).unwrap();
        std::fs::write(cached.join(size).join(file), b"jpeg").unwrap();
    }
    for (uri, expected) in [
        ("/api/v1/artwork/w185/open-p1.jpg", StatusCode::OK),
        ("/api/v1/artwork/w342/open-c1.jpg", StatusCode::OK),
        (
            "/api/v1/artwork/w185/zyxveil-p11.jpg",
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/v1/artwork/w342/zyxveil-c12.jpg",
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(ask(&w, "GET", uri).await.0, expected, "{uri}");
    }

    lift(&w).await;
    for uri in [
        "/api/v1/people/11",
        "/api/v1/collections/12",
        "/api/v1/movies/12/credits",
    ] {
        let (status, body) = ask(&w, "GET", uri).await;
        assert!(
            body.contains("Zyxveil"),
            "GET {uri} ({status}) shows them once lifted: {body}"
        );
    }
}
