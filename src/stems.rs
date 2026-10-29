//! Splitting music into its parts with the separator beside Melete
//! (`stems/`): the voice in full, and drums, bass, guitar and other for the first and last
//! minute and a half, adding back to the track exactly. Melete picks the tracks and keeps the
//! record; the separator does the work on the GPU. They talk through a folder of job files:
//! Melete writes `queue/<track>.json`, the separator answers with `done/<track>.json`.
//!
//! Only while Auto DJ is on (`MELETE_AUTO_DJ`, off by default); answers to jobs already
//! queued are still taken when it's turned off. One track at a time, and only while no video
//! is being transcoded, so the two don't fight
//! over the GPU. The newest tracks go first. A track whose file changes is separated again;
//! one that leaves the library takes its parts with it. A failure is tried again a day later,
//! three times at most.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::config::Stems;
use crate::db::unix_now;
use crate::transcode::Transcoder;

/// How much of each end of a track gets its band parts: enough for the planner's longest mix
/// (32 bars) down to about 90 BPM, from the mix-out back and the mix-in on.
pub const EDGE_SECONDS: i64 = 90;
const EVERY: Duration = Duration::from_secs(10);
const TRIES: i64 = 3;
const RETRY_AFTER: i64 = 24 * 3600;

/// The separator's answer to one job.
#[derive(Debug, Deserialize)]
struct Answer {
    id: String,
    ok: bool,
    manifest: Option<Value>,
    error: Option<String>,
}

/// Works for as long as the server runs: reads answers, tidies, and queues the next track.
pub async fn work(pool: SqlitePool, stems: Stems, transcoder: Arc<Transcoder>) {
    for sub in ["queue", "done"] {
        if let Err(err) = std::fs::create_dir_all(stems.jobs.join(sub)) {
            tracing::warn!(error = %err, "can't use the stem separator's job folder; no music is split into parts");
            return;
        }
    }
    loop {
        let on = crate::settings::auto_dj(&pool).await.unwrap_or(false);
        if let Err(err) = step(&pool, &stems, || !on || transcoder.busy()).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "splitting music into parts stopped for now"
            );
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// One round: answers recorded, the parts of tracks that left removed, and, when nothing's
/// waiting and nothing says to hold (Auto DJ off, or video being transcoded), the next track
/// queued. Returns the track queued.
pub async fn step(
    pool: &SqlitePool,
    stems: &Stems,
    hold: impl Fn() -> bool,
) -> anyhow::Result<Option<i64>> {
    read_answers(pool, &stems.jobs).await?;
    tidy(pool, &stems.dir).await?;
    let waiting: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM music_stems WHERE state = 'queued')")
            .fetch_one(pool)
            .await?;
    if waiting || hold() {
        return Ok(None);
    }
    let now = unix_now();
    // New tracks, changed ones, and failures due another try; the newest first.
    let next: Option<(i64, String, String, i64, i64)> = sqlx::query_as(
        "SELECT t.id, l.path, t.rel_path, t.size, t.modified_at
         FROM music_tracks t JOIN libraries l ON l.id = t.library_id
         LEFT JOIN music_stems s ON s.track_id = t.id
         WHERE t.missing_since IS NULL AND (
                 s.track_id IS NULL
              OR (s.state = 'done' AND (s.size <> t.size OR s.modified_at <> t.modified_at))
              OR (s.state = 'failed' AND s.attempts < ?1 AND s.finished_at <= ?2))
         ORDER BY t.id DESC LIMIT 1",
    )
    .bind(TRIES)
    .bind(now - RETRY_AFTER)
    .fetch_optional(pool)
    .await?;
    let Some((track, library, rel_path, size, modified_at)) = next else {
        return Ok(None);
    };
    let source = Path::new(&library).join(&rel_path);
    let folder = folder_for(track);
    let job = json!({
        "id": track.to_string(),
        "source": source,
        "folder": folder,
        "edge_seconds": EDGE_SECONDS,
    });
    let queued = stems.jobs.join("queue").join(format!("{track}.json"));
    let partial = stems.jobs.join("queue").join(format!(".{track}.json.tmp"));
    std::fs::write(&partial, serde_json::to_vec(&job)?)?;
    sqlx::query(
        "INSERT INTO music_stems (track_id, state, size, modified_at, folder, queued_at)
         VALUES (?, 'queued', ?, ?, ?, ?)
         ON CONFLICT (track_id) DO UPDATE SET state = 'queued', size = excluded.size,
             modified_at = excluded.modified_at, folder = excluded.folder, manifest = NULL,
             error = NULL, queued_at = excluded.queued_at, finished_at = NULL",
    )
    .bind(track)
    .bind(size)
    .bind(modified_at)
    .bind(&folder)
    .bind(now)
    .execute(pool)
    .await?;
    // Only now is it the separator's to take.
    std::fs::rename(&partial, &queued)?;
    Ok(Some(track))
}

