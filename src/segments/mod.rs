//! Intros and credits in episodes, and credits in movies, so apps can offer to
//! skip them.
//!
//! An intro is audio an episode shares with other episodes of its season near its start, and
//! credits are audio it shares with them near its end (an ending song). Each episode's first
//! quarter (10 minutes at most) and last quarter (6 at most) are decoded through ffmpeg at 8 kHz
//! and fingerprinted ([`fingerprint`]), and compared with up to three other episodes of the
//! season, nearest first; the longest shared stretch of a plausible length is kept: 15 seconds
//! to 2½ minutes for an intro, 15 seconds to 5 minutes for credits. Credits with music of their
//! own each week, or none, aren't found that way; [`ending`] looks for those, and for movies'
//! credits, in the file alone.
//!
//! One file at a time, in the background after each scan and probe, each version of a file
//! once, while it's turned on (`MELETE_INTRO_DETECTION`, off by default, as the first run
//! decodes part of every episode). An episode alone in its season waits for another. Turned
//! off, nothing new is looked for, and what was found is still served.
//!
//! Each intro and credits found by matching is told apart as one of the series' openings or
//! endings ([`themes`]), so a player can leave alone, the first time, one a user hasn't heard.

pub mod ending;
pub mod fingerprint;
mod served;
pub mod themes;

#[cfg(test)]
pub use served::of_file;
pub use served::{Found, sourced};

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use sqlx::SqlitePool;

use crate::db::unix_now;
use crate::jobs::pace::Pace;
use fingerprint::{frames, seconds, shared};

/// Episodes shorter than this aren't looked at.
const SHORTEST: f64 = 300.0;
const INTRO_SHARE: f64 = 0.25;
const INTRO_MOST: f64 = 600.0;
const CREDITS_SHARE: f64 = 0.25;
const CREDITS_MOST: f64 = 360.0;
/// Shortest and longest plausible intros and credits, in seconds.
const INTRO_LENGTH: (f64, f64) = (15.0, 150.0);
const CREDITS_LENGTH: (f64, f64) = (15.0, 300.0);
/// Other episodes each one is compared with.
const PARTNERS: usize = 3;
const DECODE_TIMEOUT: Duration = Duration::from_secs(300);

/// What a pass of intro and credits detection did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Episodes compared with others of their season.
    pub scanned: usize,
    /// Intros found.
    pub intros: usize,
    /// Credits found.
    pub credits: usize,
    /// Episodes whose audio couldn't be decoded; looked at again only once the file changes.
    pub failed: usize,
}

/// A stretch of a file, such as its intro.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Where it starts, in seconds from the beginning of the file.
    pub start: f64,
    /// Where it ends, in seconds from the beginning of the file.
    pub end: f64,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    root: String,
    rel_path: String,
    size: i64,
    modified_at: i64,
    series_id: i64,
    season: i64,
    episode: i64,
    duration: f64,
    current: bool,
}

/// An episode's fingerprints: of its opening and, from `credits_from` seconds, of its end.
struct Prints {
    intro: Vec<u32>,
    credits: Vec<u32>,
    credits_from: f64,
    duration: f64,
}

enum Decoded {
    Prints(Prints),
    /// ffmpeg couldn't read its audio: kept as this version's answer.
    Failed(String),
    /// Ran out of time (a slow share, say): tried again next run.
    Later,
}

/// As [`run_both`], for what matching episodes found.
#[cfg(test)]
pub async fn run(pool: &SqlitePool, ffmpeg: &Path) -> anyhow::Result<Report> {
    Ok(run_both(pool, ffmpeg).await?.0)
}

/// Looks for intros and credits as [`find_pending`] does, then for credits in files alone as
/// [`ending::find_pending`] does, while that's turned on.
pub async fn run_both(pool: &SqlitePool, ffmpeg: &Path) -> anyhow::Result<(Report, ending::Found)> {
    if !crate::settings::intro_detection(pool).await? {
        return Ok((Report::default(), ending::Found::default()));
    }
    let matched = find_pending(pool, ffmpeg).await?;
    Ok((matched, ending::find_pending(pool, ffmpeg).await?))
}

