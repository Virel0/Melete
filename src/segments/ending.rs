//! Credits found in one file alone: in movies, and in episodes whose credits share no
//! sound with the rest of their season (music of their own each week, or none) or that have
//! no season to compare with (specials, an episode alone in its season).
//!
//! The end of the file is read through ffmpeg: a frame a second, small and in grey, through
//! its `blackframe` filter, and the sound as the fingerprints read it (mono, 8 kHz). A second
//! looks like credits when its picture is mostly black (text on black: [`BLACK_SHARE`] percent
//! of its pixels or more darker than [`BLACK_LEVEL`]) or its sound is near silence (quieter
//! than [`QUIET_DB`]). The credits are the run of such seconds that reaches the end, a few
//! other seconds within it bridged and a logo of a few seconds after it allowed, when that run
//! is at least half a minute long. Rolling credits over pictures with sound aren't found.
//!
//! Matching a season's episodes finds credits more surely, so its credits come first; these
//! are kept apart (`media_credit_scans`), each version of a file once, and served only where
//! matching found none.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sqlx::SqlitePool;

use super::{Segment, audio, fingerprint, round, unix_now};

/// How much of the end is read: a quarter of the file, at most this long.
const END_SHARE: f64 = 0.25;
const MOVIE_END_MOST: f64 = 720.0;
const EPISODE_END_MOST: f64 = 360.0;
/// The shortest run of seconds taken for credits.
const SHORTEST_CREDITS: usize = 30;
/// Other seconds bridged within the credits.
const GAP: usize = 3;
/// Other seconds allowed after the credits, before the end (a distributor's logo, say).
const TAIL: usize = 10;
/// A frame is mostly black when this percentage of its pixels or more are darker than
/// [`BLACK_LEVEL`] (of 255): text on black leaves most of the screen black.
const BLACK_SHARE: u32 = 80;
const BLACK_LEVEL: u32 = 32;
/// A second of sound quieter than this, in dB below full scale, is near silence.
const QUIET_DB: f64 = -50.0;
/// Decoding video takes longer than sound alone.
const DECODE_TIMEOUT: Duration = Duration::from_secs(600);

/// What a pass found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Files whose end was looked at, failures included.
    pub scanned: usize,
    /// Credits found.
    pub credits: usize,
    /// Of those, files whose end couldn't be decoded.
    pub failed: usize,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: i64,
    root: String,
    rel_path: String,
    size: i64,
    modified_at: i64,
    movie: bool,
    duration: f64,
    video: bool,
    sound: bool,
}