/// A track's parts' folder under the stems folder: spread over a hundred folders, so none holds
/// the whole library.
pub fn folder_for(track: i64) -> String {
    format!("{:02}/{track}", track % 100)
}

async fn read_answers(pool: &SqlitePool, jobs: &Path) -> anyhow::Result<()> {
    let Ok(entries) = std::fs::read_dir(jobs.join("done")) else {
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
            && let Ok(track) = answer.id.parse::<i64>()
        {
            let now = unix_now();
            match (answer.ok, answer.manifest) {
                (true, Some(manifest)) => {
                    sqlx::query(
                        "UPDATE music_stems SET state = 'done', manifest = ?, error = NULL,
                             attempts = 0, finished_at = ?
                         WHERE track_id = ? AND state = 'queued'",
                    )
                    .bind(manifest.to_string())
                    .bind(now)
                    .bind(track)
                    .execute(pool)
                    .await?;
                }
                _ => {
                    let why = answer
                        .error
                        .unwrap_or_else(|| "the separator gave no reason".into());
                    tracing::warn!(track, error = %why, "a track couldn't be split into parts");
                    sqlx::query(
                        "UPDATE music_stems SET state = 'failed', error = ?, attempts = attempts + 1,
                             finished_at = ?
                         WHERE track_id = ? AND state = 'queued'",
                    )
                    .bind(why.chars().take(500).collect::<String>())
                    .bind(now)
                    .bind(track)
                    .execute(pool)
                    .await?;
                }
            }
        }
        // Read, or unreadable: either way it's been dealt with.
        let _ = std::fs::remove_file(&path);
    }
    Ok(())
}

