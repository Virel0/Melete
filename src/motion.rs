//! Motion covers made from albums' still covers by the renderer beside Melete, in a
//! container of its own: a short loop with a slow camera move over the cover's depth, for
//! albums without a motion cover of their own. Melete picks the albums and keeps the record; the
//! renderer does the work. They talk through a folder of job files, as the stem separator does:
//! Melete writes `queue/<album>.cover` (the cover's bytes, so the renderer needs no access to the
//! library) and `queue/<album>.json`; the renderer answers with `done/<album>.json` and, when it
//! made one, the video beside it, which Melete moves under its config folder.
//!
//! Only while motion covers are on (`MELETE_MOTION_COVERS`, off by default); answers to jobs
//! already queued are still taken when they're turned off. One album at a time, newest first,
//! and only while no video is being transcoded and no music is being split into parts. A cover
//! that's mostly text or flat is skipped by the renderer, which says why. A changed cover is made
//! again (the cover is named as the colours job names it, by where it is and its bytes' hash), as
//! is every one when [`VERSION`] changes. An album that leaves the library, or gets a motion
//! cover of its own, loses the one made here. A failure is tried again a day later, three times
//! at most.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::transcode::Transcoder;

mod showing;

pub use showing::*;

/// The loop every renderer job asks for: 8 seconds at 30 frames a second, a gentle circle,
/// the cover at its own size up to 1080 pixels, never larger. Changing any of it means raising
/// [`VERSION`], so every motion cover is made again.
pub const LOOP: Loop = Loop {
    seconds: 8,
    fps: 30,
    max_side: 1080,
    path: "circle",
    strength: 0.03,
};
/// Raised whenever [`LOOP`] or what's asked of the renderer changes.
pub const VERSION: i64 = 1;
const EVERY: Duration = Duration::from_secs(10);
const TRIES: i64 = 3;
const RETRY_AFTER: i64 = 24 * 3600;
/// The largest video taken from the renderer; an 8-second loop is a few megabytes at most.
const MAX_VIDEO: u64 = 64 * 1024 * 1024;

/// What the renderer is asked to make of a cover.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Loop {
    /// The loop's length, in seconds.
    pub seconds: u32,
    /// Frames a second.
    pub fps: u32,
    /// The longest side rendered; a smaller cover is rendered at its own size.
    pub max_side: u32,
    /// The camera's path.
    pub path: &'static str,
    /// How far the camera moves, as a share of the cover's side.
    pub strength: f64,
}

/// Where the jobs and the videos are.
#[derive(Debug, Clone)]
pub struct Folders {
    /// Shared with the renderer (`MELETE_MOTION_JOBS`).
    pub jobs: PathBuf,
    /// The finished videos, under the config folder.
    pub videos: PathBuf,
    /// Where the Cover Art Archive's covers are cached (`<config>/artwork/music`).
    pub covers: PathBuf,
}

/// The finished videos' folder under the config folder.
pub fn videos_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("motion")
}

/// The renderer's answer to one job.
#[derive(Debug, Deserialize)]
struct Answer {
    id: String,
    ok: bool,
    /// The video's file name in `done/`, when one was made.
    video: Option<String>,
    /// Why the cover was skipped (mostly text, flat), when it was.
    skipped: Option<String>,
    error: Option<String>,
    details: Option<Value>,
}

