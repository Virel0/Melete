//! Finding intros and credits by matching a season's episodes, and telling openings apart.

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
    for ((file, kind, start, end), (e_file, e_kind, e_start, e_end)) in found.iter().zip(expected) {
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

/// A show whose first two seasons share an opening and an ending, and whose third has a new
/// ending and a new opening that changes again halfway through the season.
async fn three_seasons() -> Season {
    let dir = tempfile::tempdir().unwrap();
    let (first, second, third) = (
        Noise::new(21).tune(40.0),
        Noise::new(22).tune(40.0),
        Noise::new(23).tune(40.0),
    );
    let (old_song, new_song) = (Noise::new(24).tune(30.0), Noise::new(25).tune(30.0));
    // (file id, season, episode, before the intro, its intro, its ending song)
    let layout = [
        (1, 1, 1, 20.0, &first, &old_song),
        (2, 1, 2, 33.3, &first, &old_song),
        (3, 2, 1, 5.0, &first, &old_song),
        (4, 2, 2, 41.7, &first, &old_song),
        (5, 3, 1, 20.0, &second, &new_song),
        (6, 3, 2, 12.4, &second, &new_song),
        (7, 3, 3, 27.0, &third, &new_song),
        (8, 3, 4, 8.0, &third, &new_song),
    ];
    let mut files = Vec::new();
    for (id, season, episode_number, before, intro, song) in layout {
        let name = format!("S{season:02}E{episode_number:02}.wav");
        let mut samples = episode(id, before, intro, 245.0 - before);
        samples.extend_from_slice(song);
        samples.extend(Noise::new(id + 50).tune(45.0));
        write_audio(&dir.path().join(&name), &samples, false);
        files.push(format!(
            "({id}, 1, '{name}', 1, 1, 1, {season}, {episode_number}, 1, 1)"
        ));
    }
    let pool = db::open_in_memory().await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'shows', 'shows', '{}');
         INSERT INTO series (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'Show', 'Show', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, series_id,
             season, episode, first_seen_generation, seen_generation) VALUES {};
         INSERT INTO media_probes (file_id, size, modified_at, probed_at, info)
             SELECT id, 1, 1, 0, '{{\"duration_seconds\": 360.0}}' FROM media_files;",
        dir.path().display(),
        files.join(", ")
    )))
    .execute(&pool)
    .await
    .unwrap();
    Season { pool, _dir: dir }
}

/// Each file's opening or ending id, by file.
async fn themes_of(pool: &SqlitePool, kind: &str) -> Vec<Option<i64>> {
    sqlx::query_scalar(
        "SELECT seg.theme_id FROM media_files f
         LEFT JOIN media_segments seg ON seg.file_id = f.id AND seg.kind = ?
         ORDER BY f.id",
    )
    .bind(kind)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn openings_and_endings_are_told_apart_across_seasons() {
    let s = three_seasons().await;
    // Watched before: the first episode to the end, the fifth only its first seconds.
    let viewer = crate::auth::testing::user(&s.pool, "alice", false).await;
    sqlx::query(
        "INSERT INTO playback_progress (user_id, file_id, position_seconds, watched, updated_at)
         VALUES (?1, 1, 0, 1, 0), (?1, 5, 10, 0, 0)",
    )
    .bind(viewer.id)
    .execute(&s.pool)
    .await
    .unwrap();
    let report = find_pending(&s.pool, Path::new("ffmpeg")).await.unwrap();
    assert_eq!((report.scanned, report.intros, report.credits), (8, 8, 8));
    let intros = themes_of(&s.pool, "intro").await;
    assert!(intros.iter().all(Option::is_some), "{intros:?}");
    let first = intros[0];
    assert_eq!(
        &intros[..4],
        &[first; 4],
        "the same opening in the next season"
    );
    assert_eq!(intros[4], intros[5], "{intros:?}");
    assert_eq!(intros[6], intros[7], "{intros:?}");
    assert_ne!(intros[4], first, "a new season's new opening");
    assert_ne!(intros[6], intros[4], "a change halfway through the season");
    assert_ne!(intros[6], first);
    let credits = themes_of(&s.pool, "credits").await;
    assert!(credits.iter().all(Option::is_some), "{credits:?}");
    assert_eq!(&credits[..4], &[credits[0]; 4]);
    assert_eq!(&credits[4..], &[credits[4]; 4]);
    assert_ne!(credits[4], credits[0], "a new ending");
    let kept: Vec<(String, i64)> =
        sqlx::query_as("SELECT kind, length(fingerprint) FROM segment_themes ORDER BY kind, id")
            .fetch_all(&s.pool)
            .await
            .unwrap();
    assert_eq!(kept.len(), 5, "three openings and two endings: {kept:?}");
    assert!(kept.iter().all(|(_, bytes)| *bytes <= 7500), "{kept:?}");
    // Played past already: seen, in every episode with the same song.
    let new = |file| themes::new_to(&s.pool, file, viewer.id);
    assert_eq!(
        new(4).await.unwrap(),
        (false, false),
        "the first seasons' songs"
    );
    assert_eq!(new(6).await.unwrap(), (true, true), "the third's");

    // A changed file is looked at again, and keeps its opening's id.
    sqlx::raw_sql(
        "UPDATE media_files SET size = 2 WHERE id = 3;
         UPDATE media_probes SET size = 2 WHERE file_id = 3;",
    )
    .execute(&s.pool)
    .await
    .unwrap();
    let again = find_pending(&s.pool, Path::new("ffmpeg")).await.unwrap();
    assert_eq!(again.scanned, 1);
    assert_eq!(themes_of(&s.pool, "intro").await, intros);
}
