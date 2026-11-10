use std::path::Path;

use serde_json::{Value, json};

use super::*;
use crate::db;

/// The stand-in downloader: answers every queued job the way the real one does, with a short
/// test pattern made by ffmpeg instead of a trailer from YouTube.
fn fake_downloader(jobs: &Path, gone: &str, fail: &str) {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trailers/fake-downloader.sh");
    let status = std::process::Command::new("sh")
        .arg(script)
        .arg(jobs)
        .env("GONE", gone)
        .env("FAIL", fail)
        .status()
        .unwrap();
    assert!(status.success());
}

fn folders(scratch: &Path) -> Folders {
    let folders = Folders {
        jobs: scratch.join("jobs"),
        videos: scratch.join("config/trailers"),
    };
    for sub in ["queue", "done"] {
        std::fs::create_dir_all(folders.jobs.join(sub)).unwrap();
    }
    folders
}

fn videos(key: &str) -> String {
    json!([{ "key": key, "name": "Official Trailer", "language": "en", "kind": "Trailer",
             "official": true, "size": 2160, "published_at": null }])
    .to_string()
}

/// Two movies and a series with trailers listed on TMDB, and a movie with a trailer in its
/// folder.
async fn library(pool: &SqlitePool) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m'),
             (2, 'shows', 'shows', '/s');
         INSERT INTO tmdb_movies (tmdb_id, title, fetched_at, videos) VALUES
             (11, 'One', 0, '{a}'), (12, 'Two', 0, '{b}'), (13, 'Own', 0, '{c}');
         INSERT INTO tmdb_series (tmdb_id, name, fetched_at, videos) VALUES (21, 'Show', 0, '{d}');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES
             (1, 1, 'One', 'One', 1, 'matched', 11), (2, 1, 'Two', 'Two', 1, 'matched', 12),
             (3, 1, 'Own', 'Own', 1, 'matched', 13);
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation,
             match_status, match_tmdb_id) VALUES (1, 2, 'Show', 'Show', 1, 'matched', 21);
         INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation)
             VALUES (1099511627776, 1, 'Own/Own-trailer.mkv', 1, 1, 3, 'trailer', 'Own', 1, 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, added_at,
             movie_id, series_id, season, episode, first_seen_generation, seen_generation)
             VALUES (1, 1, 'One/one.mkv', 1, 1, 10, 1, NULL, NULL, NULL, 1, 1),
                 (2, 1, 'Two/two.mkv', 1, 1, 20, 2, NULL, NULL, NULL, 1, 1),
                 (3, 1, 'Own/own.mkv', 1, 1, 40, 3, NULL, NULL, NULL, 1, 1),
                 (4, 2, 'Show/S01E01.mkv', 1, 1, 30, NULL, 1, 1, 1, 1, 1);",
        a = videos("aaaaaaaaaaa"),
        b = videos("bbbbbbbbbbb"),
        c = videos("ccccccccccc"),
        d = videos("ddddddddddd"),
    )))
    .execute(pool)
    .await
    .unwrap();
}

async fn state(pool: &SqlitePool, title: Title) -> Option<(String, String, i64, Option<String>)> {
    let (movie, series) = title.ids();
    sqlx::query_as(
        "SELECT state, video, max_height, file FROM downloaded_trailers
         WHERE movie_id IS ? AND series_id IS ?",
    )
    .bind(movie)
    .bind(series)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn queues_one_title_at_a_time_and_keeps_what_the_downloader_got() {
    let pool = db::open_in_memory().await;
    let scratch = tempfile::tempdir().unwrap();
    let folders = folders(scratch.path());
    library(&pool).await;

    // Nothing while held (off, or video transcoding).
    assert_eq!(step(&pool, &folders, 2160, || true).await.unwrap(), None);

    // The newest first (a series before movies added before it), never one with a trailer in its
    // folder.
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        Some(Title::Series(1))
    );
    let job: Value =
        serde_json::from_slice(&std::fs::read(folders.jobs.join("queue/series-1.json")).unwrap())
            .unwrap();
    assert_eq!(
        job,
        json!({ "id": "series-1", "video": "ddddddddddd", "max_height": 2160 })
    );
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        None,
        "one at a time"
    );

    // Downloaded: the video moves under the config folder, and the next title is queued.
    fake_downloader(&folders.jobs, "", "");
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        Some(Title::Movie(2))
    );
    assert_eq!(
        state(&pool, Title::Series(1)).await,
        Some((
            "done".into(),
            "ddddddddddd".into(),
            2160,
            Some("series-1.mp4".into())
        ))
    );
    let video = std::fs::read(folders.videos.join("series-1.mp4")).unwrap();
    assert_eq!(&video[4..8], b"ftyp");

    // Not on YouTube: kept as such, not asked again.
    fake_downloader(&folders.jobs, "movie-2", "");
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        Some(Title::Movie(1))
    );
    assert_eq!(
        state(&pool, Title::Movie(2)).await.unwrap().0,
        "unavailable"
    );

    // A failure waits a day before another try.
    fake_downloader(&folders.jobs, "", "movie-1");
    assert_eq!(step(&pool, &folders, 2160, || false).await.unwrap(), None);
    assert_eq!(state(&pool, Title::Movie(1)).await.unwrap().0, "failed");
    sqlx::query("UPDATE downloaded_trailers SET finished_at = 0 WHERE movie_id = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        Some(Title::Movie(1))
    );
    fake_downloader(&folders.jobs, "", "");
    assert_eq!(step(&pool, &folders, 2160, || false).await.unwrap(), None);
    assert_eq!(
        coverage(&pool).await.unwrap(),
        Coverage {
            titles: 3,
            done: 2,
            queued: 0,
            unavailable: 1,
            failed: 0
        }
    );

    // A lower quality set: each is downloaded again at it, the one YouTube hadn't asked again.
    assert_eq!(
        step(&pool, &folders, 1080, || false).await.unwrap(),
        Some(Title::Series(1))
    );
    assert_eq!(
        state(&pool, Title::Series(1)).await.unwrap().2,
        1080,
        "the quality it's asked for"
    );

    // A title that leaves the library, or gets a trailer in its folder, loses the one here.
    fake_downloader(&folders.jobs, "", "");
    step(&pool, &folders, 1080, || true).await.unwrap();
    sqlx::query("UPDATE series SET missing_since = 1 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO media_extras (id, library_id, rel_path, size, modified_at, movie_id, kind,
             title, first_seen_generation, seen_generation)
         VALUES (1099511627777, 1, 'One/One-trailer.mkv', 1, 1, 1, 'trailer', 'One', 1, 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    step(&pool, &folders, 1080, || true).await.unwrap();
    assert_eq!(state(&pool, Title::Series(1)).await, None);
    assert_eq!(state(&pool, Title::Movie(1)).await, None);
    assert!(!folders.videos.join("series-1.mp4").exists());
    assert!(!folders.videos.join("movie-1.mp4").exists());
}

