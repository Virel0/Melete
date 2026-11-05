//! Compressing a title, started from the Make room page: each of its H.264 (or older) video files
//! converted to HEVC, on the GPU when it works (else the CPU), its audio, subtitles and fonts
//! copied and HDR kept, into the transcode folder first. The copy takes the file's place only if
//! it plays as long, has the same audio and subtitle tracks, and is at least a tenth smaller; the
//! file goes to the trash ([`crate::upgrades::replace_with_compressed`]). One file at a time, and
//! only while nothing is being converted for anyone. Dolby Vision isn't compressed: the encoder
//! can't keep it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::probe::{self, MediaInfo};
use crate::transcode::{Hardware, Transcoder};

/// How often the queue looks for work.
const EVERY: Duration = Duration::from_secs(30);
/// The codecs worth compressing to HEVC.
pub const COMPRESSIBLE: &[&str] = &["h264", "mpeg2video", "vc1", "mpeg4"];
/// The least share of the file a copy must save to take its place.
const MUST_SAVE: f64 = 0.1;

/// Why compressing wasn't started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotStarted {
    NotFound,
    /// No file that compresses: none H.264 or older, or Dolby Vision, or all queued already.
    NothingToCompress,
}

/// Queues a movie's (or with `series`, a series') files that compress. How many.
pub async fn start(
    pool: &SqlitePool,
    series: bool,
    id: i64,
) -> anyhow::Result<Result<usize, NotStarted>> {
    let (table, column) = if series {
        ("series", "series_id")
    } else {
        ("movies", "movie_id")
    };
    let present: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT id FROM {table} WHERE id = ? AND missing_since IS NULL"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if present.is_none() {
        return Ok(Err(NotStarted::NotFound));
    }
    let codecs = COMPRESSIBLE
        .iter()
        .map(|c| format!("'{c}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let files: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT f.id FROM media_files f JOIN media_probes p ON p.file_id = f.id
         WHERE f.{column} = ? AND f.missing_since IS NULL
           AND json_extract(p.info, '$.video[0].codec') IN ({codecs})
           AND json_extract(p.info, '$.video[0].hdr') IS NOT 'Dolby Vision'
           AND NOT EXISTS (SELECT 1 FROM compressions c WHERE c.file_id = f.id
               AND c.state IN ('waiting', 'running'))
         ORDER BY f.season, f.episode, f.id"
    )))
    .bind(id)
    .fetch_all(pool)
    .await?;
    if files.is_empty() {
        return Ok(Err(NotStarted::NothingToCompress));
    }
    let now = unix_now();
    let mut tx = pool.begin().await?;
    for file in &files {
        sqlx::query(
            "INSERT INTO compressions (file_id, state, started_at) VALUES (?, 'waiting', ?)
             ON CONFLICT (file_id) DO UPDATE SET state = 'waiting', reason = NULL, saved = NULL,
                 started_at = excluded.started_at, finished_at = NULL",
        )
        .bind(file)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(files.len()))
}