/// Works for as long as the server runs: reads answers, tidies, and queues the next album.
pub async fn work(pool: SqlitePool, folders: Folders, transcoder: Arc<Transcoder>) {
    for dir in [
        folders.jobs.join("queue"),
        folders.jobs.join("done"),
        folders.videos.clone(),
    ] {
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %err, "can't use the motion cover folders; no motion covers are made");
            return;
        }
    }
    loop {
        let on = crate::settings::motion_covers(&pool).await.unwrap_or(false);
        if let Err(err) = step(&pool, &folders, || !on || transcoder.busy()).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "making motion covers stopped for now"
            );
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// One round: answers recorded, the motion covers of albums that left (or got their own)
/// removed, and, when nothing's waiting and nothing says to hold (motion covers off, video being
/// transcoded, music being split into parts), the next album queued. Returns the album queued.
pub async fn step(
    pool: &SqlitePool,
    folders: &Folders,
    hold: impl Fn() -> bool,
) -> anyhow::Result<Option<i64>> {
    read_answers(pool, folders).await?;
    tidy(pool, &folders.videos).await?;
    let (waiting, separating): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM music_motion_covers WHERE state = 'queued'),
                EXISTS (SELECT 1 FROM music_stems WHERE state = 'queued')",
    )
    .fetch_one(pool)
    .await?;
    if waiting || separating || hold() {
        return Ok(None);
    }
    let now = unix_now();
    // Albums with a readable cover and no motion cover of their own: never tried, made from
    // another cover or loop, or failed and due another try. The newest first.
    let next: Option<(i64, String)> = sqlx::query_as(
        "SELECT a.id, c.source FROM music_albums a
         JOIN music_cover_colours c ON c.album_id = a.id AND c.colours IS NOT NULL
         LEFT JOIN music_motion_covers m ON m.album_id = a.id
         WHERE a.missing_since IS NULL AND a.motion_file IS NULL AND (
                 m.album_id IS NULL
              OR m.source <> c.source OR m.version <> ?1
              OR (m.state = 'failed' AND m.attempts < ?2 AND m.finished_at <= ?3))
         ORDER BY a.id DESC LIMIT 1",
    )
    .bind(VERSION)
    .bind(TRIES)
    .bind(now - RETRY_AFTER)
    .fetch_optional(pool)
    .await?;
    let Some((album, measured)) = next else {
        return Ok(None);
    };
    let found = crate::cover_colours::current(pool, &folders.covers, album).await;
    let (source, bytes) = match found {
        Ok(Some(found)) if found.0 == measured => found,
        // Changed since the colours job last looked: it looks again, and this waits for it, so
        // the two agree on what the cover is.
        Ok(_) => {
            crate::jobs::COVERS.notify_one();
            return Ok(None);
        }
        Err(err) => {
            fail(
                pool,
                album,
                &measured,
                &format!("couldn't read the cover: {err:#}"),
            )
            .await?;
            return Ok(None);
        }
    };
    let queue = folders.jobs.join("queue");
    let job = json!({
        "id": album.to_string(),
        "cover": format!("{album}.cover"),
        "version": VERSION,
        "loop": LOOP,
    });
    write_then_rename(&queue.join(format!("{album}.cover")), &bytes)?;
    let partial = queue.join(format!(".{album}.json.tmp"));
    std::fs::write(&partial, serde_json::to_vec(&job)?)?;
    sqlx::query(
        "INSERT INTO music_motion_covers (album_id, state, source, version, queued_at)
         VALUES (?1, 'queued', ?2, ?3, ?4)
         ON CONFLICT (album_id) DO UPDATE SET state = 'queued', file = NULL, reason = NULL,
             details = NULL, queued_at = excluded.queued_at, finished_at = NULL,
             attempts = CASE WHEN source = excluded.source AND version = excluded.version
                 THEN attempts ELSE 0 END,
             source = excluded.source, version = excluded.version",
    )
    .bind(album)
    .bind(&source)
    .bind(VERSION)
    .bind(now)
    .execute(pool)
    .await?;
    // Only now is it the renderer's to take.
    std::fs::rename(&partial, queue.join(format!("{album}.json")))?;
    Ok(Some(album))
}

fn write_then_rename(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)
}

/// Records a failure for an album that wasn't queued (its cover couldn't be read).
async fn fail(pool: &SqlitePool, album: i64, source: &str, why: &str) -> sqlx::Result<()> {
    let now = unix_now();
    sqlx::query(
        "INSERT INTO music_motion_covers (album_id, state, source, version, reason, attempts,
             queued_at, finished_at)
         VALUES (?1, 'failed', ?2, ?3, ?4, 1, ?5, ?5)
         ON CONFLICT (album_id) DO UPDATE SET state = 'failed', file = NULL, details = NULL,
             reason = excluded.reason, finished_at = excluded.finished_at,
             attempts = CASE WHEN source = excluded.source AND version = excluded.version
                 THEN attempts + 1 ELSE 1 END,
             source = excluded.source, version = excluded.version",
    )
    .bind(album)
    .bind(source)
    .bind(VERSION)
    .bind(why.chars().take(500).collect::<String>())
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// A plain file name from the renderer: nothing that leaves the folder it's in.
fn plain(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.starts_with('.');
    ok.then_some(name)
}

/// Moves a finished video into the videos folder as `<album>.mp4`, checking it looks like an
/// MP4 of a sensible size first. Returns its name there.
fn take_video(done: &Path, name: &str, videos: &Path, album: i64) -> anyhow::Result<String> {
    let name = plain(name).ok_or_else(|| anyhow::anyhow!("the renderer named no usable video"))?;
    let from = done.join(name);
    let size = std::fs::metadata(&from)?.len();
    anyhow::ensure!(
        (12..=MAX_VIDEO).contains(&size),
        "the video is {size} bytes"
    );
    let mut head = [0u8; 12];
    std::io::Read::read_exact(&mut std::fs::File::open(&from)?, &mut head)?;
    anyhow::ensure!(&head[4..8] == b"ftyp", "the video isn't an MP4");
    std::fs::create_dir_all(videos)?;
    let file = format!("{album}.mp4");
    let partial = videos.join(format!(".{file}.partial"));
    // The jobs folder may be on another volume, so copy rather than rename across.
    std::fs::copy(&from, &partial)?;
    std::fs::rename(&partial, videos.join(&file))?;
    let _ = std::fs::remove_file(&from);
    Ok(file)
}