/// Looks for intros and credits in every episode not yet looked at in its current version.
pub async fn find_pending(pool: &SqlitePool, ffmpeg: &Path) -> anyhow::Result<Report> {
    let version = tokio::process::Command::new(ffmpeg)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .with_context(|| format!("starting {}; set MELETE_FFMPEG", ffmpeg.display()))?;
    anyhow::ensure!(version.success(), "{} -version failed", ffmpeg.display());

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT f.id, l.path AS root, f.rel_path, f.size, f.modified_at, f.series_id, f.season,
             f.episode, json_extract(p.info, '$.duration_seconds') AS duration,
             (s.file_id IS NOT NULL AND s.size = f.size AND s.modified_at = f.modified_at)
                 AS current
         FROM media_files f
         JOIN libraries l ON l.id = f.library_id
         JOIN media_probes p ON p.file_id = f.id AND p.size = f.size
             AND p.modified_at = f.modified_at
         LEFT JOIN media_segment_scans s ON s.file_id = f.id
         WHERE f.missing_since IS NULL AND f.series_id IS NOT NULL AND f.season > 0
           AND json_extract(p.info, '$.duration_seconds') >= ?
         ORDER BY f.series_id, f.season, f.episode, f.id",
    )
    .bind(SHORTEST)
    .fetch_all(pool)
    .await?;
    let mut seasons: Vec<Vec<Row>> = Vec::new();
    for row in rows {
        match seasons.last_mut() {
            Some(season)
                if season[0].series_id == row.series_id && season[0].season == row.season =>
            {
                season.push(row);
            }
            _ => seasons.push(vec![row]),
        }
    }
    seasons.retain(|season| {
        let pending = season.iter().any(|r| !r.current);
        let mut numbers: Vec<i64> = season.iter().map(|r| r.episode).collect();
        numbers.dedup();
        pending && numbers.len() >= 2
    });
    let total = seasons.iter().flatten().filter(|r| !r.current).count();
    let pace = Pace::start("segments", "looking for intros and credits", total);
    let mut report = Report::default();
    for season in seasons {
        look_through(pool, ffmpeg, &season, &mut report, &pace).await?;
    }
    pace.finish();
    Ok(report)
}

