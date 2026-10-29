//! Measuring music tracks' audio: one ffmpeg decode per file,
//! every measurement taken from that one pass, by the aoide-sidecar's analyzers ported so the
//! server and the apps agree. Tracks are measured after each scan, one file at a time, and
//! measured again only when the file changes or the analysis does ([`ANALYZER`]).

pub mod arrangement;
pub mod beat_grid;
pub mod bounds;
pub(crate) mod fft;
pub mod key;
pub mod loudness;
pub mod tempo;
pub mod vocals;

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use arrangement::Arrangement;
use beat_grid::BeatGrid;
use bounds::{BoundsScan, SoundBounds};
use key::{KeyScan, MusicalKey};
use loudness::Loudness;
use tempo::{Tempo, TempoScan};
use vocals::VocalScan;

mod decoding;
mod queue;
mod storing;

pub use decoding::*;
pub use queue::*;
use storing::*;

/// Bumped when a change to the analysis should measure every track again.
pub const ANALYZER: i64 = 2;
/// Longest one file may take: a two-hour mix decodes in a few minutes.
const TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Tracks read from the database at a time.
const BATCH: i64 = 50;

/// Something that looks at every sample of a decoded file.
pub trait Consumer {
    /// The next block of interleaved samples; always whole frames.
    fn feed(&mut self, samples: &[f32]);
}

/// Every scan fed from the one decode.
struct Scans {
    bounds: BoundsScan,
    tempo: TempoScan,
    key: KeyScan,
    voice: VocalScan,
}

impl Consumer for Scans {
    fn feed(&mut self, samples: &[f32]) {
        self.bounds.feed(samples);
        self.tempo.feed(samples);
        self.key.feed(samples);
        self.voice.feed(samples);
    }
}

/// Reads little-endian float PCM to the end, feeding whole frames to `consumer` on a
/// blocking thread so the scans' arithmetic stays off the async workers. A few blocks are
/// buffered between the two, never the whole file. Returns the frames read and the consumer.
pub async fn pump<R, C>(mut reader: R, channels: usize, consumer: C) -> anyhow::Result<(i64, C)>
where
    R: AsyncRead + Unpin,
    C: Consumer + Send + 'static,
{
    let (blocks, mut received) = tokio::sync::mpsc::channel::<Vec<f32>>(4);
    let worker = tokio::task::spawn_blocking(move || {
        let mut consumer = consumer;
        while let Some(block) = received.blocking_recv() {
            consumer.feed(&block);
        }
        consumer
    });
    let frame_bytes = channels * 4;
    let mut buffer = vec![0u8; frame_bytes * 16_384];
    // Bytes short of a frame boundary wait for the next read rather than split a sample.
    let mut held = 0;
    let mut total = 0;
    loop {
        let read = reader.read(&mut buffer[held..]).await?;
        if read == 0 {
            break;
        }
        let available = held + read;
        let whole = available / frame_bytes * frame_bytes;
        if whole > 0 {
            let block = buffer[..whole]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            if blocks.send(block).await.is_err() {
                break;
            }
            total += (whole / frame_bytes) as i64;
        }
        buffer.copy_within(whole..available, 0);
        held = available - whole;
    }
    drop(blocks);
    Ok((total, worker.await?))
}

/// Everything one decode of a file measured; each part is none when it couldn't be told.
#[derive(Debug, Default, PartialEq)]
pub struct Measurement {
    /// Where the sound starts and stops; none when there's nothing worth trimming.
    pub bounds: Option<SoundBounds>,
    /// Its loudness; none for a track too short or silent to measure.
    pub loudness: Option<Loudness>,
    /// Its tempo.
    pub tempo: Option<Tempo>,
    /// Its beat grid.
    pub grid: Option<BeatGrid>,
    /// Its musical key.
    pub key: Option<MusicalKey>,
    /// Its sections, phrases and singing.
    pub arrangement: Option<Arrangement>,
}

/// Why a file couldn't be measured.
#[derive(Debug)]
pub enum MeasureError {
    /// ffmpeg or ffprobe can't run at all: nothing is learned about any file.
    Unavailable(anyhow::Error),
    /// The file couldn't be read, or the decode was cut short; tried again on a later run.
    Unreadable(String),
    /// ffmpeg read the file and rejected it; not tried again until it changes.
    Failed(String),
}

