//! Trailers downloaded for titles that have none in their folder, by the trailer downloader
//! beside Melete in a container of its own, behind the VPN. Melete picks the titles and the
//! trailer (from TMDB's list of the title's videos, [`crate::metadata::chosen`]) and keeps the
//! record; the downloader fetches it from YouTube and converts it once so every player can play
//! it. They talk through a folder of job files, as the stem separator does: Melete writes
//! `queue/<job>.json` ({"id", "video", "max_height"}); the downloader answers with
//! `done/<job>.json` and, when it got one, the video beside it, which Melete moves under its
//! config folder.
//!
//! Only while trailer downloads are on (`MELETE_TRAILER_DOWNLOADS`, off by default); answers to
//! jobs already queued are still taken when they're turned off. One title at a time, newest
//! first, and only while no video is being transcoded and no music is being split into parts. A
//! trailer is at most the quality set (`MELETE_TRAILER_QUALITY`), and made again when the
//! trailer chosen for the title changes or the quality set does. YouTube not having it is kept
//! as such until the trailer chosen changes; a failure is tried again a day later, three times
//! at most. A title that leaves the library, or gets a trailer in its folder, loses the one
//! downloaded here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::metadata::{Trailer, chosen, languages};
use crate::transcode::Transcoder;

mod answers;

/// A plain file name from the downloader: nothing that leaves the folder it's in.
pub(crate) use answers::plain as plain_name;

use answers::*;

const EVERY: Duration = Duration::from_secs(30);
const TRIES: i64 = 3;
const RETRY_AFTER: i64 = 24 * 3600;
/// The largest video taken from the downloader; a 4K trailer of a few minutes is well under.
const MAX_VIDEO: u64 = 4 << 30;

/// Where the jobs and the videos are.
#[derive(Debug, Clone)]
pub struct Folders {
    /// Shared with the downloader (`MELETE_TRAILER_JOBS`).
    pub jobs: PathBuf,
    /// The finished videos, under the config folder.
    pub videos: PathBuf,
}

/// The finished videos' folder under the config folder.
pub fn videos_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("trailers")
}

/// Where downloaded trailers' ids start, as apps know them: the extras' range (so each is
/// played as an extra is), past any extra's.
pub const ID_BASE: i64 = 1 << 41;
// Written out where SQL needs it; kept in step here.
const _: () = assert!(ID_BASE == 2_199_023_255_552);

/// Whether an id names a downloaded trailer.
pub fn is_trailer_id(id: i64) -> bool {
    id >= ID_BASE
}

/// The id apps know a downloaded trailer by, from its row's.
pub fn playable_id(row: i64) -> i64 {
    ID_BASE + row
}

/// Records where the videos folder is, for stream links to name their files; at each start,
/// whether or not trailers are downloaded now, so those kept still play.
pub async fn remember_folder(pool: &SqlitePool, videos: &Path) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO app_folders (name, path) VALUES ('trailers', ?)
         ON CONFLICT (name) DO UPDATE SET path = excluded.path",
    )
    .bind(videos.to_string_lossy())
    .execute(pool)
    .await?;
    Ok(())
}

/// A title a trailer is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Title {
    Movie(i64),
    Series(i64),
}

impl Title {
    /// The name of its job and its video: `movie-3`, `series-7`.
    fn job(self) -> String {
        match self {
            Title::Movie(id) => format!("movie-{id}"),
            Title::Series(id) => format!("series-{id}"),
        }
    }

    fn from_job(job: &str) -> Option<Title> {
        let (kind, id) = job.split_once('-')?;
        let id = id.parse().ok()?;
        match kind {
            "movie" => Some(Title::Movie(id)),
            "series" => Some(Title::Series(id)),
            _ => None,
        }
    }

    fn ids(self) -> (Option<i64>, Option<i64>) {
        match self {
            Title::Movie(id) => (Some(id), None),
            Title::Series(id) => (None, Some(id)),
        }
    }
}