/// Looks at the end of every file that needs it, in its current version: movies, and
/// episodes that matching found no credits in or can't look at.
pub async fn find_pending(pool: &SqlitePool, ffmpeg: &Path) -> anyhow::Result<Found> {
    // An episode matching looks at: one of a season (not 0) with another episode in it, both
    // probed and long enough, as `super::find_pending` picks them.
    let rows: Vec<Row> = sqlx::query_as(
        "WITH probed AS (
             SELECT f.*, l.path AS root, p.info,
                 json_extract(p.info, '$.duration_seconds') AS duration
             FROM media_files f
             JOIN libraries l ON l.id = f.library_id
             JOIN media_probes p ON p.file_id = f.id AND p.size = f.size
                 AND p.modified_at = f.modified_at
             WHERE f.missing_since IS NULL
               AND json_extract(p.info, '$.duration_seconds') >= ?1
         )
         SELECT f.id, f.root, f.rel_path, f.size, f.modified_at, f.movie_id IS NOT NULL AS movie,
             f.duration,
             coalesce(json_array_length(f.info, '$.video'), 0) > 0 AS video,
             coalesce(json_array_length(f.info, '$.audio'), 0) > 0 AS sound
         FROM probed f
         LEFT JOIN media_credit_scans c ON c.file_id = f.id
         WHERE NOT (c.file_id IS NOT NULL AND c.size = f.size AND c.modified_at = f.modified_at)
           AND (
               f.movie_id IS NOT NULL
               OR f.season IS NULL OR f.season = 0
               OR NOT EXISTS (
                   SELECT 1 FROM probed o WHERE o.series_id = f.series_id
                       AND o.season = f.season AND o.episode <> f.episode)
               OR (EXISTS (
                       SELECT 1 FROM media_segment_scans s WHERE s.file_id = f.id
                           AND s.size = f.size AND s.modified_at = f.modified_at)
                   AND NOT EXISTS (
                       SELECT 1 FROM media_segments m
                       WHERE m.file_id = f.id AND m.kind = 'credits'))
           )
         ORDER BY f.id",
    )
    .bind(super::SHORTEST)
    .fetch_all(pool)
    .await?;
    let mut found = Found::default();
    for row in rows {
        let path = Path::new(&row.root).join(&row.rel_path);
        let most = if row.movie {
            MOVIE_END_MOST
        } else {
            EPISODE_END_MOST
        };
        let length = (row.duration * END_SHARE).min(most);
        let from = row.duration - length;
        let seconds = length.ceil() as usize;
        let mut looks = vec![false; seconds];
        let mut failed = None;
        if row.video {
            match black_seconds(ffmpeg, &path, from, length).await {
                Ok(Some(black)) => mark(&mut looks, black),
                Ok(None) => continue,
                Err(why) => failed = Some(why),
            }
        }
        if row.sound && failed.is_none() {
            match audio(ffmpeg, &path, from, length).await {
                Ok(Some(samples)) => mark(&mut looks, quiet_seconds(&samples)),
                Ok(None) => continue,
                Err(why) => failed = Some(why),
            }
        }
        let credits = match &failed {
            Some(why) => {
                tracing::warn!(rel_path = row.rel_path, why, "couldn't read a file's end");
                found.failed += 1;
                None
            }
            None => run_to_end(&looks).map(|(start, end)| Segment {
                start: round(from + start as f64),
                end: round((from + end as f64).min(row.duration)),
            }),
        };
        record(pool, &row, failed.as_deref(), credits).await?;
        found.scanned += 1;
        found.credits += usize::from(credits.is_some());
    }
    Ok(found)
}

fn mark(looks: &mut [bool], seconds: impl IntoIterator<Item = usize>) {
    for second in seconds {
        if let Some(look) = looks.get_mut(second) {
            *look = true;
        }
    }
}

/// Where the credits are among a window's seconds, each true when it looks like credits: the
/// run reaching the end (but for at most [`TAIL`] other seconds), with at most [`GAP`] other
/// seconds in a row bridged, when it's [`SHORTEST_CREDITS`] long or more. From its first
/// second to just after its last.
pub fn run_to_end(looks: &[bool]) -> Option<(usize, usize)> {
    let last = looks.iter().rposition(|l| *l)?;
    if looks.len() - 1 - last > TAIL {
        return None;
    }
    let mut start = last;
    let mut gap = 0;
    for i in (0..last).rev() {
        if looks[i] {
            start = i;
            gap = 0;
        } else {
            gap += 1;
            if gap > GAP {
                break;
            }
        }
    }
    let end = last + 1;
    (end - start >= SHORTEST_CREDITS).then_some((start, end))
}

/// The seconds of sound (at the fingerprints' rate) quieter than [`QUIET_DB`].
fn quiet_seconds(samples: &[f32]) -> Vec<usize> {
    samples
        .chunks(fingerprint::RATE as usize)
        .enumerate()
        .filter(|(_, second)| {
            let power = second.iter().map(|s| s * s).sum::<f32>() / second.len() as f32;
            10.0 * f64::from(power).log10() < QUIET_DB
        })
        .map(|(n, _)| n)
        .collect()
}

