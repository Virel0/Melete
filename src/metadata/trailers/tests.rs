use serde_json::json;

use super::*;
use crate::tmdb::testing::{FakeTmdb, Reply};

fn trailer(key: &str, language: Option<&str>, kind: &str, official: bool, size: i64) -> Trailer {
    Trailer {
        key: key.into(),
        name: key.into(),
        language: language.map(str::to_owned),
        kind: kind.into(),
        official,
        size,
        published_at: None,
    }
}

#[test]
fn the_trailer_picked_is_in_the_first_language_it_has_one_in() {
    let all = [
        trailer("aaaaaaaaaaa", Some("en"), "Trailer", true, 1080),
        trailer("bbbbbbbbbbb", Some("fr"), "Teaser", true, 1080),
        trailer("ccccccccccc", Some("fr"), "Trailer", false, 720),
        trailer("ddddddddddd", Some("fr"), "Trailer", true, 480),
        trailer("eeeeeeeeeee", Some("de"), "Trailer", true, 2160),
        trailer("fffffffffff", None, "Trailer", true, 2160),
    ];
    let key = |languages: &[&str]| chosen(&all, languages).map(|t| t.key.as_str());
    // French: a trailer before a teaser, an official one before a bigger one that isn't.
    assert_eq!(key(&["fr", "en"]), Some("ddddddddddd"));
    // Nothing in Spanish: English. Nothing in German or without a language is taken for it.
    assert_eq!(key(&["es", "en"]), Some("aaaaaaaaaaa"));
    assert_eq!(key(&["en"]), Some("aaaaaaaaaaa"));
    assert_eq!(chosen(&all[4..], &["en"]), None);
    // A title's own language first: a German film's German trailer.
    assert_eq!(key(&["de", "fr", "en"]), Some("eeeeeeeeeee"));
    // The bigger one, all else equal.
    let sizes = [
        trailer("ggggggggggg", Some("en"), "Trailer", true, 720),
        trailer("hhhhhhhhhhh", Some("en"), "Trailer", true, 2160),
    ];
    assert_eq!(chosen(&sizes, &["en"]).unwrap().key, "hhhhhhhhhhh");
}

#[test]
fn trailers_are_looked_for_in_the_titles_language_then_the_metadata_language_then_english() {
    assert_eq!(languages(Some("ja"), "fr-FR"), ["ja", "fr", "en"]);
    assert_eq!(languages(Some("en"), "en-US"), ["en"]);
    assert_eq!(languages(None, "de-DE"), ["de", "en"]);
    assert_eq!(languages(Some("fr"), "fr-FR"), ["fr", "en"]);
}

#[test]
fn only_youtube_trailers_and_teasers_with_a_proper_id_are_kept() {
    let video = |key: &str, site: &str, kind: &str| Video {
        key: key.into(),
        name: "x".into(),
        site: site.into(),
        kind: kind.into(),
        official: true,
        language: Some("en".into()),
        size: 1080,
        published_at: None,
    };
    let kept: Vec<String> = kept(vec![
        video("dQw4w9WgXcQ", "YouTube", "Trailer"),
        video("dQw4w9WgXc_", "YouTube", "Teaser"),
        video("12345678901", "Vimeo", "Trailer"),
        video("dQw4w9WgXcQ", "YouTube", "Featurette"),
        video("bad\"key/abc", "YouTube", "Trailer"),
        video("short", "YouTube", "Trailer"),
    ])
    .into_iter()
    .map(|t| t.key)
    .collect();
    assert_eq!(kept, ["dQw4w9WgXcQ", "dQw4w9WgXc_"]);
}

async fn with_a_movie(on: bool) -> SqlitePool {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO tmdb_movies (tmdb_id, title, fetched_at) VALUES (603, 'The Matrix', 100);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES (1, 1, 'The Matrix (1999)', 'The Matrix', 1,
             'matched', 603);",
    )
    .execute(&pool)
    .await
    .unwrap();
    if on {
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TRAILER_DOWNLOADS', 'on', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
    }
    pool
}

fn videos() -> Reply {
    Reply::ok(json!({ "id": 603, "results": [
        { "key": "vKQi3bBA1y8", "name": "Official Trailer", "site": "YouTube", "type": "Trailer",
          "official": true, "iso_639_1": "en", "size": 1080,
          "published_at": "2021-01-01T00:00:00.000Z" },
        { "key": "m8e-FF8MsqU", "name": "Behind the scenes", "site": "YouTube",
          "type": "Featurette", "official": true, "iso_639_1": "en", "size": 1080 }
    ] }))
}

#[tokio::test]
async fn off_asks_tmdb_for_nothing() {
    let pool = with_a_movie(false).await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603/videos?include_video_language=en",
        vec![videos()],
    );
    let tmdb = fake.start().await;
    assert_eq!(refresh_trailers(&pool, &tmdb).await.unwrap(), 0);
    assert!(fake.log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn on_each_title_is_asked_for_once_and_again_with_its_details() {
    let pool = with_a_movie(true).await;
    let fake = FakeTmdb::default();
    fake.reply(
        "/movie/603/videos?include_video_language=en",
        vec![videos()],
    );
    let tmdb = fake.start().await;
    assert_eq!(refresh_trailers(&pool, &tmdb).await.unwrap(), 1);
    assert_eq!(
        *fake.log.lock().unwrap(),
        ["/movie/603/videos?include_video_language=en"]
    );
    let stored: String = sqlx::query_scalar("SELECT videos FROM tmdb_movies WHERE tmdb_id = 603")
        .fetch_one(&pool)
        .await
        .unwrap();
    let kept: Vec<Trailer> = serde_json::from_str(&stored).unwrap();
    assert_eq!(kept.len(), 1, "the featurette left out");
    assert_eq!(kept[0].key, "vKQi3bBA1y8");

    assert_eq!(refresh_trailers(&pool, &tmdb).await.unwrap(), 0, "once");
    sqlx::query("UPDATE tmdb_movies SET fetched_at = videos_fetched_at + 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        refresh_trailers(&pool, &tmdb).await.unwrap(),
        1,
        "with the details"
    );
}

#[tokio::test]
async fn its_own_language_the_metadata_languages_and_englishs_are_asked_for() {
    let pool = with_a_movie(true).await;
    sqlx::query("UPDATE tmdb_movies SET original_language = 'ja'")
        .execute(&pool)
        .await
        .unwrap();
    let fake = FakeTmdb::default();
    fake.language("fr-FR");
    fake.reply(
        "/movie/603/videos?include_video_language=ja,fr,en#fr-FR",
        vec![videos()],
    );
    let tmdb = fake.start().await.with_language("fr-FR".into());
    assert_eq!(refresh_trailers(&pool, &tmdb).await.unwrap(), 1);
}