#[tokio::test]
async fn a_new_trailer_on_tmdb_is_downloaded_in_place_of_the_old() {
    let pool = db::open_in_memory().await;
    let scratch = tempfile::tempdir().unwrap();
    let folders = folders(scratch.path());
    library(&pool).await;
    for _ in 0..3 {
        step(&pool, &folders, 2160, || false).await.unwrap();
        fake_downloader(&folders.jobs, "", "");
    }
    step(&pool, &folders, 2160, || true).await.unwrap();
    assert_eq!(state(&pool, Title::Movie(2)).await.unwrap().0, "done");
    sqlx::query("UPDATE tmdb_movies SET videos = ? WHERE tmdb_id = 12")
        .bind(videos("eeeeeeeeeee"))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        Some(Title::Movie(2))
    );
    assert_eq!(
        state(&pool, Title::Movie(2)).await.unwrap().1,
        "eeeeeeeeeee"
    );
    // The new one fails: the old one stays, and plays meanwhile.
    fake_downloader(&folders.jobs, "", "movie-2");
    step(&pool, &folders, 2160, || true).await.unwrap();
    assert_eq!(
        state(&pool, Title::Movie(2)).await,
        Some((
            "failed".into(),
            "eeeeeeeeeee".into(),
            2160,
            Some("movie-2.mp4".into())
        ))
    );
    assert!(folders.videos.join("movie-2.mp4").exists());
}

#[tokio::test]
async fn a_video_that_isnt_an_mp4_or_leaves_the_folder_is_a_failure() {
    let pool = db::open_in_memory().await;
    let scratch = tempfile::tempdir().unwrap();
    let folders = folders(scratch.path());
    library(&pool).await;
    assert_eq!(
        step(&pool, &folders, 2160, || false).await.unwrap(),
        Some(Title::Series(1))
    );
    let done = folders.jobs.join("done");
    std::fs::write(done.join("series-1.mp4"), b"<html>not a video</html>").unwrap();
    std::fs::write(
        done.join("series-1.json"),
        json!({"id": "series-1", "ok": true, "file": "series-1.mp4"}).to_string(),
    )
    .unwrap();
    step(&pool, &folders, 2160, || true).await.unwrap();
    assert_eq!(state(&pool, Title::Series(1)).await.unwrap().0, "failed");
    assert!(!folders.videos.join("series-1.mp4").exists());
    assert_eq!(std::fs::read_dir(&done).unwrap().count(), 0, "nothing left");

    assert_eq!(plain("../queue/x"), None);
    assert_eq!(plain(".hidden"), None);
    assert_eq!(Title::from_job("movie-3"), Some(Title::Movie(3)));
    assert_eq!(Title::from_job("other-3"), None);
    assert_eq!(
        video_path(Path::new("/config/trailers"), "../../etc/passwd"),
        Path::new("/config/trailers/passwd")
    );
}

#[tokio::test]
async fn songs_answers_in_the_shared_folder_are_left_for_songs() {
    let pool = db::open_in_memory().await;
    let scratch = tempfile::tempdir().unwrap();
    let folders = folders(scratch.path());
    let done = folders.jobs.join("done");
    std::fs::write(
        done.join("song-2-search.json"),
        r#"{"id":"song-2-search","ok":true}"#,
    )
    .unwrap();
    std::fs::write(done.join("movie-9.json"), r#"{"id":"movie-9","ok":true}"#).unwrap();
    read_answers(&pool, &folders).await.unwrap();
    assert!(
        done.join("song-2-search.json").exists(),
        "a song's answer stays"
    );
    assert!(
        !done.join("movie-9.json").exists(),
        "a trailer's is dealt with"
    );
}