/// The seconds from `start` whose frame is mostly black, read through ffmpeg's `blackframe`;
/// `None` when it took too long, `Err` with ffmpeg's word when it couldn't be read.
async fn black_seconds(
    ffmpeg: &Path,
    path: &Path,
    start: f64,
    length: f64,
) -> Result<Option<Vec<usize>>, String> {
    let filter =
        format!("fps=1,scale=160:-2,format=gray,blackframe=amount=0:threshold={BLACK_LEVEL}");
    let child = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-hide_banner", "-nostats", "-loglevel", "info"])
        .args(["-ss", &format!("{start:.3}"), "-t", &format!("{length:.3}")])
        .arg("-i")
        .arg(path)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &filter])
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("ffmpeg didn't start: {err}"))?;
    let Ok(out) = tokio::time::timeout(DECODE_TIMEOUT, child.wait_with_output()).await else {
        return Ok(None);
    };
    let out = out.map_err(|err| format!("ffmpeg failed: {err}"))?;
    let log = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        let last = log
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(last.trim().chars().take(200).collect());
    }
    Ok(Some(log.lines().filter_map(black_frame).collect()))
}

/// The second a `blackframe` line is about, when its frame is mostly black.
fn black_frame(line: &str) -> Option<usize> {
    let field = |name: &str| {
        line.split_whitespace()
            .find_map(|word| word.strip_prefix(name))
            .and_then(|value| value.parse::<f64>().ok())
    };
    let black = field("pblack:")?;
    let at = field("t:")?;
    (black >= f64::from(BLACK_SHARE) && at >= 0.0).then_some(at.floor() as usize)
}

