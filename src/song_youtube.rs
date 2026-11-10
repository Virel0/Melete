//! Songs from YouTube as a last resort (`MELETE_SONG_YOUTUBE`, off by default), fetched by the
//! trailer downloader beside Melete, behind the VPN. A wanted song is tried once Soulseek has
//! been searched for it and nothing is on its way or in the library: nothing fitted, or every
//! Soulseek download of it failed (a peer refused it, say, as many times as the next source
//! goes), [`SETTLED`] after the last of those. Songs wanted before this was turned on are tried
//! the same way, the newest first; one at a time.
//!
//! Two jobs go to the downloader through its job folder, as trailers' do: a search of YouTube
//! for the song's artist and title (`song-<id>-search`), whose results Melete chooses from
//! ([`choosing`]: title, artist and length must all fit, and nothing that makes it another
//! version), then the chosen video's audio as it comes (`song-<id>-audio`; Opus or AAC, lossy,
//! never converted). The file is checked to be as long as the song again, put beside the song's
//! album, or in `Singles`, as a Soulseek song is, without replacing anything, and tagged as one.
//! No video fitting is believed for [`NONE_FOR`]; a failure is tried again a day later, three
//! times at most. What leaves the server: the song's artist and title (to YouTube's search) and
//! the chosen video's id, only through the VPN the downloader runs behind.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use sqlx::SqlitePool;

use crate::album_fix::Fixes;
use crate::db::unix_now;
use crate::progress::Step;

mod answers;
pub mod choosing;
mod placing;

use answers::*;
use choosing::{Found, Song};
use placing::*;

/// How often answers are read and the next song queued.
const EVERY: Duration = Duration::from_secs(30);
/// Seconds after a song's last Soulseek search or download before YouTube is tried: long
/// enough for a next source to be grabbed, or its album fetched for it.
pub const SETTLED: i64 = 10 * 60;
/// How long no video fitting is believed.
pub const NONE_FOR: i64 = 30 * 86_400;
/// How long after a failure it's tried again, and how many tries it gets.
const FAIL_AGAIN: i64 = 86_400;
const TRIES: i64 = 3;
/// How long a job may sit with the downloader before it's taken to be lost.
const LOST_AFTER: i64 = 86_400;

/// The downloader's answer to one of a song's jobs.
#[derive(Debug, Deserialize)]
struct Answer {
    ok: bool,
    #[serde(default)]
    results: Vec<Found>,
    file: Option<String>,
    duration: Option<f64>,
    unavailable: Option<String>,
    error: Option<String>,
}

/// One of a song's jobs, as named by its file.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Job {
    Search(i64),
    Audio(i64),
}

impl Job {
    fn name(self) -> String {
        match self {
            Job::Search(song) => format!("song-{song}-search"),
            Job::Audio(song) => format!("song-{song}-audio"),
        }
    }

    fn from_name(name: &str) -> Option<Job> {
        let rest = name.strip_prefix("song-")?;
        let (song, kind) = rest.split_once('-')?;
        let song = song.parse().ok()?;
        match kind {
            "search" => Some(Job::Search(song)),
            "audio" => Some(Job::Audio(song)),
            _ => None,
        }
    }
}

/// Whether a job file in the downloader's folder is a song's, which trailers leave alone.
pub fn is_song_job(name: &str) -> bool {
    name.starts_with("song-")
}

#[derive(sqlx::FromRow)]
struct Wanted {
    id: i64,
    title: String,
    artist: String,
    length_seconds: Option<i64>,
}

impl Wanted {
    fn song(&self) -> Song<'_> {
        Song {
            title: &self.title,
            artist: &self.artist,
            length_seconds: self.length_seconds,
        }
    }
}

