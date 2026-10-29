//! Making copies in the background, one at a time, behind live conversions.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::*;
use crate::subtitles::{self, Format, Source};
use crate::transcode::{self, Hardware, Settings, Transcoder};

/// How often the worker looks for a copy to make, or waits for live conversions to ease.
const TICK: Duration = Duration::from_secs(5);
/// How often expired copies are swept.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);
/// Progress is written down at most this often.
const PROGRESS_EVERY: Duration = Duration::from_secs(2);

/// What making a copy needs.
#[derive(Clone)]
pub struct Tools {
    /// The transcoder, for its ffmpeg, hardware and how busy it is.
    pub transcoder: Arc<Transcoder>,
    /// Where copies are kept.
    pub root: PathBuf,
    /// The subtitle cache, where text tracks are converted once.
    pub subtitle_cache: PathBuf,
}

/// Makes copies as they're asked for while the setting is on, and sweeps expired ones, for
/// as long as the server runs.
pub async fn work(pool: SqlitePool, tools: Tools) {
    let mut swept: Option<Instant> = None;
    crate::tasks::scheduled("offline-copies", TICK);
    loop {
        let began = crate::db::unix_now();
        if swept.is_none_or(|at| at.elapsed() >= SWEEP_EVERY) {
            match sweep(&pool, &tools.root).await {
                Ok(n) if n > 0 => {
                    crate::tasks::ran("offline-copies", began, format!("deleted {n} expired"));
                    tracing::info!(deleted = n, "deleted offline copies whose time was up")
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(
                    error = format!("{err:#}"),
                    "couldn't delete old offline copies"
                ),
            }
            swept = Some(Instant::now());
        }
        match make_next(&pool, &tools).await {
            Ok(Some(_)) => {
                crate::tasks::ran("offline-copies", began, "made a copy".into());
                continue;
            }
            Ok(None) => {}
            Err(err) => {
                let failed = crate::tasks::failed().unwrap_or_default();
                crate::tasks::ran("offline-copies", began, failed);
                tracing::warn!(error = format!("{err:#}"), "making an offline copy stopped")
            }
        }
        tokio::time::sleep(TICK).await;
    }
}

/// Whether a copy may start now: live conversions take no more than half the slots.
pub fn room_for_one(transcoder: &Transcoder) -> bool {
    let (running, most) = transcoder.load();
    running * 2 <= most
}

/// Makes the copy asked for longest ago, when the setting is on and there's room; its id, or
/// None when there was nothing to do now.
pub async fn make_next(pool: &SqlitePool, tools: &Tools) -> anyhow::Result<Option<i64>> {
    if !crate::settings::offline_copies(pool).await? || !room_for_one(&tools.transcoder) {
        return Ok(None);
    }
    let next: Option<(i64, i64, String, String)> = sqlx::query_as(
        "SELECT id, file_id, settings, subtitles FROM offline_copies
         WHERE status = 'queued' ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((id, file_id, settings, tracks)) = next else {
        return Ok(None);
    };
    sqlx::query("UPDATE offline_copies SET status = 'making' WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let tracks: Vec<String> = serde_json::from_str(&tracks)?;
    let made = make(
        pool,
        tools,
        id,
        file_id,
        &serde_json::from_str(&settings)?,
        &tracks,
    )
    .await;
    let failed = |err: anyhow::Error| async move {
        tracing::warn!(
            id,
            error = format!("{err:#}"),
            "an offline copy couldn't be made"
        );
        sqlx::query("UPDATE offline_copies SET status = 'failed', error = ? WHERE id = ?")
            .bind(format!("{err:#}").chars().take(300).collect::<String>())
            .bind(id)
            .execute(pool)
            .await
    };
    match made {
        Ok(bytes) => {
            sqlx::query(
                "UPDATE offline_copies SET status = 'ready', progress = 1, bytes = ?, ready_at = ?
                 WHERE id = ?",
            )
            .bind(bytes)
            .bind(unix_now())
            .bind(id)
            .execute(pool)
            .await?;
        }
        Err(err) => {
            failed(err).await?;
        }
    }
    Ok(Some(id))
}

/// The file to copy, with its probe.
async fn source(
    pool: &SqlitePool,
    file_id: i64,
) -> anyhow::Result<(PathBuf, crate::probe::MediaInfo, (i64, i64))> {
    let row: Option<(String, String, i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT l.path, f.rel_path, f.size, f.modified_at, p.info FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         LEFT JOIN media_probes p ON p.file_id = f.id
             AND p.size = f.size AND p.modified_at = f.modified_at
         WHERE f.id = ? AND f.missing_since IS NULL",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let (root, rel_path, size, modified, info) =
        row.context("the file is no longer in the library")?;
    let info = info.context("the file hasn't been read yet, or couldn't be")?;
    Ok((
        Path::new(&root).join(rel_path),
        serde_json::from_str(&info)?,
        (size, modified),
    ))
}

/// Makes one copy: its subtitles, then its video; the video's size in bytes.
async fn make(
    pool: &SqlitePool,
    tools: &Tools,
    id: i64,
    file_id: i64,
    settings: &Settings,
    tracks: &[String],
) -> anyhow::Result<i64> {
    let (input, info, (size, modified)) = source(pool, file_id).await?;
    tokio::fs::create_dir_all(&tools.root).await?;
    for track in tracks {
        let made = subtitle(tools, &input, &info, (file_id, size, modified), track).await?;
        tokio::fs::copy(&made, subtitle_path(&tools.root, id, track)).await?;
    }
    let hw = tools.transcoder.hardware();
    let plan = transcode::plan(input, &info, settings, hw).map_err(|why| anyhow::anyhow!(why))?;
    let part = tools.root.join(format!("{id}.mp4.part"));
    let ran = run(pool, tools, id, &plan, hw, &part).await;
    let ran = match ran {
        // The GPU failed: once more on the CPU, as live conversions do.
        Err(err) if hw == Hardware::Nvenc => {
            tracing::warn!(
                id,
                error = format!("{err:#}"),
                "an offline copy failed on the GPU; trying the CPU"
            );
            let mut plan = plan.clone();
            plan.gpu_decode = false;
            run(pool, tools, id, &plan, Hardware::Software, &part).await
        }
        other => other,
    };
    if let Err(err) = ran {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(err);
    }
    let path = video_path(&tools.root, id);
    tokio::fs::rename(&part, &path).await?;
    Ok(tokio::fs::metadata(&path).await?.len() as i64)
}

/// A text track as WebVTT in the subtitle cache, converted once.
async fn subtitle(
    tools: &Tools,
    video: &Path,
    info: &crate::probe::MediaInfo,
    version: (i64, i64, i64),
    track: &str,
) -> anyhow::Result<PathBuf> {
    let ffmpeg = tools.transcoder.ffmpeg();
    let (file_id, size, modified) = version;
    if let Some(index) = track.strip_prefix('s').and_then(|i| i.parse::<i64>().ok()) {
        let text = info.subtitles.iter().any(|s| s.index == index && s.text);
        anyhow::ensure!(
            text,
            "subtitle track {track} isn't a text track of the file"
        );
        let key = subtitles::embedded_key(file_id, index, size, modified);
        let source = Source::Embedded { video, index };
        return subtitles::convert(ffmpeg, &tools.subtitle_cache, &key, source, Format::WebVtt)
            .await;
    }
    let id = track
        .strip_prefix('x')
        .context("subtitle tracks are s or x ones")?;
    let video = video.to_path_buf();
    let sidecars = tokio::task::spawn_blocking(move || subtitles::sidecars(&video)).await??;
    let sidecar = sidecars
        .iter()
        .find(|s| s.id == id)
        .with_context(|| format!("subtitle track {track} isn't beside the file any more"))?;
    let key = subtitles::sidecar_key(file_id, sidecar);
    let source = Source::Sidecar(&sidecar.path);
    subtitles::convert(ffmpeg, &tools.subtitle_cache, &key, source, Format::WebVtt).await
}

/// Runs ffmpeg for a copy, noting its progress, until it ends or the copy is deleted.
async fn run(
    pool: &SqlitePool,
    tools: &Tools,
    id: i64,
    plan: &transcode::Plan,
    hw: Hardware,
    out: &Path,
) -> anyhow::Result<()> {
    let args = transcode::whole_file_args(plan, hw, tools.transcoder.abilities(), out);
    let mut child = Command::new(tools.transcoder.ffmpeg())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("starting ffmpeg")?;
    let mut lines = BufReader::new(child.stdout.take().context("ffmpeg's output")?).lines();
    let mut noted = Instant::now();
    while let Some(line) = lines.next_line().await? {
        let Some(progress) = transcode::progress_of(&line, plan.duration) else {
            continue;
        };
        if noted.elapsed() < PROGRESS_EVERY {
            continue;
        }
        noted = Instant::now();
        let still = sqlx::query("UPDATE offline_copies SET progress = ? WHERE id = ?")
            .bind(progress)
            .bind(id)
            .execute(pool)
            .await?;
        if still.rows_affected() == 0 {
            // Deleted while being made.
            let _ = child.kill().await;
            anyhow::bail!("the copy was deleted while it was being made");
        }
    }
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        let log = String::from_utf8_lossy(&output.stderr);
        let last = log
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("ffmpeg failed");
        anyhow::bail!(
            "ffmpeg couldn't convert the file: {}",
            last.chars().take(200).collect::<String>()
        );
    }
    Ok(())
}
