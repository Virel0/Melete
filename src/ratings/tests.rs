use axum::http::StatusCode;
use serde_json::json;

use super::*;
use crate::tmdb::testing::{FakeTmdb, Reply};

/// Movies matched to 603 and 604 (the second gone from the library) and 605 (rated already, a
/// day ago), and a series matched to 1399.
async fn library() -> SqlitePool {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'shows', 'shows', '/s');
         INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (603, 'A', 0), (604, 'B', 0),
             (605, 'C', 0);
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at) VALUES (1399, 'S', 0);
         INSERT INTO movies (library_id, rel_path, parsed_title, seen_generation, match_status,
             match_tmdb_id, missing_since) VALUES
             (1, 'a', 'A', 1, 'matched', 603, NULL), (1, 'b', 'B', 1, 'matched', 604, 9),
             (1, 'c', 'C', 1, 'matched', 605, NULL), (1, 'd', 'D', 1, NULL, NULL, NULL);
         INSERT INTO series (library_id, rel_path, parsed_title, seen_generation, match_status,
             match_tmdb_id) VALUES (2, 's', 'S', 1, 'matched', 1399);",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at)
         VALUES (0, 605, '{\"US\": \"G\"}', 0, ?)",
    )
    .bind(unix_now() - 24 * 60 * 60)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

async fn stored(pool: &SqlitePool, tv: bool, id: i64) -> Option<(String, Option<i64>)> {
    sqlx::query_as("SELECT certifications, age FROM tmdb_ratings WHERE tv = ? AND tmdb_id = ?")
        .bind(tv)
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn ratings_are_fetched_for_the_librarys_matched_titles_and_read_in_the_chosen_country() {
    let pool = library().await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603/release_dates",
        vec![Reply::ok(json!({ "id": 603, "results": [
            { "iso_3166_1": "US", "release_dates": [
                { "certification": "", "type": 4 },
                { "certification": "R", "type": 3 } ] },
            { "iso_3166_1": "DE", "release_dates": [ { "certification": "16", "type": 3 } ] },
            { "iso_3166_1": "XX", "release_dates": [ { "certification": "Mystery", "type": 3 } ] },
            { "iso_3166_1": "GB", "release_dates": [ { "certification": "", "type": 3 } ] }
        ]}))],
    )
    .reply(
        "/tv/1399/content_ratings",
        vec![Reply::ok(json!({ "id": 1399, "results": [
            { "iso_3166_1": "US", "rating": "TV-MA" }, { "iso_3166_1": "AU", "rating": "MA15+" }
        ]}))],
    );
    let tmdb = fake.start().await;
    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(
        report,
        Report {
            fetched: 2,
            errors: 0
        }
    );
    let (certs, age) = stored(&pool, false, 603).await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&certs).unwrap(),
        json!({ "US": "R", "DE": "16", "XX": "Mystery" })
    );
    assert_eq!(age, Some(17));
    assert_eq!(stored(&pool, true, 1399).await.unwrap().1, Some(17));
    // Gone from the library, or rated lately: not asked for.
    assert_eq!(stored(&pool, false, 604).await, None);
    let log = fake.log.lock().unwrap().clone();
    assert!(!log.iter().any(|r| r.contains("/605/")), "{log:?}");

    // A second run asks for nothing.
    fake.log.lock().unwrap().clear();
    refresh(&pool, &tmdb).await.unwrap();
    assert!(fake.log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_title_tmdb_lacks_is_stored_unrated_and_a_failure_is_tried_again() {
    let pool = library().await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603/release_dates",
        vec![Reply::status(StatusCode::NOT_FOUND)],
    )
    .reply(
        "/tv/1399/content_ratings",
        vec![Reply::status(StatusCode::INTERNAL_SERVER_ERROR)],
    );
    let tmdb = fake.start().await;
    let report = refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(report.fetched, 1);
    assert_eq!(report.errors, 1);
    assert_eq!(stored(&pool, false, 603).await, Some(("{}".into(), None)));
    assert_eq!(stored(&pool, true, 1399).await, None);
}

#[tokio::test]
async fn ages_are_worked_out_again_from_what_was_stored() {
    let pool = library().await;
    sqlx::query("UPDATE tmdb_ratings SET age = 18")
        .execute(&pool)
        .await
        .unwrap();
    reread(&pool).await.unwrap();
    assert_eq!(stored(&pool, false, 605).await.unwrap().1, Some(0));
}

