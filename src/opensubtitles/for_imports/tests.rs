use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::*;
use crate::opensubtitles::testing::{self, Fake};

struct Fixture {
    pool: SqlitePool,
    api: String,
    fake: Arc<Fake>,
    subtitles: PathBuf,
    _dir: tempfile::TempDir,
}

/// Film 1, imported by a download `imported_ago` seconds ago, with a Spanish text track and a
/// French picture track inside it, English beside it, and OpenSubtitles set up with the
/// languages `imports` for new imports.
async fn fixture(imports: Option<&str>, imported_ago: i64) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("movies");
    std::fs::create_dir_all(root.join("Film (2020)")).unwrap();
    let video = root.join("Film (2020)/Film.mkv");
    std::fs::write(&video, vec![7u8; 100_000]).unwrap();
    std::fs::write(root.join("Film (2020)/Film.en.srt"), "1\n").unwrap();
    let size = std::fs::metadata(&video).unwrap().len() as i64;
    let pool = crate::db::open_in_memory().await;
    let info = serde_json::json!({
        "container": "matroska,webm", "duration_seconds": 60.0, "bit_rate": null,
        "video": [], "audio": [],
        "subtitles": [
            { "index": 2, "codec": "subrip", "language": "spa", "title": null,
              "default": false, "forced": false, "text": true },
            { "index": 3, "codec": "hdmv_pgs_subtitle", "language": "fre", "title": null,
              "default": false, "forced": false, "text": false }
        ]
    });
    sqlx::query(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?);
         INSERT INTO movies (id, library_id, rel_path, parsed_title, tmdb_id, seen_generation)
             VALUES (1, 1, 'Film (2020)', 'Film', 603, 1);",
    )
    .bind(root.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, 'Film (2020)/Film.mkv', ?, 1, 1, 1, 1)",
    )
    .bind(size)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
         VALUES (1, ?, 1, 0, ?)",
    )
    .bind(size)
    .bind(info.to_string())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO wanted_movies (id, tmdb_id, title, monitored, source, added_at)
             VALUES (1, 603, 'Film', 1, 'manual', 0);
         INSERT INTO downloads (infohash, title, wanted_movie_id, decision, state, placed,
             added_at, updated_at) VALUES (lower(hex(randomblob(20))), 'Film.2020', 1, '{}', 'imported', ?, 0, ?)",
    )
    .bind(serde_json::to_string(&[video.to_str().unwrap()]).unwrap())
    .bind(unix_now() - imported_ago)
    .execute(&pool)
    .await
    .unwrap();
    let mut settings = vec![("MELETE_OPENSUBTITLES_KEY", testing::KEY.to_owned())];
    if let Some(imports) = imports {
        settings.push(("MELETE_OPENSUBTITLES_IMPORTS", imports.to_owned()));
    }
    for (name, value) in settings {
        sqlx::query("INSERT INTO settings (name, value, updated_at) VALUES (?, ?, 0)")
            .bind(name)
            .bind(value)
            .execute(&pool)
            .await
            .unwrap();
    }
    let (api, fake) = testing::start().await;
    Fixture {
        pool,
        api,
        fake,
        subtitles: dir.path().join("subtitles"),
        _dir: dir,
    }
}

impl Fixture {
    async fn run(&self) -> Report {
        fetch_for_imports_at(&self.pool, &self.subtitles, &self.api)
            .await
            .unwrap()
    }

    /// The paths OpenSubtitles was asked for, and the language of each search.
    fn asked(&self) -> Vec<String> {
        self.fake
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| match s.path.as_str() {
                "/subtitles" => {
                    let language = s
                        .query
                        .split('&')
                        .find_map(|p| p.strip_prefix("languages="))
                        .unwrap_or_default();
                    format!("search {language}")
                }
                other => other.to_owned(),
            })
            .collect()
    }
}

#[tokio::test]
async fn a_new_import_gets_one_made_for_it_in_each_language_it_lacks() {
    let f = fixture(Some("en, fr, es"), 3600).await;
    assert_eq!(
        f.run().await,
        Report {
            fetched: 1,
            none_made: 0
        }
    );
    // English is beside it and Spanish inside it; French is only pictures, so it's looked for.
    assert_eq!(f.asked(), ["search fr", "/download"]);
    let fetched = fetched_for(&f.pool, 1).await.unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].language, "fr");
    assert_eq!(fetched[0].release.as_deref(), Some("This.Very.File"));
    let kept = fetched_path(&f.subtitles, (1, 100_000, 1), "fr");
    assert!(
        std::fs::read_to_string(kept)
            .unwrap()
            .contains("Fetched 102")
    );
    // Once per file and language.
    assert_eq!(f.run().await, Report::default());
    assert_eq!(f.asked().len(), 2);
}

#[tokio::test]
async fn off_without_languages_and_only_for_new_imports() {
    let off = fixture(None, 3600).await;
    assert_eq!(off.run().await, Report::default());
    assert!(off.asked().is_empty(), "no languages, nothing sent");

    let old = fixture(Some("fr"), IMPORTS_KEPT + 60).await;
    assert_eq!(old.run().await, Report::default());
    assert!(old.asked().is_empty(), "imported too long ago");

    let unprobed = fixture(Some("fr"), 3600).await;
    sqlx::query("DELETE FROM media_probes")
        .execute(&unprobed.pool)
        .await
        .unwrap();
    assert_eq!(unprobed.run().await, Report::default());
    assert!(unprobed.asked().is_empty(), "what's inside isn't known yet");
}

#[tokio::test]
async fn nothing_made_for_the_file_fetches_nothing_and_isnt_asked_again() {
    let f = fixture(Some("fr"), 3600).await;
    f.fake.nothing.store(true, Ordering::Relaxed);
    assert_eq!(
        f.run().await,
        Report {
            fetched: 0,
            none_made: 1
        }
    );
    assert_eq!(f.asked(), ["search fr"]);
    // A search finding only subtitles made for other releases fetches none of them either.
    f.fake.nothing.store(false, Ordering::Relaxed);
    sqlx::query("DELETE FROM opensubtitles_import_tries")
        .execute(&f.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE opensubtitles_searches SET results = ?")
        .bind(
            serde_json::json!([{ "id": 101, "release": "Popular.WEB", "language": "fr",
                "hearing_impaired": false, "downloads": 5000, "matches_file": false }])
            .to_string(),
        )
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(f.run().await.none_made, 1);
    assert_eq!(f.run().await, Report::default());
    assert_eq!(f.asked(), ["search fr"], "remembered, and tried once");
}

#[tokio::test]
async fn the_days_downloads_used_up_stop_it_until_they_come_back() {
    let f = fixture(Some("fr, de"), 3600).await;
    f.fake.quota_reached.store(true, Ordering::Relaxed);
    assert_eq!(f.run().await, Report::default());
    assert_eq!(
        f.asked(),
        ["search fr", "/download"],
        "stopped at the first"
    );
    assert!(used_up_until(&f.pool).await.unwrap().is_some());
    assert_eq!(f.run().await, Report::default());
    assert_eq!(f.asked().len(), 2, "nothing asked until they come back");
    let tried: i64 = sqlx::query_scalar("SELECT count(*) FROM opensubtitles_import_tries")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(tried, 0, "left to try again");
}