/// Works for as long as the server runs: reads the downloader's answers to songs' jobs, and
/// with the setting on and none under way, queues the next song. `jobs` is the downloader's job
/// folder.
pub async fn work(pool: SqlitePool, jobs: PathBuf, fixes: Arc<Fixes>) {
    for dir in [jobs.join("queue"), jobs.join("done")] {
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(error = %err, "can't use the downloader's folders; no songs come from YouTube");
            return;
        }
    }
    loop {
        if let Err(err) = step(&pool, &jobs, &fixes, unix_now()).await {
            tracing::warn!(
                error = format!("{err:#}"),
                "fetching songs from YouTube stopped for now"
            );
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// One round: answers read, a lost job given up on, and with the setting on and nothing under
/// way, the next song's search queued.
pub async fn step(pool: &SqlitePool, jobs: &Path, fixes: &Fixes, now: i64) -> anyhow::Result<()> {
    read_answers(pool, jobs, fixes, now).await?;
    sqlx::query(
        "UPDATE song_youtube SET state = 'failed', attempts = attempts + 1, updated_at = ?1,
             message = 'the downloader didn''t answer; is it running?'
         WHERE state IN ('searching', 'downloading') AND updated_at <= ?2",
    )
    .bind(now)
    .bind(now - LOST_AFTER)
    .execute(pool)
    .await?;
    if !crate::settings::song_youtube(pool).await? {
        return Ok(());
    }
    let busy: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM song_youtube WHERE state IN ('searching', 'downloading'))",
    )
    .fetch_one(pool)
    .await?;
    if busy {
        return Ok(());
    }
    if let Some(song) = next(pool, now).await? {
        queue_search(pool, jobs, &song, now).await?;
    }
    Ok(())
}

/// The next song to try: Soulseek searched for it, no download of it but failed ones, nothing
/// for it or the album fetched for it for [`SETTLED`], not tried on YouTube or due again, and nothing on its way or in the library
/// (an album fetched for it, say); the newest wanted first.
async fn next(pool: &SqlitePool, now: i64) -> anyhow::Result<Option<Wanted>> {
    let candidates: Vec<Wanted> = sqlx::query_as(
        "SELECT s.id, s.title, s.artist, s.length_seconds FROM wanted_songs s
         LEFT JOIN song_youtube y ON y.wanted_song_id = s.id
         WHERE EXISTS (SELECT 1 FROM searches q WHERE q.wanted_song_id = s.id)
           AND NOT EXISTS (SELECT 1 FROM downloads d
                           WHERE d.wanted_song_id = s.id AND d.state <> 'failed')
           AND (SELECT max(at) FROM (
                    SELECT searched_at AS at FROM searches WHERE wanted_song_id = s.id
                    UNION ALL
                    SELECT updated_at FROM downloads
                    WHERE wanted_song_id = s.id
                       OR (wanted_album_id IS NOT NULL
                           AND wanted_album_id = (SELECT wanted_album_id FROM wanted_songs
                                                  WHERE id = s.id)))) <= ?1
           AND (y.id IS NULL
                OR (y.state = 'failed' AND y.attempts < ?2 AND y.updated_at <= ?3)
                OR (y.state = 'none' AND y.updated_at <= ?4))
         ORDER BY s.added_at DESC, s.id DESC LIMIT 25",
    )
    .bind(now - SETTLED)
    .bind(TRIES)
    .bind(now - FAIL_AGAIN)
    .bind(now - NONE_FOR)
    .fetch_all(pool)
    .await?;
    for song in candidates {
        let step = crate::progress::of(pool, "song", song.id)
            .await?
            .map(|p| p.step);
        if matches!(step, Some(Step::Wanted | Step::Failed)) {
            return Ok(Some(song));
        }
    }
    Ok(None)
}

/// Writes a job for the downloader: under a hidden name first, then renamed, so it never
/// takes half a file.
fn write_job(jobs: &Path, job: Job, body: &serde_json::Value) -> anyhow::Result<()> {
    let queue = jobs.join("queue");
    let name = job.name();
    let partial = queue.join(format!(".{name}.json.tmp"));
    std::fs::write(&partial, serde_json::to_vec(body)?)?;
    std::fs::rename(&partial, queue.join(format!("{name}.json")))?;
    Ok(())
}

async fn queue_search(
    pool: &SqlitePool,
    jobs: &Path,
    song: &Wanted,
    now: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO song_youtube (wanted_song_id, state, started_at, updated_at)
         VALUES (?1, 'searching', ?2, ?2)
         ON CONFLICT (wanted_song_id) DO UPDATE SET state = 'searching', video = NULL,
             video_title = NULL, channel = NULL, duration_seconds = NULL, message = NULL,
             attempts = CASE WHEN song_youtube.state = 'failed' THEN song_youtube.attempts
                 ELSE 0 END,
             started_at = ?2, updated_at = ?2",
    )
    .bind(song.id)
    .bind(now)
    .execute(pool)
    .await?;
    let job = Job::Search(song.id);
    let query = format!("{} {}", song.artist, song.title);
    write_job(
        jobs,
        job,
        &json!({ "id": job.name(), "kind": "song_search", "query": query }),
    )
}

#[cfg(test)]
mod tests;