/// Works through compressions for as long as the server runs.
pub async fn run(pool: SqlitePool, transcoder: Arc<Transcoder>, ffprobe: PathBuf) {
    // One cut off by a restart starts again.
    let _ = sqlx::query("UPDATE compressions SET state = 'waiting' WHERE state = 'running'")
        .execute(&pool)
        .await;
    loop {
        if !transcoder.busy()
            && let Err(err) = step(&pool, &transcoder, &ffprobe).await
        {
            tracing::warn!(error = format!("{err:#}"), "compressing stopped for now");
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Compresses the next waiting file, if any.
pub async fn step(
    pool: &SqlitePool,
    transcoder: &Transcoder,
    ffprobe: &Path,
) -> anyhow::Result<()> {
    let next: Option<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT c.id, f.id, l.path, f.rel_path FROM compressions c
         JOIN media_files f ON f.id = c.file_id JOIN libraries l ON l.id = f.library_id
         WHERE c.state = 'waiting' ORDER BY c.id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((id, file_id, root, rel_path)) = next else {
        return Ok(());
    };
    sqlx::query("UPDATE compressions SET state = 'running' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let dir = transcoder.root().join("compress");
    tokio::fs::create_dir_all(&dir).await?;
    let out = dir.join(format!("{file_id}.mkv"));
    let source = Path::new(&root).join(&rel_path);
    let tools = Tools {
        ffmpeg: transcoder.ffmpeg(),
        ffprobe,
        hw: transcoder.hardware(),
    };
    let outcome = match compress(&tools, &source, &out).await {
        Ok(()) => crate::upgrades::replace_with_compressed(pool, file_id, &out).await?,
        Err(why) => Err(why),
    };
    let _ = tokio::fs::remove_file(&out).await;
    match outcome {
        Ok(saved) => {
            sqlx::query(
                "UPDATE compressions SET state = 'done', saved = ?, finished_at = ? WHERE id = ?",
            )
            .bind(saved)
            .bind(unix_now())
            .bind(id)
            .execute(pool)
            .await?;
        }
        Err(why) => {
            tracing::warn!(rel_path, why, "a file couldn't be compressed");
            sqlx::query(
                "UPDATE compressions SET state = 'failed', reason = ?, finished_at = ? WHERE id = ?",
            )
            .bind(why.chars().take(300).collect::<String>())
            .bind(unix_now())
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// What compressing runs: ffmpeg, ffprobe, and where to encode.
pub(crate) struct Tools<'a> {
    pub ffmpeg: &'a Path,
    pub ffprobe: &'a Path,
    pub hw: Hardware,
}

/// Converts `source` to an HEVC copy at `out` and checks it may take the file's place: the same
/// length, audio and subtitle tracks, and at least a tenth smaller. Why not, when it can't.
pub(crate) async fn compress(tools: &Tools<'_>, source: &Path, out: &Path) -> Result<(), String> {
    let (ffprobe, hw) = (tools.ffprobe, tools.hw);
    let before = probe::probe(ffprobe, source)
        .await
        .map_err(|err| format!("reading the file: {err}"))?;
    let video = before.video.first().ok_or("it has no video")?;
    if video.hdr.as_deref() == Some("Dolby Vision") {
        return Err("it's Dolby Vision, which the encoder can't keep".into());
    }
    let ten_bit = video
        .pixel_format
        .as_deref()
        .is_some_and(|p| p.contains("10"));
    let args = encode_args(hw, source, out, video.index, ten_bit);
    let done = tokio::process::Command::new(tools.ffmpeg)
        .args(&args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|err| format!("running ffmpeg: {err}"))?;
    if !done.status.success() {
        let said = String::from_utf8_lossy(&done.stderr);
        return Err(format!(
            "ffmpeg failed: {}",
            said.trim().chars().take(200).collect::<String>()
        ));
    }
    let after = probe::probe(ffprobe, out)
        .await
        .map_err(|err| format!("reading the copy: {err}"))?;
    let (old_size, new_size) = (
        std::fs::metadata(source).map_or(0, |m| m.len()),
        std::fs::metadata(out).map_or(0, |m| m.len()),
    );
    check(&before, &after, old_size, new_size)
}

/// ffmpeg's arguments: the main video converted to HEVC (10-bit when it is), everything else
/// copied (audio, subtitles, fonts), as Matroska.
fn encode_args(hw: Hardware, source: &Path, out: &Path, video: i64, ten_bit: bool) -> Vec<String> {
    let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    args.push(source.to_string_lossy().into_owned());
    for map in [
        format!("0:{video}"),
        "0:a?".into(),
        "0:s?".into(),
        "0:t?".into(),
    ] {
        args.extend(["-map".into(), map]);
    }
    args.extend(["-c".into(), "copy".into()]);
    let encoder: &[&str] = match hw {
        Hardware::Nvenc => &[
            "-c:v",
            "hevc_nvenc",
            "-preset",
            "p6",
            "-tune",
            "hq",
            "-rc",
            "vbr",
            "-cq",
            "22",
            "-b:v",
            "0",
            "-spatial_aq",
            "1",
        ],
        Hardware::Software => &["-c:v", "libx265", "-preset", "medium", "-crf", "20"],
    };
    args.extend(encoder.iter().map(|s| (*s).to_owned()));
    if ten_bit {
        let format = if hw == Hardware::Nvenc {
            "p010le"
        } else {
            "yuv420p10le"
        };
        args.extend([
            "-pix_fmt".into(),
            format.into(),
            "-profile:v".into(),
            "main10".into(),
        ]);
    }
    args.extend([
        "-f".into(),
        "matroska".into(),
        out.to_string_lossy().into_owned(),
    ]);
    args
}

/// Whether a copy may take the file's place: as long (within half a percent, or two seconds),
/// the same audio and subtitle tracks, and at least [`MUST_SAVE`] smaller.
fn check(
    before: &MediaInfo,
    after: &MediaInfo,
    old_size: u64,
    new_size: u64,
) -> Result<(), String> {
    let (Some(was), Some(is)) = (before.duration_seconds, after.duration_seconds) else {
        return Err("ffprobe couldn't tell both lengths".into());
    };
    if (was - is).abs() > (was * 0.005).max(2.0) {
        return Err(format!("the copy plays {is:.0} s against {was:.0} s"));
    }
    if after.video.first().map(|v| v.codec.as_str()) != Some("hevc") {
        return Err("the copy isn't HEVC".into());
    }
    if before.audio.len() != after.audio.len() || before.subtitles.len() != after.subtitles.len() {
        return Err("the copy hasn't the same audio and subtitle tracks".into());
    }
    if (new_size as f64) > old_size as f64 * (1.0 - MUST_SAVE) {
        return Err(format!(
            "the copy would save only {:.0}%",
            100.0 * (1.0 - new_size as f64 / old_size.max(1) as f64)
        ));
    }
    Ok(())
}

/// How compressing one title is going, for the Make room page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, sqlx::FromRow)]
pub struct Progress {
    /// `movie` or `series`.
    pub kind: String,
    pub id: i64,
    pub title: String,
    /// In a library with the anime category, or wanted as anime.
    pub anime: bool,
    pub waiting: i64,
    pub running: i64,
    pub done: i64,
    pub failed: i64,
    /// Bytes freed so far.
    pub saved: i64,
}

/// Every title with compressions, the most recently started first.
pub async fn progress(pool: &SqlitePool) -> sqlx::Result<Vec<Progress>> {
    sqlx::query_as(
        "SELECT CASE WHEN f.movie_id IS NULL THEN 'series' ELSE 'movie' END AS kind,
             coalesce(f.movie_id, f.series_id) AS id,
             coalesce(m.parsed_title, s.parsed_title) AS title,
             (l.category IS 'anime' OR EXISTS (SELECT 1 FROM wanted_series w
                 WHERE w.series_id = f.series_id AND w.series_type = 'anime')) AS anime,
             sum(c.state = 'waiting') AS waiting, sum(c.state = 'running') AS running,
             sum(c.state = 'done') AS done, sum(c.state = 'failed') AS failed,
             coalesce(sum(c.saved), 0) AS saved
         FROM compressions c JOIN media_files f ON f.id = c.file_id
         JOIN libraries l ON l.id = f.library_id
         LEFT JOIN movies m ON m.id = f.movie_id LEFT JOIN series s ON s.id = f.series_id
         GROUP BY kind, coalesce(f.movie_id, f.series_id)
         ORDER BY max(c.started_at) DESC",
    )
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests;
