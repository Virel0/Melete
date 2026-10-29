//! The walk with TMDB answering: search, discovery and request search list titles outside the
//! library too, whose ratings are fetched for a user with a highest rating. Those rated above
//! the kid's limit, or not rated, are named "Zyxveil" like everything hidden.

use super::*;
use crate::tmdb::testing::{FakeTmdb, Reply};

/// Titles outside the library, by their stable ids: one rated within the kid's limit, one
/// above it, one TMDB gives no rating.
const OUTSIDE_IDS: &[&str] = &["tmdb:movie:501", "tmdb:movie:511", "tmdb:movie:512"];

/// Query strings for the routes that list TMDB's results.
const OUTSIDE_QUERIES: &[(&str, &str)] = &[
    ("/api/v1/discover/search", "?q=zyxveil"),
    ("/api/v1/requests/search", "?kind=movie&q=zyxveil"),
];

/// A TMDB that lists the library's titles and the outside ones everywhere, rating the open
/// movie and the outside ones (but the unrated).
async fn tmdb() -> (FakeTmdb, Arc<tmdb::Client>) {
    let listed = |id: i64, title: &str| {
        json!({ "id": id, "title": title, "release_date": "2001-01-01",
                "poster_path": format!("/{}-{id}.jpg", title.split(' ').next().unwrap().to_lowercase()) })
    };
    let page = json!({ "results": [
        listed(101, "Open Movie"),
        listed(111, "Zyxveil Grown Movie"),
        listed(112, "Zyxveil Rated Movie"),
        listed(501, "Open Outside Movie"),
        listed(511, "Zyxveil Outside Grown"),
        listed(512, "Zyxveil Outside Unrated"),
    ]});
    let fake = FakeTmdb::default();
    fake.reply("/trending/movie/week", vec![Reply::ok(page.clone())])
        .reply(
            "/search/movie?include_adult=false&query=zyxveil",
            vec![Reply::ok(page.clone())],
        );
    for id in [101, 111, 112, 114, 501, 511, 512] {
        fake.reply(
            &format!("/movie/{id}/recommendations"),
            vec![Reply::ok(page.clone())],
        );
    }
    for (id, cert) in [(101, "PG"), (112, "R"), (501, "PG"), (511, "R")] {
        fake.reply(
            &format!("/movie/{id}/release_dates"),
            vec![Reply::ok(json!({ "id": id, "results": [
                { "iso_3166_1": "US", "release_dates": [{ "certification": cert, "type": 3 }] }
            ]}))],
        );
    }
    for (id, title) in [
        (501, "Open Outside Movie"),
        (511, "Zyxveil Outside Grown"),
        (512, "Zyxveil Outside Unrated"),
    ] {
        fake.reply(&format!("/movie/{id}"), vec![Reply::ok(listed(id, title))]);
    }
    let client = Arc::new(fake.start().await);
    (fake, client)
}

/// The world with TMDB, discovery and its posters on.
async fn outside_world() -> World {
    let (_fake, client) = tmdb().await;
    let w = world_with(super::super::openapi::full_server_with(Some(client), None).await).await;
    sqlx::raw_sql(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_DISCOVERY', 'on', 0),
             ('MELETE_DISCOVERY_POSTERS', 'on', 0);",
    )
    .execute(&w.pool)
    .await
    .unwrap();
    w
}

/// Every address to ask for a documented path, with TMDB's results in the answers.
fn outside_addresses(path: &str) -> Vec<String> {
    if let Some((_, query)) = OUTSIDE_QUERIES.iter().find(|(p, _)| *p == path) {
        return vec![format!("{path}{query}")];
    }
    let mut all = addresses(path);
    if path.starts_with("/api/v1/items/") || path.starts_with("/api/v1/discover/similar/") {
        all.extend(OUTSIDE_IDS.iter().map(|id| path.replace("{id}", id)));
    }
    all
}

#[tokio::test]
async fn tmdbs_results_show_a_rating_limited_user_only_what_is_rated_within() {
    let w = outside_world().await;
    let spec = spec();
    let mut asked = 0;
    for (path, operations) in spec["paths"].as_object().unwrap() {
        let Some(op) = operations.get("get") else {
            continue;
        };
        if NOT_WALKED.contains(&path.as_str()) || op["tags"][0] == "Admin" {
            continue;
        }
        for uri in outside_addresses(path) {
            let (status, body) = ask(&w, "GET", &uri).await;
            assert!(
                !body.to_lowercase().contains("zyxveil"),
                "GET {uri} ({status}) shows a hidden title: {body}"
            );
            asked += 1;
        }
    }
    assert!(asked > 50, "{asked}");
    // The one rated within is there to see and ask for.
    for uri in [
        "/api/v1/discover/trending",
        "/api/v1/discover/search?q=zyxveil",
        "/api/v1/discover/similar/tmdb:movie:101",
        "/api/v1/requests/search?kind=movie&q=zyxveil",
        "/api/v1/items/tmdb:movie:501",
    ] {
        let (status, body) = ask(&w, "GET", uri).await;
        assert!(
            body.contains("Open Outside Movie"),
            "GET {uri} ({status}): {body}"
        );
    }
}

/// The walk above means something only if the same routes show the outside titles to someone
/// without a highest rating.
#[tokio::test]
async fn without_a_highest_rating_tmdbs_results_are_all_there() {
    let w = outside_world().await;
    restrict(
        &w,
        Some(Restriction {
            libraries: Some(vec![1, 2, 5]),
            max_age: None,
        }),
    )
    .await;
    for uri in [
        "/api/v1/discover/trending",
        "/api/v1/discover/search?q=zyxveil",
        "/api/v1/discover/similar/tmdb:movie:101",
        "/api/v1/requests/search?kind=movie&q=zyxveil",
        "/api/v1/items/tmdb:movie:511",
        "/api/v1/items/tmdb:movie:512",
    ] {
        let (status, body) = ask(&w, "GET", uri).await;
        assert!(
            body.contains("Zyxveil Outside"),
            "GET {uri} ({status}): {body}"
        );
    }
}

/// A rating-limited user is offered and served the poster of an outside title rated within
/// their limit, and not one listed for a title above it or unrated, even once listed.
#[tokio::test]
async fn a_rating_limited_user_gets_posters_only_of_outside_titles_rated_within() {
    let w = outside_world().await;
    // Already in the artwork cache, so nothing is fetched.
    let cache = w
        ._dir
        .path()
        .join("artwork")
        .join(crate::discover_posters::SIZE);
    std::fs::create_dir_all(&cache).unwrap();
    for file in ["open-501.jpg", "zyxveil-511.jpg", "zyxveil-512.jpg"] {
        std::fs::write(cache.join(file), b"poster").unwrap();
    }
    let (status, body) = ask(&w, "GET", "/api/v1/discover/trending").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("/api/v1/artwork/w342/open-501.jpg"), "{body}");
    let (status, _) = ask(&w, "GET", "/api/v1/artwork/w342/open-501.jpg").await;
    assert_eq!(status, StatusCode::OK);
    // As a discovery answer to someone without a rating limit would list them.
    let now = crate::db::unix_now();
    for (file, id) in [("/zyxveil-511.jpg", 511), ("/zyxveil-512.jpg", 512)] {
        crate::discover_posters::listed_for(&w.pool, file, false, id, now)
            .await
            .unwrap();
        let (status, _) = ask(&w, "GET", &format!("/api/v1/artwork/w342{file}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{file}");
    }
}