fn certs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn a_title_is_judged_by_each_listed_countrys_own_rating() {
    // A rating each country really gives, and the age it comes to; the United States' differs,
    // so the chosen country's is the one that counts.
    let samples: &[(&str, &str, u8)] = &[
        ("US", "PG-13", 13),
        ("AU", "MA15+", 15),
        ("BR", "14", 14),
        ("CA", "14A", 14),
        ("DK", "11", 11),
        ("FI", "K-16", 16),
        ("FR", "12", 12),
        ("DE", "16", 16),
        ("IN", "UA", 12),
        ("IE", "15A", 15),
        ("IT", "VM14", 14),
        ("JP", "PG12", 12),
        ("NL", "AL", 0),
        ("NZ", "R16", 16),
        ("NO", "9", 9),
        ("RU", "6+", 6),
        ("KR", "ALL", 0),
        ("ES", "APTA", 0),
        ("SE", "Btl", 0),
        ("GB", "12A", 12),
    ];
    let listed: Vec<&str> = COUNTRIES.iter().map(|c| c.value).collect();
    let sampled: Vec<&str> = samples.iter().map(|(c, _, _)| *c).collect();
    assert_eq!(listed, sampled, "every listed country, and only those");
    assert_eq!(COUNTRIES[0].value, "US", "the United States first");
    for (country, cert, want) in samples {
        let mut title = certs(&[(country, cert)]);
        if *country == "US" {
            title.insert("GB".into(), "18".into());
        } else {
            title.insert("US".into(), "NC-17".into());
        }
        assert_eq!(age_of(&title, country), Some(*want), "{country} {cert}");
    }
}

#[test]
fn without_the_countrys_rating_the_united_states_counts_and_without_both_none() {
    let us_and_de = certs(&[("US", "R"), ("DE", "16")]);
    assert_eq!(age_of(&us_and_de, "GB"), Some(17));
    assert_eq!(age_of(&us_and_de, "DE"), Some(16));
    // One the chosen country gives that can't be read counts as none.
    assert_eq!(age_of(&certs(&[("GB", "E"), ("US", "PG")]), "GB"), Some(10));
    for (title, country) in [
        (certs(&[("DE", "16")]), "US"),
        (certs(&[("DE", "16")]), "GB"),
        (certs(&[("US", "NR"), ("FR", "Mystery")]), "FR"),
        (certs(&[]), "GB"),
    ] {
        assert_eq!(age_of(&title, country), None, "{title:?} in {country}");
    }
}

#[tokio::test]
async fn changing_the_country_judges_every_title_again_without_fetching() {
    let pool = library().await;
    sqlx::query(
        "INSERT INTO tmdb_ratings (tv, tmdb_id, certifications, age, fetched_at) VALUES
             (0, 603, '{\"US\": \"R\", \"GB\": \"15\"}', 17, ?1),
             (1, 1399, '{\"DE\": \"12\"}', NULL, ?1)",
    )
    .bind(unix_now())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(country(&pool).await.unwrap(), "US", "the default");
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_RATINGS_COUNTRY', 'GB', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    reread(&pool).await.unwrap();
    assert_eq!(stored(&pool, false, 603).await.unwrap().1, Some(15));
    assert_eq!(stored(&pool, true, 1399).await.unwrap().1, None);
    assert_eq!(
        stored(&pool, false, 605).await.unwrap().1,
        Some(0),
        "the US one"
    );

    // What's fetched from now on is judged by it too.
    let fake = FakeTmdb::default();
    fake.reply(
        "/tv/1399/content_ratings",
        vec![Reply::ok(json!({ "id": 1399, "results": [
            { "iso_3166_1": "US", "rating": "TV-MA" }, { "iso_3166_1": "GB", "rating": "15" }
        ]}))],
    );
    sqlx::query("UPDATE tmdb_ratings SET fetched_at = 0 WHERE tv = 1")
        .execute(&pool)
        .await
        .unwrap();
    let tmdb = fake.start().await;
    refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(stored(&pool, true, 1399).await.unwrap().1, Some(15));
}

#[tokio::test]
async fn ratings_of_titles_shown_outside_the_library_are_fetched_once_and_kept_a_month() {
    let pool = library().await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/900/release_dates",
        vec![Reply::ok(json!({ "id": 900, "results": [
            { "iso_3166_1": "US", "release_dates": [ { "certification": "PG", "type": 3 } ] }
        ]}))],
    )
    .reply(
        "/movie/901/release_dates",
        vec![Reply::status(StatusCode::INTERNAL_SERVER_ERROR)],
    );
    let tmdb = fake.start().await;
    // 605 is rated already; 902 is unknown to TMDB, so unrated; 901's request fails.
    let titles = [
        (false, 900),
        (false, 900),
        (false, 605),
        (false, 901),
        (true, 902),
    ];
    assert_eq!(fetch_unknown(&pool, &tmdb, &titles).await.unwrap(), 2);
    assert_eq!(stored(&pool, false, 900).await.unwrap().1, Some(10));
    assert_eq!(stored(&pool, true, 902).await, Some(("{}".into(), None)));
    assert_eq!(
        stored(&pool, false, 901).await,
        None,
        "tried again next time"
    );
    let log = fake.log.lock().unwrap().clone();
    assert_eq!(
        log.iter().filter(|r| r.contains("/900/")).count(),
        1,
        "{log:?}"
    );
    assert!(!log.iter().any(|r| r.contains("/605/")), "{log:?}");

    // A month on, the library's are fetched again and the rest forgotten.
    sqlx::query("UPDATE tmdb_ratings SET fetched_at = 0")
        .execute(&pool)
        .await
        .unwrap();
    refresh(&pool, &tmdb).await.unwrap();
    assert_eq!(stored(&pool, false, 900).await, None);
    assert_eq!(stored(&pool, true, 902).await, None);
    assert!(stored(&pool, false, 605).await.is_some(), "the library's");
}