async fn read_answers(pool: &SqlitePool, folders: &Folders) -> anyhow::Result<()> {
    let done = folders.jobs.join("done");
    let Ok(entries) = std::fs::read_dir(&done) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let answer: Option<Answer> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if let Some(answer) = answer
            && let Ok(album) = answer.id.parse::<i64>()
        {
            record(pool, folders, album, answer).await?;
            let _ = std::fs::remove_file(folders.jobs.join("queue").join(format!("{album}.cover")));
        }
        // Read, or unreadable: either way it's been dealt with.
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

async fn record(
    pool: &SqlitePool,
    folders: &Folders,
    album: i64,
    answer: Answer,
) -> anyhow::Result<()> {
    let done = folders.jobs.join("done");
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM music_motion_covers WHERE album_id = ? AND state = 'queued')",
    )
    .bind(album)
    .fetch_one(pool)
    .await?;
    if !queued {
        // Nothing asked for it (a job from before a restart that was already answered).
        if let Some(name) = answer.video.as_deref().and_then(plain) {
            let _ = std::fs::remove_file(done.join(name));
        }
        return Ok(());
    }
    let now = unix_now();
    let details = answer.details.map(|d| d.to_string());
    let outcome = match (answer.ok, answer.skipped, answer.video) {
        (true, Some(why), _) => Ok((None, Some(why))),
        (true, None, Some(name)) => take_video(&done, &name, &folders.videos, album)
            .map(|file| (Some(file), None))
            .map_err(|err| format!("{err:#}")),
        (true, None, None) => Err("the renderer answered with neither a video nor a reason".into()),
        (false, _, video) => {
            if let Some(name) = video.as_deref().and_then(plain) {
                let _ = std::fs::remove_file(done.join(name));
            }
            Err(answer
                .error
                .unwrap_or_else(|| "the renderer gave no reason".into()))
        }
    };
    match outcome {
        Ok((file, skipped)) => {
            let state = if file.is_some() { "done" } else { "skipped" };
            sqlx::query(
                "UPDATE music_motion_covers SET state = ?, file = ?, reason = ?, details = ?,
                     attempts = 0, finished_at = ?
                 WHERE album_id = ? AND state = 'queued'",
            )
            .bind(state)
            .bind(file)
            .bind(skipped.map(|s| s.chars().take(200).collect::<String>()))
            .bind(details)
            .bind(now)
            .bind(album)
            .execute(pool)
            .await?;
        }
        Err(why) => {
            tracing::warn!(album, error = %why, "an album's motion cover couldn't be made");
            sqlx::query(
                "UPDATE music_motion_covers SET state = 'failed', file = NULL, reason = ?,
                     details = ?, attempts = attempts + 1, finished_at = ?
                 WHERE album_id = ? AND state = 'queued'",
            )
            .bind(why.chars().take(500).collect::<String>())
            .bind(details)
            .bind(now)
            .bind(album)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Removes the motion covers made for albums that left the library, got a motion cover of their
/// own, or no longer have a cover.
async fn tidy(pool: &SqlitePool, videos: &Path) -> anyhow::Result<()> {
    let gone: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT m.album_id, m.file FROM music_motion_covers m
         JOIN music_albums a ON a.id = m.album_id
         WHERE m.state <> 'queued' AND (a.missing_since IS NOT NULL OR a.motion_file IS NOT NULL
             OR NOT EXISTS (SELECT 1 FROM music_cover_colours c WHERE c.album_id = a.id))",
    )
    .fetch_all(pool)
    .await?;
    for (album, file) in gone {
        if let Some(file) = file {
            let path = video_path(videos, &file);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        sqlx::query("DELETE FROM music_motion_covers WHERE album_id = ?")
            .bind(album)
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db;

    /// The stand-in renderer: answers every queued job the way the real one does, with a short
    /// test pattern made by ffmpeg instead of a rendered loop.
    pub(crate) fn fake_renderer(jobs: &Path, skip: &str, fail: &str) {
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/motion/fake-renderer.sh");
        let status = std::process::Command::new("sh")
            .arg(script)
            .arg(jobs)
            .env("SKIP", skip)
            .env("FAIL", fail)
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// A music library with three albums, each with a cover file in its folder, and the
    /// colours job's record of each cover (as it names them). Returns the library's folder.
    pub(crate) async fn library(pool: &SqlitePool, covers: &Path) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?)")
            .bind(root.path().to_str().unwrap())
            .execute(pool)
            .await
            .unwrap();
        for album in 1..=3 {
            let folder = format!("a{album}");
            std::fs::create_dir_all(root.path().join(&folder)).unwrap();
            std::fs::write(
                root.path().join(&folder).join("cover.jpg"),
                format!("\u{ff}cover {album}"),
            )
            .unwrap();
            sqlx::query(
                "INSERT INTO music_albums (id, library_id, key, title, artist_name, folder,
                     seen_generation, cover_file) VALUES (?1, 1, ?2, ?2, 'Alder', ?3, 1, ?4)",
            )
            .bind(album)
            .bind(format!("k{album}"))
            .bind(&folder)
            .bind(format!("{folder}/cover.jpg"))
            .execute(pool)
            .await
            .unwrap();
            measured(pool, covers, album).await;
        }
        root
    }

    /// Records the album's cover as the colours job would.
    pub(crate) async fn measured(pool: &SqlitePool, covers: &Path, album: i64) {
        let (source, _) = crate::cover_colours::current(pool, covers, album)
            .await
            .unwrap()
            .unwrap();
        sqlx::query(
            "INSERT INTO music_cover_colours (album_id, source, colours, measured_at)
             VALUES (?, ?, '{}', 0)
             ON CONFLICT (album_id) DO UPDATE SET source = excluded.source",
        )
        .bind(album)
        .bind(source)
        .execute(pool)
        .await
        .unwrap();
    }

    pub(crate) fn folders(scratch: &Path) -> Folders {
        let folders = Folders {
            jobs: scratch.join("jobs"),
            videos: scratch.join("config/motion"),
            covers: scratch.join("config/artwork/music"),
        };
        for sub in ["queue", "done"] {
            std::fs::create_dir_all(folders.jobs.join(sub)).unwrap();
        }
        folders
    }

    async fn state(pool: &SqlitePool, album: i64) -> Option<(String, Option<String>, i64)> {
        sqlx::query_as("SELECT state, reason, attempts FROM music_motion_covers WHERE album_id = ?")
            .bind(album)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn queues_one_album_at_a_time_and_keeps_what_the_renderer_made() {
        let pool = db::open_in_memory().await;
        let scratch = tempfile::tempdir().unwrap();
        let folders = folders(scratch.path());
        let library = library(&pool, &folders.covers).await;

        // Nothing while held (off, or video transcoding), nor while music is split into parts.
        assert_eq!(step(&pool, &folders, || true).await.unwrap(), None);
        sqlx::raw_sql(
            "INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres)
                 VALUES (9, 1, 'a1/01.flac', 1, 1, 1, 1, 1, 1, 'T', '[]');
             INSERT INTO music_stems (track_id, state, size, modified_at, folder, queued_at)
                 VALUES (9, 'queued', 1, 1, '09/9', 0);",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), None);
        sqlx::query("DELETE FROM music_stems")
            .execute(&pool)
            .await
            .unwrap();

        // The newest first: the cover's bytes, and a job saying what loop to make.
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), Some(3));
        let job: Value =
            serde_json::from_slice(&std::fs::read(folders.jobs.join("queue/3.json")).unwrap())
                .unwrap();
        assert_eq!(
            job,
            json!({"id": "3", "cover": "3.cover", "version": 1, "loop": {
                "seconds": 8, "fps": 30, "max_side": 1080, "path": "circle", "strength": 0.03}})
        );
        assert_eq!(
            std::fs::read(folders.jobs.join("queue/3.cover")).unwrap(),
            "\u{ff}cover 3".as_bytes()
        );
        assert_eq!(
            step(&pool, &folders, || false).await.unwrap(),
            None,
            "one at a time"
        );

        // Made: the video moves under the config folder, and the next album is queued.
        fake_renderer(&folders.jobs, "", "");
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), Some(2));
        assert_eq!(state(&pool, 3).await, Some(("done".into(), None, 0)));
        assert_eq!(
            current_file(&pool, 3).await.unwrap().as_deref(),
            Some("3.mp4")
        );
        let video = std::fs::read(folders.videos.join("3.mp4")).unwrap();
        assert_eq!(&video[4..8], b"ftyp");
        assert!(
            std::fs::read_dir(folders.jobs.join("done"))
                .unwrap()
                .next()
                .is_none(),
            "answers and videos are taken"
        );

        // Skipped, with the renderer's reason.
        fake_renderer(&folders.jobs, "2", "");
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), Some(1));
        assert_eq!(
            state(&pool, 2).await,
            Some(("skipped".into(), Some("mostly text".into()), 0))
        );
        assert_eq!(
            status(&pool, 2).await.unwrap(),
            Some(json!({"state": "skipped", "reason": "mostly text"}))
        );

        // A failure waits a day before another try.
        fake_renderer(&folders.jobs, "", "1");
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), None);
        assert_eq!(
            state(&pool, 1).await,
            Some((
                "failed".into(),
                Some("the cover could not be read".into()),
                1
            ))
        );
        assert_eq!(
            coverage(&pool).await.unwrap(),
            Coverage {
                albums: 3,
                done: 1,
                queued: 0,
                skipped: 1,
                failed: 1,
                last_error: Some("the cover could not be read".into())
            }
        );
        sqlx::query("UPDATE music_motion_covers SET finished_at = 0 WHERE album_id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), Some(1));
        fake_renderer(&folders.jobs, "", "");
        assert_eq!(
            step(&pool, &folders, || false).await.unwrap(),
            None,
            "all done"
        );
        assert_eq!(state(&pool, 1).await, Some(("done".into(), None, 0)));

        // A changed cover: the old loop isn't shown, and it's made again once the colours job
        // has seen the change.
        std::fs::write(library.path().join("a3/cover.jpg"), "\u{ff}new cover 3").unwrap();
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), None);
        measured(&pool, &folders.covers, 3).await;
        assert_eq!(current_file(&pool, 3).await.unwrap(), None);
        assert_eq!(status(&pool, 3).await.unwrap(), None);
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), Some(3));
        fake_renderer(&folders.jobs, "", "");
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), None);
        assert_eq!(
            current_file(&pool, 3).await.unwrap().as_deref(),
            Some("3.mp4")
        );

        // An album that gets a motion cover of its own loses the one made here; so does one
        // that leaves the library.
        sqlx::query("UPDATE music_albums SET motion_file = 'a3/cover.mp4' WHERE id = 3")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE music_albums SET missing_since = 1 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), None);
        assert!(!folders.videos.join("3.mp4").exists());
        assert!(!folders.videos.join("1.mp4").exists());
        assert_eq!(state(&pool, 3).await, None);
        assert_eq!(state(&pool, 1).await, None);
        assert_eq!(state(&pool, 2).await.unwrap().0, "skipped");
    }

    #[tokio::test]
    async fn a_video_that_isnt_an_mp4_is_a_failure_not_a_motion_cover() {
        let pool = db::open_in_memory().await;
        let scratch = tempfile::tempdir().unwrap();
        let folders = folders(scratch.path());
        let _library = library(&pool, &folders.covers).await;
        assert_eq!(step(&pool, &folders, || false).await.unwrap(), Some(3));
        let done = folders.jobs.join("done");
        std::fs::write(done.join("3.mp4"), b"<html>not a video</html>").unwrap();
        std::fs::write(
            done.join("3.json"),
            json!({"id": "3", "ok": true, "video": "3.mp4"}).to_string(),
        )
        .unwrap();
        step(&pool, &folders, || true).await.unwrap();
        assert_eq!(
            state(&pool, 3).await,
            Some(("failed".into(), Some("the video isn't an MP4".into()), 1))
        );
        assert!(!folders.videos.join("3.mp4").exists());

        // A name that leaves the folder isn't followed.
        sqlx::query("UPDATE music_motion_covers SET state = 'queued', reason = NULL, finished_at = NULL WHERE album_id = 3")
            .execute(&pool)
            .await
            .unwrap();
        std::fs::write(
            done.join("3.json"),
            json!({"id": "3", "ok": true, "video": "../queue/3.cover"}).to_string(),
        )
        .unwrap();
        step(&pool, &folders, || true).await.unwrap();
        assert_eq!(
            state(&pool, 3).await.unwrap().1.as_deref(),
            Some("the renderer named no usable video")
        );
    }

    #[test]
    fn a_video_from_the_database_stays_in_the_videos_folder() {
        let dir = Path::new("/config/motion");
        assert_eq!(video_path(dir, "3.mp4"), Path::new("/config/motion/3.mp4"));
        assert_eq!(
            video_path(dir, "../../etc/passwd"),
            Path::new("/config/motion/passwd")
        );
        assert_eq!(plain("../x"), None);
        assert_eq!(plain(".hidden"), None);
        assert_eq!(plain("3.mp4"), Some("3.mp4"));
    }
}