/// Keeps what was found at the end of this version of a file.
async fn record(
    pool: &SqlitePool,
    row: &Row,
    error: Option<&str>,
    credits: Option<Segment>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO media_credit_scans (file_id, size, modified_at, scanned_at, error,
             start_seconds, end_seconds)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (file_id) DO UPDATE SET size = excluded.size,
             modified_at = excluded.modified_at, scanned_at = excluded.scanned_at,
             error = excluded.error, start_seconds = excluded.start_seconds,
             end_seconds = excluded.end_seconds",
    )
    .bind(row.id)
    .bind(row.size)
    .bind(row.modified_at)
    .bind(unix_now())
    .bind(error)
    .bind(credits.map(|c| c.start))
    .bind(credits.map(|c| c.end))
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn looks(pattern: &str) -> Vec<bool> {
        pattern.chars().map(|c| c == '#').collect()
    }

    #[test]
    fn credits_are_the_run_that_reaches_the_end() {
        let film = ".".repeat(40);
        let credits = "#".repeat(35);
        // Straight to the end.
        assert_eq!(
            run_to_end(&looks(&format!("{film}{credits}"))),
            Some((40, 75))
        );
        // A few other seconds within the credits are bridged, and a logo after them allowed.
        let broken = format!(
            "{film}{}...{}{}",
            "#".repeat(20),
            "#".repeat(15),
            ".".repeat(8)
        );
        assert_eq!(run_to_end(&looks(&broken)), Some((40, 78)));
        // A dark moment in the film, apart from the credits, isn't taken in.
        let dark = format!("{}##{}{credits}", ".".repeat(20), ".".repeat(18));
        assert_eq!(run_to_end(&looks(&dark)), Some((40, 75)));
        // Too short, or ending too long before the end of the file: no credits.
        assert_eq!(
            run_to_end(&looks(&format!("{film}{}", "#".repeat(29)))),
            None
        );
        assert_eq!(
            run_to_end(&looks(&format!("{film}{credits}{}", ".".repeat(11)))),
            None
        );
        assert_eq!(run_to_end(&looks(&film)), None);
        assert_eq!(run_to_end(&[]), None);
    }

    #[test]
    fn reads_ffmpegs_blackframe_lines() {
        let line = "[Parsed_blackframe_3 @ 0x7f] frame:12 pblack:96 pts:12 t:12.000000 type:P last_keyframe:0";
        assert_eq!(black_frame(line), Some(12));
        let lit = "[Parsed_blackframe_3 @ 0x7f] frame:3 pblack:23 pts:3 t:3.000000 type:P last_keyframe:0";
        assert_eq!(black_frame(lit), None);
        assert_eq!(black_frame("Stream #0:0: Video: h264"), None);
    }

    #[test]
    fn silence_is_quiet_and_a_tone_is_not() {
        let rate = fingerprint::RATE as usize;
        let mut samples: Vec<f32> = (0..rate * 2)
            .map(|n| (n as f32 * 0.3).sin() * 0.5)
            .collect();
        samples.extend(std::iter::repeat_n(0.0005, rate * 2));
        assert_eq!(quiet_seconds(&samples), [2, 3]);
    }

    /// A clip of `parts` (each a picture and a sound, as lavfi sources with their own options,
    /// and seconds), one after another, small so it's quick to make. A picture may name a
    /// filter after it, past a comma.
    fn clip(path: &Path, parts: &[(&str, &str, u32)]) {
        let mut command = std::process::Command::new("ffmpeg");
        command.args(["-v", "error", "-y"]);
        for (picture, _, seconds) in parts {
            let (source, filter) = picture.split_once(',').unwrap_or((picture, ""));
            let filter = if filter.is_empty() {
                String::new()
            } else {
                format!(",{filter}")
            };
            command.args(["-f", "lavfi", "-i"]).arg(format!(
                "{source}{}size=160x120:rate=10:duration={seconds}{filter}",
                if source.contains('=') { ':' } else { '=' }
            ));
        }
        for (_, sound, seconds) in parts {
            command
                .args(["-f", "lavfi", "-i"])
                .arg(format!("{sound}:duration={seconds}"));
        }
        let n = parts.len();
        let mut graph = String::new();
        for i in 0..n {
            graph.push_str(&format!("[{i}:v]"));
        }
        graph.push_str(&format!("concat=n={n}:v=1:a=0[v];"));
        for i in 0..n {
            graph.push_str(&format!("[{}:a]", n + i));
        }
        graph.push_str(&format!("concat=n={n}:v=0:a=1[a]"));
        let status = command
            .args(["-filter_complex", &graph, "-map", "[v]", "-map", "[a]"])
            .args([
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ])
            .args(["-c:a", "aac", "-ac", "1"])
            .arg(path)
            .status()
            .expect("ffmpeg runs");
        assert!(status.success());
    }

    /// Names scrolling up a black screen: white bars, as text would be.
    const NAMES: &str = "color=c=black";
    const NAMES_SCROLL: &str = ",drawbox=x=30:y='mod(t*20,120)':w=100:h=6:color=white:t=fill";

    #[tokio::test]
    async fn finds_credits_on_black_and_silent_ones_in_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let music = "sine=frequency=330";
        let names = format!("{NAMES}{NAMES_SCROLL}");
        // Names on black over music; a film whose credits play over its last scene in
        // silence; one with no credits to find.
        clip(
            &dir.path().join("black.mkv"),
            &[("testsrc", music, 240), (names.as_str(), music, 70)],
        );
        clip(
            &dir.path().join("silent.mkv"),
            &[
                ("testsrc", music, 250),
                ("testsrc", "anullsrc=r=44100:cl=mono", 60),
            ],
        );
        clip(&dir.path().join("none.mkv"), &[("testsrc", music, 310)]);
        let pool = db::open_in_memory().await;
        let info =
            r#"{"duration_seconds": 310.0, "video": [{"index": 0}], "audio": [{"index": 1}]}"#;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '{}');
             INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation) VALUES
                 (1, 1, 'a', 'A', 1), (2, 1, 'b', 'B', 1), (3, 1, 'c', 'C', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
                 first_seen_generation, seen_generation) VALUES
                 (1, 1, 'black.mkv', 1, 1, 1, 1, 1),
                 (2, 1, 'silent.mkv', 1, 1, 2, 1, 1),
                 (3, 1, 'none.mkv', 1, 1, 3, 1, 1);
             INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
                 SELECT id, 1, 1, 0, '{info}' FROM media_files;",
            dir.path().display()
        )))
        .execute(&pool)
        .await
        .unwrap();

        let found = find_pending(&pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!(
            found,
            Found {
                scanned: 3,
                credits: 2,
                failed: 0
            }
        );
        let near = |actual: f64, expected: f64| (actual - expected).abs() <= 1.5;
        let credits = |id| {
            let pool = pool.clone();
            async move { super::super::of_file(&pool, id).await.unwrap() }
        };
        let (intro, black) = credits(1).await.unwrap();
        assert_eq!(intro, None);
        let black = black.unwrap();
        assert!(
            near(black.start, 240.0) && near(black.end, 310.0),
            "{black:?}"
        );
        let silent = credits(2).await.unwrap().1.unwrap();
        assert!(
            near(silent.start, 250.0) && near(silent.end, 310.0),
            "{silent:?}"
        );
        assert_eq!(
            credits(3).await,
            Some((None, None)),
            "looked at, nothing found"
        );

        // Each version once.
        assert_eq!(
            find_pending(&pool, Path::new("ffmpeg")).await.unwrap(),
            Found::default()
        );
    }

    #[tokio::test]
    async fn looks_only_where_matching_episodes_cant_or_found_nothing() {
        let pool = db::open_in_memory().await;
        // No streams listed, so nothing is decoded: this is only about which files are picked.
        sqlx::raw_sql(
            r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '/x');
             INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
                 VALUES (1, 1, 'Show', 'Show', 1), (2, 1, 'Other', 'Other', 1);
             INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
                 season, episode, first_seen_generation, seen_generation) VALUES
                 (1, 1, 'S01E01', 1, 1, 1, 1, 1, 1, 1),
                 (2, 1, 'S01E02', 1, 1, 1, 1, 2, 1, 1),
                 (3, 1, 'S01E03', 1, 1, 1, 1, 3, 1, 1),
                 (4, 1, 'S00E01', 1, 1, 1, 0, 1, 1, 1),
                 (5, 1, 'Other.S01E01', 1, 1, 2, 1, 1, 1, 1);
             INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
                 SELECT id, 1, 1, 0, '{"duration_seconds": 360.0}' FROM media_files;
             -- Matching looked at episodes 1 and 2 and found credits in 1; 3 waits for it.
             INSERT INTO media_segment_scans (file_id, size, modified_at, scanned_at)
                 VALUES (1, 1, 1, 0), (2, 1, 1, 0);
             INSERT INTO media_segments (file_id, kind, start_seconds, end_seconds)
                 VALUES (1, 'credits', 300, 340);"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let found = find_pending(&pool, Path::new("ffmpeg")).await.unwrap();
        assert_eq!(found.scanned, 3);
        let looked: Vec<i64> =
            sqlx::query_scalar("SELECT file_id FROM media_credit_scans ORDER BY file_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            looked,
            [2, 4, 5],
            "found nothing, a special, alone in its season"
        );

        // Credits found alone count only where matching found none.
        sqlx::raw_sql(
            "UPDATE media_credit_scans SET start_seconds = 320, end_seconds = 360;
             INSERT INTO media_credit_scans (file_id, size, modified_at, scanned_at,
                 start_seconds, end_seconds) VALUES (1, 1, 1, 0, 320, 360);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let credits = |id| {
            let pool = pool.clone();
            async move { super::super::of_file(&pool, id).await.unwrap().unwrap().1 }
        };
        assert_eq!(credits(1).await.unwrap().start, 300.0, "matching's first");
        assert_eq!(credits(2).await.unwrap().start, 320.0);
        assert_eq!(credits(4).await.unwrap().start, 320.0);
        // A changed file counts as not looked at, until it's looked at again.
        sqlx::query("UPDATE media_files SET size = 2 WHERE id = 4")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(super::super::of_file(&pool, 4).await.unwrap(), None);
    }
}