/// Looks at a season's pending episodes, each against its nearest others, each counted in
/// `pace`.
async fn look_through(
    pool: &SqlitePool,
    ffmpeg: &Path,
    season: &[Row],
    report: &mut Report,
    pace: &Pace,
) -> anyhow::Result<()> {
    let mut prints: HashMap<i64, Decoded> = HashMap::new();
    let mut fresh: Vec<i64> = season.iter().filter(|r| r.current).map(|r| r.id).collect();
    for row in season.iter().filter(|r| !r.current) {
        let item = pace.item();
        if let Entry::Vacant(entry) = prints.entry(row.id) {
            entry.insert(decode(ffmpeg, row).await);
        }
        let own = match &prints[&row.id] {
            Decoded::Prints(_) => row.id,
            Decoded::Failed(why) => {
                tracing::warn!(
                    rel_path = row.rel_path,
                    why,
                    "couldn't read an episode's audio"
                );
                record(pool, row, Some(why), None, None).await?;
                report.failed += 1;
                item.failed();
                continue;
            }
            Decoded::Later => continue,
        };
        let mut partners: Vec<&Row> = season.iter().filter(|r| r.episode != row.episode).collect();
        partners.sort_by_key(|r| ((r.episode - row.episode).abs(), r.id));
        partners.truncate(PARTNERS);
        let (mut intro, mut credits): (Option<(Segment, i64, Segment)>, _) = (None, None);
        for partner in partners {
            if let Entry::Vacant(entry) = prints.entry(partner.id) {
                entry.insert(decode(ffmpeg, partner).await);
            }
            let (Decoded::Prints(mine), Decoded::Prints(theirs)) =
                (&prints[&own], &prints[&partner.id])
            else {
                continue;
            };
            if let Some((a, b)) = compare(&mine.intro, 0.0, &theirs.intro, 0.0, INTRO_LENGTH)
                && intro.is_none_or(|(best, _, _)| length(a) > length(best))
            {
                intro = Some((a, partner.id, b));
            }
            if let Some((a, b)) = compare(
                &mine.credits,
                mine.credits_from,
                &theirs.credits,
                theirs.credits_from,
                CREDITS_LENGTH,
            ) && credits.is_none_or(|(best, _, _)| length(a) > length(best))
            {
                let a = clamp(a, mine.duration);
                let b = clamp(b, theirs.duration);
                credits = Some((a, partner.id, b));
            }
        }
        // The start of each one's fingerprint, to tell which opening or ending it is.
        let Decoded::Prints(mine) = &prints[&own] else {
            continue;
        };
        let intro = intro.map(|(a, p, b)| (a, p, b, themes::excerpt(&mine.intro, 0.0, a)));
        let credits = credits.map(|(a, p, b)| {
            (
                a,
                p,
                b,
                themes::excerpt(&mine.credits, mine.credits_from, a),
            )
        });
        let told = record(
            pool,
            row,
            None,
            intro
                .as_ref()
                .map(|(a, _, _, print)| (*a, print.as_slice())),
            credits
                .as_ref()
                .map(|(a, _, _, print)| (*a, print.as_slice())),
        )
        .await?;
        fresh.push(row.id);
        report.scanned += 1;
        report.intros += usize::from(intro.is_some());
        report.credits += usize::from(credits.is_some());
        let shares = [
            ("intro", intro.map(|(_, p, b, _)| (p, b)), told.0),
            ("credits", credits.map(|(_, p, b, _)| (p, b)), told.1),
        ];
        share(pool, &fresh, shares).await?;
    }
    Ok(())
}

/// What an episode shares of one kind ("intro" or "credits"): the partner it was found with and
/// that partner's stretch, if any, and the opening or ending id the episode's was told apart as.
type Share<'a> = (&'a str, Option<(i64, Segment)>, Option<i64>);

