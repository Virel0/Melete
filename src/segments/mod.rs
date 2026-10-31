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

pub mod ending;
pub mod fingerprint;

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
        record(
            pool,
            row,
            None,
            intro.map(|(a, _, _)| a),
            credits.map(|(a, _, _)| a),
        )
        .await?;
        fresh.push(row.id);
        report.scanned += 1;
        report.intros += usize::from(intro.is_some());
        report.credits += usize::from(credits.is_some());
        // A partner looked at before that found nothing of the kind gets what it shares.
        for (kind, found) in [("intro", intro), ("credits", credits)] {
            if let Some((_, partner, theirs)) = found
                && fresh.contains(&partner)
            {
                sqlx::query(
                    "INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds)
                     VALUES (?, ?, ?, ?) ON CONFLICT (file_id, kind) DO NOTHING",
                )
                .bind(partner)
                .bind(kind)
                .bind(theirs.start)
                .bind(theirs.end)
                .execute(pool)
                .await?;
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

/// Keeps what was found in this version of a file, replacing what was found before.
async fn record(
    pool: &SqlitePool,
    row: &Row,
    error: Option<&str>,
    intro: Option<Segment>,
    credits: Option<Segment>,
) -> anyhow::Result<()> {
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
    for (kind, segment) in [("intro", intro), ("credits", credits)] {
        if let Some(segment) = segment {
            sqlx::query(
                "INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(row.id)
            .bind(kind)
            .bind(segment.start)
            .bind(segment.end)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
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

/// A file's intro and credits, when its current version has been looked at: `None` when it
/// hasn't been, by matching episodes or alone. Credits found by matching come first; those
/// found in the file alone count where matching found none. The API asks [`sourced`].
#[cfg(test)]
pub async fn of_file(
    pool: &SqlitePool,
    file_id: i64,
) -> anyhow::Result<Option<(Option<Segment>, Option<Segment>)>> {
    let found = sourced(pool, file_id).await?;
    Ok(found.map(|(intro, credits)| (intro, credits.map(|(c, _)| c))))
}

/// How a segment was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// By matching audio an episode shares with others of its season.
    Audio,
    /// By the black or silent ending of the file alone.
    Ending,
}

/// As [`of_file`], with how the credits were found (an intro is only found by matching).
pub async fn sourced(
    pool: &SqlitePool,
    file_id: i64,
) -> anyhow::Result<Option<(Option<Segment>, Option<(Segment, Found)>)>> {
    let scanned: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM media_segment_scans s JOIN media_files f ON f.id = s.file_id
             WHERE s.file_id = ? AND s.size = f.size AND s.modified_at = f.modified_at)",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await?;
    let alone: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT c.start_seconds, c.end_seconds FROM media_credit_scans c
         JOIN media_files f ON f.id = c.file_id
         WHERE c.file_id = ? AND c.size = f.size AND c.modified_at = f.modified_at",
    )
    .bind(file_id)
    .fetch_optional(pool)
    .await?;
    let alone_credits = match alone {
        Some((Some(start), Some(end))) => Some((Segment { start, end }, Found::Ending)),
        _ => None,
    };
    if !scanned {
        return Ok(alone.map(|_| (None, alone_credits)));
    }
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT kind, start_seconds, end_seconds FROM media_segments WHERE file_id = ?",
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;
    let find = |kind: &str| {
        rows.iter()
            .find(|(k, _, _)| k == kind)
            .map(|(_, start, end)| Segment {
                start: *start,
                end: *end,
            })
    };
    let matched = find("credits").map(|c| (c, Found::Audio));
    Ok(Some((find("intro"), matched.or(alone_credits))))
}

#[cfg(test)]
mod tests {
    use super::fingerprint::tests::{Noise, episode};
    use super::*;
    use crate::db;

    /// Writes mono 16-bit audio at the fingerprints' rate as a WAV file, then, for `lossy`,
    /// makes it an AAC file instead, as a broadcast would be.
    fn write_audio(path: &Path, samples: &[f32], lossy: bool) {
        let wav = path.with_extension("wav");
        let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
        let data_len = (samples.len() * 2) as u32;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&fingerprint::RATE.to_le_bytes());
        bytes.extend_from_slice(&(fingerprint::RATE * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            bytes.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(&wav, bytes).unwrap();
        if lossy {
            let status = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-i"])
                .arg(&wav)
                .args(["-c:a", "aac", "-b:a", "96k"])
                .arg(path)
                .status()
                .expect("ffmpeg runs");
            assert!(status.success());
            std::fs::remove_file(&wav).unwrap();
        } else {
            std::fs::rename(&wav, path).unwrap();
        }
    }

    struct Season {
        pool: SqlitePool,
        _dir: tempfile::TempDir,
    }

    /// Three 6-minute episodes of one season sharing a 40-second intro and a 30-second ending
    /// song at different times, plus a special and an episode of another show alone in its
    /// season.
    async fn season() -> Season {
        let dir = tempfile::tempdir().unwrap();
        let intro = Noise::new(7).tune(40.0);
        let song = Noise::new(8).tune(30.0);
        // (file, before the intro, after it until the song, after the song)
        let layout = [
            ("S01E01.m4a", 20.0, 225.0, 45.0),
            ("S01E02.wav", 45.3, 195.0, 49.7),
            ("S01E03.wav", 5.0, 240.0, 45.0),
            ("S00E01.wav", 20.0, 225.0, 45.0),
            ("Other.S01E01.wav", 20.0, 225.0, 45.0),
        ];
        for (n, (name, before, middle, after)) in layout.iter().enumerate() {
            let mut samples = episode(n as u64 + 1, *before, &intro, *middle);
            samples.extend_from_slice(&song);
            samples.extend(Noise::new(n as u64 + 50).tune(*after));
            write_audio(&dir.path().join(name), &samples, name.ends_with(".m4a"));
        }
        let pool = db::open_in_memory().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '{}');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation) VALUES
                 (1, 1, 'Show', 'Show', 1), (2, 1, 'Other', 'Other', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                 season, episode, first_seen_generation, seen_generation) VALUES
                 (1, 1, 'S01E01.m4a', 1, 1, 1, 1, 1, 1, 1),
                 (2, 1, 'S01E02.wav', 1, 1, 1, 1, 2, 1, 1),
                 (3, 1, 'S01E03.wav', 1, 1, 1, 1, 3, 1, 1),
                 (4, 1, 'S00E01.wav', 1, 1, 1, 0, 1, 1, 1),
                 (5, 1, 'Other.S01E01.wav', 1, 1, 2, 1, 1, 1, 1);
             INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
                 SELECT id, 1, 1, 0, '{{\"duration_seconds\": 360.0}}' FROM media_files;",
            dir.path().display()
        )))
        .execute(&pool)
        .await
        .unwrap();
        Season { pool, _dir: dir }
    }

    async fn segments(pool: &SqlitePool) -> Vec<(i64, String, f64, f64)> {
        sqlx::query_as(
            "SELECT file_id, kind, start_seconds, end_seconds FROM media_segments
             ORDER BY file_id, kind DESC",
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    fn near(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1.0
    }

    #[tokio::test]
    async fn finds_a_seasons_intros_and_credits_once() {
        let s = season().await;
        let report = find_pending(&s.pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!(
            report,
            Report {
                scanned: 3,
                intros: 3,
                credits: 3,
                failed: 0
            },
            "the special and the lone episode wait"
        );
        let found = segments(&s.pool).await;
        let expected = [
            (1, "intro", 20.0, 60.0),
            (1, "credits", 285.0, 315.0),
            (2, "intro", 45.3, 85.3),
            (2, "credits", 280.3, 310.3),
            (3, "intro", 5.0, 45.0),
            (3, "credits", 285.0, 315.0),
        ];
        assert_eq!(found.len(), expected.len(), "{found:?}");
        for ((file, kind, start, end), (e_file, e_kind, e_start, e_end)) in
            found.iter().zip(expected)
        {
            assert_eq!((*file, kind.as_str()), (e_file, e_kind));
            assert!(
                near(*start, e_start) && near(*end, e_end),
                "{file} {kind}: {start}-{end}, not {e_start}-{e_end}"
            );
        }
        let (intro, credits) = of_file(&s.pool, 2).await.unwrap().unwrap();
        assert!(near(intro.unwrap().start, 45.3));
        assert!(credits.is_some());
        assert_eq!(of_file(&s.pool, 5).await.unwrap(), None, "not looked at");

        // Nothing more to do until a file changes.
        let again = find_pending(&s.pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!(again, Report::default());
        sqlx::query("UPDATE media_files SET size = 2 WHERE id = 3")
            .execute(&s.pool)
            .await
            .unwrap();
        assert_eq!(of_file(&s.pool, 3).await.unwrap(), None, "a changed file");
        sqlx::query("UPDATE media_probes SET size = 2 WHERE file_id = 3")
            .execute(&s.pool)
            .await
            .unwrap();
        let changed = find_pending(&s.pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!(changed.scanned, 1);
    }

    #[tokio::test]
    async fn nothing_is_looked_for_until_it_is_turned_on() {
        let s = season().await;
        let report = run(&s.pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!(report, Report::default());
        let scans: i64 = sqlx::query_scalar("SELECT count(*) FROM media_segment_scans")
            .fetch_one(&s.pool)
            .await
            .unwrap();
        assert_eq!(scans, 0);
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_INTRO_DETECTION', 'on', 0)",
        )
        .execute(&s.pool)
        .await
        .unwrap();
        assert_eq!(run(&s.pool, Path::new("ffmpeg")).await.unwrap().scanned, 3);
    }

    #[tokio::test]
    async fn an_unreadable_episode_is_kept_with_why() {
        let s = season().await;
        std::fs::write(s._dir.path().join("S01E03.wav"), b"not audio").unwrap();
        let report = find_pending(&s.pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!((report.scanned, report.failed), (2, 1));
        let error: Option<String> =
            sqlx::query_scalar("SELECT error FROM media_segment_scans WHERE file_id = 3")
                .fetch_one(&s.pool)
                .await
                .unwrap();
        assert!(error.is_some_and(|e| !e.is_empty()));
        assert_eq!(of_file(&s.pool, 3).await.unwrap(), Some((None, None)));
        assert!(
            find_pending(&s.pool, Path::new("no-such-ffmpeg"))
                .await
                .is_err(),
            "without ffmpeg nothing is recorded"
        );
    }
}