impl std::fmt::Display for MeasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(err) => write!(f, "ffmpeg can't run: {err:#}"),
            Self::Unreadable(reason) => write!(f, "couldn't read the file: {reason}"),
            Self::Failed(reason) => write!(f, "ffmpeg rejected the file: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools() -> Tools {
        Tools {
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
        }
    }

    /// 7.5 s of stereo float WAV: silence, a 440 Hz tone at half scale from 2 s to 5 s, silence.
    /// Made with the local ffmpeg, a test prerequisite.
    fn make_track(dir: &Path) -> PathBuf {
        let path = dir.join("tone.wav");
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("aevalsrc=if(between(t\\,2\\,5)\\,0.5*sin(2*PI*440*t)\\,0)|if(between(t\\,2\\,5)\\,0.5*sin(2*PI*440*t)\\,0):s=44100:d=7.5")
            .args(["-c:a", "pcm_f32le"])
            .arg(&path)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        path
    }

    #[tokio::test]
    async fn measures_bounds_and_loudness_from_one_decode() {
        let dir = tempfile::tempdir().unwrap();
        let measured = measure(&tools(), &make_track(dir.path())).await.unwrap();
        // The sidecar's own fixture, decoded by ffmpeg this time: exact, as the audio is.
        assert_eq!(
            measured.bounds,
            Some(SoundBounds {
                start_ms: 1940,
                end_ms: 5200
            })
        );
        let loudness = measured.loudness.unwrap();
        assert!((-20.0..0.0).contains(&loudness.lufs), "{loudness:?}");
        // A sine at half scale peaks at −6 dBFS.
        assert!((loudness.true_peak_dbfs + 6.0).abs() < 0.5, "{loudness:?}");
    }

    #[tokio::test]
    async fn measures_tempo_from_the_same_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clicks.flac");
        let click = "0.8*exp(-mod(t\\,60/128)/0.04)*sin(2*PI*1000*t)";
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("aevalsrc={click}|{click}:s=44100:d=30"))
            .arg(&path)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        let tempo = measure(&tools(), &path).await.unwrap().tempo.unwrap();
        assert!((tempo.bpm - 128.0).abs() <= 0.2, "{tempo:?}");
        assert!(tempo.confidence >= tempo::MINIMUM_CONFIDENCE, "{tempo:?}");
        // Thirty seconds is under three twenty-second windows.
        assert_eq!(tempo.stability, None);
    }

    #[tokio::test]
    async fn tells_a_broken_file_from_a_missing_one_and_a_missing_ffmpeg() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.flac");
        std::fs::write(&junk, "not really audio").unwrap();
        assert!(matches!(
            measure(&tools(), &junk).await,
            Err(MeasureError::Failed(_))
        ));
        assert!(matches!(
            measure(&tools(), &dir.path().join("gone.flac")).await,
            Err(MeasureError::Unreadable(_))
        ));
        let nowhere = Tools {
            ffmpeg: dir.path().join("no-ffmpeg"),
            ffprobe: dir.path().join("no-ffprobe"),
        };
        assert!(matches!(
            measure(&nowhere, &junk).await,
            Err(MeasureError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn measures_each_track_until_its_file_or_the_analysis_changes() {
        let dir = tempfile::tempdir().unwrap();
        make_track(dir.path());
        std::fs::write(dir.path().join("junk.flac"), "not really audio").unwrap();
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title, missing_since) VALUES
                 (1, 1, 'tone.wav', 1, 100, 1, 1, 0, 'Tone', NULL),
                 (2, 1, 'junk.flac', 1, 100, 1, 1, 0, 'Junk', NULL),
                 (3, 1, 'gone.flac', 1, 100, 1, 1, 0, 'Gone', NULL),
                 (4, 1, 'old.flac', 1, 100, 1, 1, 0, 'Old', 5);",
        )
        .bind(dir.path().to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();

        let first = measure_pending(&pool, &tools(), &Requests::new())
            .await
            .unwrap();
        assert_eq!(
            first,
            Report {
                measured: 1,
                failed: 1,
                retry_later: 1
            }
        );
        type Row = (i64, Option<i64>, Option<i64>, Option<String>);
        let bounds: Vec<Row> = sqlx::query_as(
            "SELECT track_id, sound_start_ms, sound_end_ms, error FROM music_sound_bounds
             ORDER BY track_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(bounds.len(), 2, "nothing for a file that couldn't be read");
        assert_eq!(bounds[0], (1, Some(1940), Some(5200), None));
        assert!(bounds[1].3.as_deref().unwrap().contains("rejected"));
        let lufs: Option<f64> =
            sqlx::query_scalar("SELECT loudness_lufs FROM music_audio_analysis WHERE track_id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(lufs.is_some());

        // Only the unreadable one is tried again.
        let again = measure_pending(&pool, &tools(), &Requests::new())
            .await
            .unwrap();
        assert_eq!(
            again,
            Report {
                retry_later: 1,
                ..Report::default()
            }
        );
        // A changed file, and a measurement from an older analysis, are measured again.
        sqlx::raw_sql(
            "UPDATE music_tracks SET modified_at = 200 WHERE id = 1;
             UPDATE music_audio_analysis SET analyzer = 0 WHERE track_id = 2;
             UPDATE music_tracks SET missing_since = 9 WHERE id = 3;",
        )
        .execute(&pool)
        .await
        .unwrap();
        let changed = measure_pending(&pool, &tools(), &Requests::new())
            .await
            .unwrap();
        assert_eq!(
            changed,
            Report {
                measured: 1,
                failed: 1,
                retry_later: 0
            }
        );
        let modified: i64 =
            sqlx::query_scalar("SELECT modified_at FROM music_sound_bounds WHERE track_id = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(modified, 200);
    }

    #[tokio::test]
    async fn a_track_asked_about_is_measured_first_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let tone = make_track(dir.path());
        std::fs::copy(&tone, dir.path().join("second.wav")).unwrap();
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title) VALUES
                 (1, 1, 'tone.wav', 1, 100, 1, 1, 0, 'First'),
                 (2, 1, 'second.wav', 1, 100, 1, 1, 0, 'Second');",
        )
        .bind(dir.path().to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
        let requests = Requests::new();
        // An unknown track is asked about too, and simply isn't there.
        requests.request(&[2, 99]);
        let report = measure_pending(&pool, &tools(), &requests).await.unwrap();
        assert_eq!(report.measured, 2);
        let order: Vec<i64> =
            sqlx::query_scalar("SELECT track_id FROM music_sound_bounds ORDER BY measured_at")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(order, [2, 1]);
    }

    #[tokio::test]
    async fn the_same_decode_gives_a_beat_grid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clicks.flac");
        let click = "0.8*exp(-mod(t\\,60/128)/0.04)*sin(2*PI*1000*t)";
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("aevalsrc={click}|{click}:s=44100:d=40"))
            .arg(&path)
            .status()
            .expect("ffmpeg is installed");
        assert!(status.success());
        let grid = measure(&tools(), &path).await.unwrap().grid.unwrap();
        let [segment] = grid.segments[..] else {
            panic!("{grid:?}");
        };
        assert!((127.9..=128.1).contains(&segment.bpm), "{segment:?}");
        assert!(segment.residual_ms <= 12.0, "{segment:?}");
        assert_eq!(segment.start_ms, 0.0);
    }

    /// Tracks measured before beat grids existed have no row for one, and get measured again.
    #[tokio::test]
    async fn a_track_without_a_beat_grid_row_is_measured_again() {
        let dir = tempfile::tempdir().unwrap();
        make_track(dir.path());
        let pool = crate::db::open_in_memory().await;
        sqlx::query(
            "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'music', 'music', ?);
             INSERT INTO music_tracks (id, library_id, rel_path, size, modified_at,
                 first_seen_generation, seen_generation, tagged, title) VALUES
                 (1, 1, 'tone.wav', 1, 100, 1, 1, 0, 'Tone');",
        )
        .bind(dir.path().to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();
        for table in ["music_sound_bounds", "music_audio_analysis"] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO {table} (track_id, modified_at, analyzer, source, measured_at)
                 VALUES (1, 100, ?, 'server', 0)"
            )))
            .bind(ANALYZER)
            .execute(&pool)
            .await
            .unwrap();
        }
        let report = measure_pending(&pool, &tools(), &Requests::new())
            .await
            .unwrap();
        assert_eq!(report.measured, 1);
        // 7.5 s is too short for a tempo, so there's no grid; three seconds of a bare A read
        // as A minor.
        let row: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT error, segments, key_camelot FROM music_beat_grids WHERE track_id = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, (None, None, Some("8A".to_owned())));
    }
}