/// Works for as long as the server runs: reads answers, tidies, and queues the next title.
pub async fn work(pool: SqlitePool, folders: Folders, transcoder: Arc<Transcoder>) {
    for dir in [
        folders.jobs.join("queue"),
        folders.jobs.join("done"),
        folders.videos.clone(),
    ] {
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %err, "can't use the trailer folders; no trailers are downloaded");
            return;
        }
    }
    loop {
        let on = crate::settings::trailer_downloads(&pool)
            .await
            .unwrap_or(false);
        let quality = crate::settings::trailer_quality(&pool)
            .await
            .unwrap_or(2160);
        if let Err(err) = step(&pool, &folders, quality, || !on || transcoder.busy()).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "downloading trailers stopped for now"
            );
        }
        if let Ok(c) = coverage(&pool).await {
            let done = (c.done + c.unavailable + c.failed).min(c.titles);
            let counts = (done as usize, c.titles as usize, c.failed as usize);
            crate::tasks::backlog("trailers", "downloading trailers", on, counts, || {
                format!(
                    "{} downloaded, {} not on YouTube, {} failed",
                    c.done, c.unavailable, c.failed
                )
            });
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// A present title with no trailer in its folder, its trailers from TMDB, and its record here.
#[derive(sqlx::FromRow)]
struct Candidate {
    movie_id: Option<i64>,
    series_id: Option<i64>,
    videos: String,
    original_language: Option<String>,
    video: Option<String>,
    max_height: Option<i64>,
    state: Option<String>,
    attempts: Option<i64>,
    finished_at: Option<i64>,
}

impl Candidate {
    fn title(&self) -> Option<Title> {
        match (self.movie_id, self.series_id) {
            (Some(id), None) => Some(Title::Movie(id)),
            (None, Some(id)) => Some(Title::Series(id)),
            _ => None,
        }
    }

    /// The trailer this title should have now, at most `quality` high, when it's due another
    /// try: never asked for, a different trailer or quality wanted, or failed and its wait over.
    fn due(&self, language: &str, quality: i64, now: i64) -> Option<String> {
        let trailers: Vec<Trailer> = serde_json::from_str(&self.videos).ok()?;
        let wanted = chosen(
            &trailers,
            &languages(self.original_language.as_deref(), language),
        )?;
        let same =
            self.video.as_deref() == Some(wanted.key.as_str()) && self.max_height == Some(quality);
        let due = match self.state.as_deref() {
            None => true,
            Some(_) if !same => true,
            Some("failed") => {
                self.attempts.unwrap_or(0) < TRIES
                    && self.finished_at.unwrap_or(0) <= now - RETRY_AFTER
            }
            _ => false,
        };
        due.then(|| wanted.key.clone())
    }
}

/// One round: answers recorded, the trailers of titles that left (or got their own) removed,
/// and, when nothing's waiting and nothing says to hold (downloads off, video being transcoded,
/// music being split into parts), the next title queued at most `quality` lines high. Returns
/// the title queued.
pub async fn step(
    pool: &SqlitePool,
    folders: &Folders,
    quality: i64,
    hold: impl Fn() -> bool,
) -> anyhow::Result<Option<Title>> {
    read_answers(pool, folders).await?;
    tidy(pool, &folders.videos).await?;
    let (waiting, separating): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM downloaded_trailers WHERE state = 'queued'),
                EXISTS (SELECT 1 FROM music_stems WHERE state = 'queued')",
    )
    .fetch_one(pool)
    .await?;
    if waiting || separating || hold() {
        return Ok(None);
    }
    let language = crate::settings::value(pool, "MELETE_METADATA_LANGUAGE")
        .await?
        .unwrap_or_else(|| "en-US".into());
    let now = unix_now();
    // Newest first: movies and series by when their latest file came into the library.
    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT x.id AS movie_id, NULL AS series_id, t.videos, t.original_language,
             d.video, d.max_height, d.state, d.attempts, d.finished_at,
             (SELECT max(coalesce(f.added_at, f.modified_at)) FROM media_files f
                 WHERE f.movie_id = x.id) AS added
         FROM movies x JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
         LEFT JOIN downloaded_trailers d ON d.movie_id = x.id
         WHERE x.missing_since IS NULL AND t.videos IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM media_extras e WHERE e.movie_id = x.id
               AND e.kind = 'trailer' AND e.missing_since IS NULL)
         UNION ALL
         SELECT NULL, x.id, t.videos, t.original_language,
             d.video, d.max_height, d.state, d.attempts, d.finished_at,
             (SELECT max(coalesce(f.added_at, f.modified_at)) FROM media_files f
                 WHERE f.series_id = x.id)
         FROM series x JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
         LEFT JOIN downloaded_trailers d ON d.series_id = x.id
         WHERE x.missing_since IS NULL AND t.videos IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM media_extras e WHERE e.series_id = x.id
               AND e.kind = 'trailer' AND e.missing_since IS NULL)
         ORDER BY added DESC NULLS LAST",
    )
    .fetch_all(pool)
    .await?;
    let next = candidates.iter().find_map(|c| {
        let video = c.due(&language, quality, now)?;
        Some((c.title()?, video))
    });
    let Some((title, video)) = next else {
        return Ok(None);
    };
    let job = title.job();
    let queue = folders.jobs.join("queue");
    let partial = queue.join(format!(".{job}.json.tmp"));
    std::fs::write(
        &partial,
        serde_json::to_vec(&json!({ "id": job, "video": video, "max_height": quality }))?,
    )?;
    let (movie_id, series_id) = title.ids();
    sqlx::query(
        "INSERT INTO downloaded_trailers (movie_id, series_id, video, max_height, state, queued_at)
         VALUES (?1, ?2, ?3, ?4, 'queued', ?5)
         ON CONFLICT DO UPDATE SET state = 'queued', reason = NULL, finished_at = NULL,
             queued_at = excluded.queued_at,
             attempts = CASE WHEN video = excluded.video AND max_height = excluded.max_height
                 THEN attempts ELSE 0 END,
             video = excluded.video, max_height = excluded.max_height",
    )
    .bind(movie_id)
    .bind(series_id)
    .bind(&video)
    .bind(quality)
    .bind(now)
    .execute(pool)
    .await?;
    // Only now is it the downloader's to take.
    std::fs::rename(&partial, queue.join(format!("{job}.json")))?;
    Ok(Some(title))
}