/// Removes the parts of tracks no longer in the library.
async fn tidy(pool: &SqlitePool, dir: &Path) -> anyhow::Result<()> {
    let gone: Vec<(i64, String)> = sqlx::query_as(
        "SELECT s.track_id, s.folder FROM music_stems s JOIN music_tracks t ON t.id = s.track_id
         WHERE t.missing_since IS NOT NULL AND s.state <> 'queued'",
    )
    .fetch_all(pool)
    .await?;
    for (track, folder) in gone {
        let path = parts_dir(dir, &folder);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        sqlx::query("DELETE FROM music_stems WHERE track_id = ?")
            .bind(track)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// A folder named in the database, under the stems folder and nowhere else.
pub fn parts_dir(dir: &Path, folder: &str) -> PathBuf {
    let clean: PathBuf = Path::new(folder)
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect();
    dir.join(clean)
}

/// How far splitting the library has got.
#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Coverage {
    /// Present tracks in the music library.
    pub tracks: i64,
    /// Tracks split into parts.
    pub done: i64,
    /// Tracks waiting to be split.
    pub queued: i64,
    /// Tracks whose split failed.
    pub failed: i64,
    /// The latest failure's reason.
    pub last_error: Option<String>,
}

/// How far splitting the library has got, counted from the database.
///
/// # Errors
///
/// Fails only when the database can't be read.
pub async fn coverage(pool: &SqlitePool) -> sqlx::Result<Coverage> {
    let (tracks, done, queued, failed): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM music_tracks WHERE missing_since IS NULL),
                (SELECT count(*) FROM music_stems WHERE state = 'done'),
                (SELECT count(*) FROM music_stems WHERE state = 'queued'),
                (SELECT count(*) FROM music_stems WHERE state = 'failed')",
    )
    .fetch_one(pool)
    .await?;
    let last_error = sqlx::query_scalar(
        "SELECT error FROM music_stems WHERE state = 'failed' ORDER BY finished_at DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(Coverage {
        tracks,
        done,
        queued,
        failed,
        last_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn library(pool: &SqlitePool) {
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', '/media/music');
             INSERT INTO music_albums (id, library_id, key, title, artist_name, folder, seen_generation)
                 VALUES (1, 1, 'k1', 'One', 'Alder', 'Alder/One', 1);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, album_id, tagged, title, genres) VALUES
                 (101, 1, 'Alder/One/01.flac', 10, 1, 1, 1, 1, 1, 'A1', '[]'),
                 (102, 1, 'Alder/One/02.flac', 20, 1, 1, 1, 1, 1, 'A2', '[]');"#,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    fn answer(jobs: &Path, track: i64, body: Value) {
        std::fs::write(
            jobs.join("done").join(format!("{track}.json")),
            body.to_string(),
        )
        .unwrap();
    }

    async fn state(pool: &SqlitePool, track: i64) -> Option<(String, i64)> {
        sqlx::query_as("SELECT state, attempts FROM music_stems WHERE track_id = ?")
            .bind(track)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn queues_one_track_at_a_time_and_records_the_answers() {
        let pool = db::open_in_memory().await;
        library(&pool).await;
        let scratch = tempfile::tempdir().unwrap();
        let stems = Stems {
            dir: scratch.path().join("stems"),
            jobs: scratch.path().join("jobs"),
        };
        for sub in ["queue", "done"] {
            std::fs::create_dir_all(stems.jobs.join(sub)).unwrap();
        }
        // Nothing while video transcodes.
        assert_eq!(step(&pool, &stems, || true).await.unwrap(), None);
        // The newest first, as a job file with where it is and where its parts go.
        assert_eq!(step(&pool, &stems, || false).await.unwrap(), Some(102));
        let job: Value =
            serde_json::from_slice(&std::fs::read(stems.jobs.join("queue/102.json")).unwrap())
                .unwrap();
        assert_eq!(
            job,
            json!({"id": "102", "source": "/media/music/Alder/One/02.flac", "folder": "02/102", "edge_seconds": 90})
        );
        assert_eq!(
            step(&pool, &stems, || false).await.unwrap(),
            None,
            "one at a time"
        );

        answer(
            &stems.jobs,
            102,
            json!({"id": "102", "ok": true, "manifest": {"frames": 5, "regions": []}}),
        );
        assert_eq!(step(&pool, &stems, || false).await.unwrap(), Some(101));
        assert_eq!(state(&pool, 102).await, Some(("done".into(), 0)));
        assert!(
            std::fs::read_dir(stems.jobs.join("done"))
                .unwrap()
                .next()
                .is_none(),
            "answers are taken"
        );

        // A failure waits a day before another try.
        answer(
            &stems.jobs,
            101,
            json!({"id": "101", "ok": false, "error": "RuntimeError: out of memory"}),
        );
        assert_eq!(step(&pool, &stems, || false).await.unwrap(), None);
        assert_eq!(state(&pool, 101).await, Some(("failed".into(), 1)));
        assert_eq!(
            coverage(&pool).await.unwrap().last_error.as_deref(),
            Some("RuntimeError: out of memory")
        );
        sqlx::query("UPDATE music_stems SET finished_at = 0 WHERE track_id = 101")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(step(&pool, &stems, || false).await.unwrap(), Some(101));
        answer(
            &stems.jobs,
            101,
            json!({"id": "101", "ok": true, "manifest": {}}),
        );
        assert_eq!(
            step(&pool, &stems, || false).await.unwrap(),
            None,
            "all done"
        );

        // A changed file is split again.
        sqlx::query("UPDATE music_tracks SET size = 21 WHERE id = 102")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(step(&pool, &stems, || false).await.unwrap(), Some(102));
        answer(
            &stems.jobs,
            102,
            json!({"id": "102", "ok": true, "manifest": {}}),
        );

        // A track that leaves the library takes its parts with it.
        let parts = parts_dir(&stems.dir, "01/101");
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("voice.flac"), b"x").unwrap();
        sqlx::query("UPDATE music_tracks SET missing_since = 1 WHERE id = 101")
            .execute(&pool)
            .await
            .unwrap();
        step(&pool, &stems, || false).await.unwrap();
        assert!(!parts.exists());
        assert_eq!(state(&pool, 101).await, None);
        assert_eq!(
            coverage(&pool).await.unwrap(),
            Coverage {
                tracks: 1,
                done: 1,
                queued: 0,
                failed: 0,
                last_error: None
            }
        );
    }

    #[test]
    fn a_folder_from_the_database_stays_under_the_stems_folder() {
        let dir = Path::new("/stems");
        assert_eq!(parts_dir(dir, "02/102"), Path::new("/stems/02/102"));
        assert_eq!(parts_dir(dir, "../../etc"), Path::new("/stems/etc"));
        assert_eq!(
            parts_dir(dir, "/etc/passwd"),
            Path::new("/stems/etc/passwd")
        );
        assert_eq!(folder_for(7), "07/7");
    }
}
