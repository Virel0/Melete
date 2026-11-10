//! Trailers' sound levelled once, so every trailer plays at about the same loudness whatever
//! its age or studio: measured with ffmpeg's `loudnorm`, then the whole trailer moved by one
//! steady amount (its linear second pass) to [`TARGET_LUFS`], peaks under [`TRUE_PEAK`], the
//! picture copied as it is and the sound written again as AAC. One already within
//! [`CLOSE_ENOUGH`] of it, or with no sound, is left as it is. A trailer downloaded again is
//! levelled again; one that can't be is kept as it was, with why.

use std::path::Path;
use std::process::Stdio;

use super::*;

/// The loudness every trailer is brought to: about where films are mixed, so a trailer before
/// one isn't louder than it.
pub const TARGET_LUFS: f64 = -24.0;
/// The loudest a peak may be, in dBTP.
pub const TRUE_PEAK: f64 = -2.0;
/// The loudness range kept, in LU.
const RANGE: f64 = 11.0;
/// A trailer this close to the target, in LU, is left as it is.
pub const CLOSE_ENOUGH: f64 = 1.0;
/// Trailers levelled at most in one round.
pub const PER_ROUND: usize = 4;

/// What `loudnorm`'s first pass measured, as its second pass takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    pub i: f64,
    pub tp: f64,
    pub lra: f64,
    pub thresh: f64,
    pub offset: f64,
}

/// The last `loudnorm` block ffmpeg printed, with every value the second pass needs.
pub fn parse(log: &str) -> Option<Measured> {
    let number = crate::sound::loudness::block(log)?;
    Some(Measured {
        i: number("input_i")?,
        tp: number("input_tp")?,
        lra: number("input_lra")?,
        thresh: number("input_thresh")?,
        offset: number("target_offset")?,
    })
}

fn filter() -> String {
    format!("loudnorm=I={TARGET_LUFS}:TP={TRUE_PEAK}:LRA={RANGE}")
}

/// What became of one trailer.
#[derive(Debug, Clone, PartialEq)]
pub enum Levelled {
    /// Written again at the target.
    Done,
    /// Left as it is, and why: close enough, silent, or no sound at all.
    Left(String),
}

/// Levels the trailer file at `path` in place: measured, and when it's off the target, written
/// again beside it and put in its place. Fails, leaving it as it was, when ffmpeg does.
pub async fn level(ffmpeg: &Path, path: &Path) -> Result<Levelled, String> {
    let measured = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-i"])
        .arg(path)
        .args(["-map", "0:a:0?", "-vn", "-af"])
        .arg(format!("{}:print_format=json", filter()))
        .args(["-f", "null", "-"])
        .stdout(Stdio::null())
        .output()
        .await
        .map_err(|err| format!("starting ffmpeg: {err}"))?;
    let log = String::from_utf8_lossy(&measured.stderr);
    // No sound to map: nothing to level.
    if log.contains("does not contain any stream") {
        return Ok(Levelled::Left("it has no sound".into()));
    }
    if !measured.status.success() {
        return Err(format!("measuring it: {}", last_line(&log)));
    }
    let Some(m) = parse(&log) else {
        return Ok(Levelled::Left("it has no sound".into()));
    };
    if m.i <= -70.0 {
        return Ok(Levelled::Left("it's silent".into()));
    }
    if (m.i - TARGET_LUFS).abs() <= CLOSE_ENOUGH && m.tp <= TRUE_PEAK {
        return Ok(Levelled::Left(format!("already at {:.1} LUFS", m.i)));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("the trailer's file has no name")?;
    let partial = path.with_file_name(format!(".{name}.levelling.mp4"));
    let second = format!(
        "{}:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true,aresample=48000",
        filter(),
        m.i,
        m.tp,
        m.lra,
        m.thresh,
        m.offset
    );
    let written = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-map", "0:a:0", "-c:v", "copy", "-af"])
        .arg(second)
        .args([
            "-c:a",
            "aac",
            "-b:a",
            "256k",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(&partial)
        .stdout(Stdio::null())
        .output()
        .await
        .map_err(|err| format!("starting ffmpeg: {err}"))?;
    if !written.status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "writing it: {}",
            last_line(&String::from_utf8_lossy(&written.stderr))
        ));
    }
    // A player that has it open keeps reading the old one until it's done.
    std::fs::rename(&partial, path).map_err(|err| {
        let _ = std::fs::remove_file(&partial);
        format!("putting it in place: {err}")
    })?;
    Ok(Levelled::Done)
}

fn last_line(log: &str) -> String {
    log.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("ffmpeg said nothing")
        .chars()
        .take(300)
        .collect()
}

/// Levels up to [`PER_ROUND`] downloaded trailers not levelled yet, newest first, recording
/// each; one ffmpeg can't level is kept as it was, with why, and not tried again. How many
/// were looked at.
pub async fn level_some(
    pool: &SqlitePool,
    folders: &Folders,
    ffmpeg: &Path,
) -> anyhow::Result<usize> {
    let due: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, file FROM downloaded_trailers
         WHERE state = 'done' AND file IS NOT NULL AND levelled_at IS NULL
         ORDER BY finished_at DESC, id DESC LIMIT ?",
    )
    .bind(PER_ROUND as i64)
    .fetch_all(pool)
    .await?;
    for (id, file) in &due {
        let Some(name) = plain_name(file) else {
            continue;
        };
        let path = folders.videos.join(name);
        let note = match level(ffmpeg, &path).await {
            Ok(Levelled::Done) => format!("levelled to {TARGET_LUFS} LUFS"),
            Ok(Levelled::Left(why)) => why,
            Err(why) => {
                tracing::warn!(trailer = id, why, "couldn't level a trailer's sound");
                format!("kept as it was: {why}")
            }
        };
        let (size, modified) = answers::stamp(&path);
        // Only the file it looked at: one downloaded again meanwhile is levelled afresh.
        sqlx::query(
            "UPDATE downloaded_trailers SET levelled_at = ?, level_note = ?, size = ?,
                 modified_at = ?
             WHERE id = ? AND file = ? AND levelled_at IS NULL",
        )
        .bind(unix_now())
        .bind(note)
        .bind(size)
        .bind(modified)
        .bind(id)
        .bind(file)
        .execute(pool)
        .await?;
    }
    Ok(due.len())
}

#[cfg(test)]
mod tests;