/// Removes the trailers downloaded for titles that left the library or got a trailer in their
/// folder.
async fn tidy(pool: &SqlitePool, videos: &Path) -> anyhow::Result<()> {
    let gone: Vec<(i64, Option<String>)> = sqlx::query_as(
        "SELECT d.id, d.file FROM downloaded_trailers d
         LEFT JOIN movies m ON m.id = d.movie_id
         LEFT JOIN series s ON s.id = d.series_id
         WHERE d.state <> 'queued' AND (
             COALESCE(m.missing_since, s.missing_since) IS NOT NULL
             OR EXISTS (SELECT 1 FROM media_extras e WHERE e.kind = 'trailer'
                 AND e.missing_since IS NULL
                 AND (e.movie_id = d.movie_id OR e.series_id = d.series_id)))",
    )
    .fetch_all(pool)
    .await?;
    for (id, file) in gone {
        if let Some(file) = file {
            let path = video_path(videos, &file);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }
        sqlx::query("DELETE FROM downloaded_trailers WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// A video named in the database, in the videos folder and nowhere else.
pub fn video_path(videos: &Path, file: &str) -> PathBuf {
    let name = Path::new(file)
        .file_name()
        .map_or_else(|| "none".into(), |n| n.to_owned());
    videos.join(name)
}

/// How far downloading trailers has got.
#[derive(Debug, Default, PartialEq)]
pub struct Coverage {
    /// Present titles with a trailer chosen from TMDB's list and none in their folder.
    pub titles: i64,
    pub done: i64,
    pub queued: i64,
    /// Titles whose trailer YouTube hasn't (removed, private, blocked where the VPN is).
    pub unavailable: i64,
    pub failed: i64,
}

/// How far downloading trailers has got, counted from the database.
pub async fn coverage(pool: &SqlitePool) -> sqlx::Result<Coverage> {
    let (titles, done, queued, unavailable, failed): (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM movies x JOIN tmdb_movies t ON t.tmdb_id = x.match_tmdb_id
                     WHERE x.missing_since IS NULL AND json_array_length(t.videos) > 0
                       AND NOT EXISTS (SELECT 1 FROM media_extras e WHERE e.movie_id = x.id
                           AND e.kind = 'trailer' AND e.missing_since IS NULL))
              + (SELECT count(*) FROM series x JOIN tmdb_series t ON t.tmdb_id = x.match_tmdb_id
                     WHERE x.missing_since IS NULL AND json_array_length(t.videos) > 0
                       AND NOT EXISTS (SELECT 1 FROM media_extras e WHERE e.series_id = x.id
                           AND e.kind = 'trailer' AND e.missing_since IS NULL)),
                (SELECT count(*) FROM downloaded_trailers WHERE state = 'done'),
                (SELECT count(*) FROM downloaded_trailers WHERE state = 'queued'),
                (SELECT count(*) FROM downloaded_trailers WHERE state = 'unavailable'),
                (SELECT count(*) FROM downloaded_trailers WHERE state = 'failed')",
    )
    .fetch_one(pool)
    .await?;
    Ok(Coverage {
        titles,
        done,
        queued,
        unavailable,
        failed,
    })
}

#[cfg(test)]
pub(crate) mod tests;