/// A partner looked at before that found nothing of the kind gets what it shares, as the same
/// opening or ending.
async fn share(pool: &SqlitePool, fresh: &[i64], shares: [Share<'_>; 2]) -> sqlx::Result<()> {
    let mut conn = pool.acquire().await?;
    for (kind, found, theme) in shares {
        if let Some((partner, theirs)) = found
            && fresh.contains(&partner)
        {
            let added = sqlx::query(
                "INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds, theme_id)
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT (file_id, kind) DO NOTHING",
            )
            .bind(partner)
            .bind(kind)
            .bind(theirs.start)
            .bind(theirs.end)
            .bind(theme)
            .execute(&mut *conn)
            .await?;
            if let Some(theme) = theme
                && added.rows_affected() > 0
            {
                themes::reached_past(&mut conn, partner, theme, theirs.end).await?;
            }
        }
    }
    Ok(())
}

fn length(segment: Segment) -> f64 {
    segment.end - segment.start
}

fn clamp(segment: Segment, duration: f64) -> Segment {
    Segment {
        start: segment.start,
        end: segment.end.min(duration),
    }
}

/// The stretch two fingerprints share within `limits` seconds long, in each one's seconds
/// (their audio starting at `a_from` and `b_from`).
fn compare(
    a: &[u32],
    a_from: f64,
    b: &[u32],
    b_from: f64,
    (shortest, longest): (f64, f64),
) -> Option<(Segment, Segment)> {
    let found = shared(a, b, frames(shortest))?;
    let at = |from: f64, (start, end): (usize, usize)| Segment {
        start: round(from + seconds(start)),
        end: round(from + seconds(end)),
    };
    let (a, b) = (at(a_from, found.a), at(b_from, found.b));
    (length(a) <= longest).then_some((a, b))
}

fn round(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// Keeps what was found in this version of a file, replacing what was found before, each with
/// the start of its fingerprint to tell which opening or ending it is; returns their ids.
async fn record(
    pool: &SqlitePool,
    row: &Row,
    error: Option<&str>,
    intro: Option<(Segment, &[u32])>,
    credits: Option<(Segment, &[u32])>,
) -> anyhow::Result<(Option<i64>, Option<i64>)> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO media_segment_scans (file_id, size, modified_at, scanned_at, error)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (file_id) DO UPDATE SET size = excluded.size,
             modified_at = excluded.modified_at, scanned_at = excluded.scanned_at,
             error = excluded.error",
    )
    .bind(row.id)
    .bind(row.size)
    .bind(row.modified_at)
    .bind(unix_now())
    .bind(error)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM media_segments WHERE file_id = ?")
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    let mut told = [None, None];
    for ((kind, found), theme) in [("intro", intro), ("credits", credits)]
        .into_iter()
        .zip(&mut told)
    {
        if let Some((segment, print)) = found {
            *theme = themes::recognise(&mut tx, row.series_id, kind, print).await?;
            sqlx::query(
                "INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds, theme_id)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(row.id)
            .bind(kind)
            .bind(segment.start)
            .bind(segment.end)
            .bind(*theme)
            .execute(&mut *tx)
            .await?;
            if let Some(theme) = *theme {
                themes::reached_past(&mut tx, row.id, theme, segment.end).await?;
            }
        }
    }
    tx.commit().await?;
    Ok((told[0], told[1]))
}

/// An episode's opening and end, fingerprinted.
async fn decode(ffmpeg: &Path, row: &Row) -> Decoded {
    let path = Path::new(&row.root).join(&row.rel_path);
    let opening = (row.duration * INTRO_SHARE).min(INTRO_MOST);
    let end = (row.duration * CREDITS_SHARE).min(CREDITS_MOST);
    let credits_from = (row.duration - end).max(opening);
    let intro = match audio(ffmpeg, &path, 0.0, opening).await {
        Ok(Some(samples)) => fingerprint::fingerprint(&samples),
        Ok(None) => return Decoded::Later,
        Err(why) => return Decoded::Failed(why),
    };
    let credits = match audio(ffmpeg, &path, credits_from, row.duration - credits_from).await {
        Ok(Some(samples)) => fingerprint::fingerprint(&samples),
        Ok(None) => return Decoded::Later,
        Err(why) => return Decoded::Failed(why),
    };
    Decoded::Prints(Prints {
        intro,
        credits,
        credits_from,
        duration: row.duration,
    })
}

/// `length` seconds of a file's first audio track from `start`, mono at the fingerprints'
/// rate; `None` when it took too long, `Err` with ffmpeg's word when it couldn't be read.
async fn audio(
    ffmpeg: &Path,
    path: &PathBuf,
    start: f64,
    length: f64,
) -> Result<Option<Vec<f32>>, String> {
    let child = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(["-ss", &format!("{start:.3}"), "-t", &format!("{length:.3}")])
        .arg("-i")
        .arg(path)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-ac", "1"])
        .args(["-ar", &fingerprint::RATE.to_string()])
        .args(["-f", "s16le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("ffmpeg didn't start: {err}"))?;
    let Ok(out) = tokio::time::timeout(DECODE_TIMEOUT, child.wait_with_output()).await else {
        return Ok(None);
    };
    let out = out.map_err(|err| format!("ffmpeg failed: {err}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(200)
            .collect());
    }
    Ok(Some(
        out.stdout
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
            .collect(),
    ))
}

#[cfg(test)]
mod tests;
