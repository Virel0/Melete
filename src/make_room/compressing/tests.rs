use super::*;

/// A four-second H.264 clip at a high bit rate with an audio track and English subtitles.
fn clip(dir: &Path) -> PathBuf {
    let srt = dir.join("en.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:02,000\nHello\n").unwrap();
    let path = dir.join("film.mkv");
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=640x360:r=24:d=4",
        ])
        .args(["-f", "lavfi", "-i", "sine=d=4", "-i"])
        .arg(&srt)
        .args([
            "-map", "0", "-map", "1", "-map", "2", "-c:v", "libx264", "-b:v", "6M",
        ])
        .args(["-c:a", "aac", "-c:s", "srt"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    path
}

#[tokio::test]
async fn an_h264_file_is_compressed_to_a_smaller_hevc_copy_with_its_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let source = clip(dir.path());
    let out = dir.path().join("out.mkv");
    let tools = Tools {
        ffmpeg: Path::new("ffmpeg"),
        ffprobe: Path::new("ffprobe"),
        hw: Hardware::Software,
    };
    compress(&tools, &source, &out).await.unwrap();
    let copy = probe::probe(Path::new("ffprobe"), &out).await.unwrap();
    assert_eq!(copy.video[0].codec, "hevc");
    assert_eq!((copy.audio.len(), copy.subtitles.len()), (1, 1));
    assert!(std::fs::metadata(&out).unwrap().len() < std::fs::metadata(&source).unwrap().len());
}

fn info(seconds: f64, codec: &str, audio: usize) -> MediaInfo {
    serde_json::from_value(serde_json::json!({
        "container": "matroska", "duration_seconds": seconds, "bit_rate": null,
        "video": [{ "index": 0, "codec": codec }],
        "audio": (0..audio).map(|i| serde_json::json!({ "index": i + 1, "codec": "aac", "default": false }))
            .collect::<Vec<_>>(),
        "subtitles": [],
    }))
    .unwrap()
}

#[test]
fn a_copy_takes_the_files_place_only_as_long_with_its_tracks_and_smaller() {
    let before = info(6000.0, "h264", 2);
    assert_eq!(check(&before, &info(6001.0, "hevc", 2), 100, 50), Ok(()));
    for (after, size, why) in [
        (info(5900.0, "hevc", 2), 50, "plays"),
        (info(6000.0, "h264", 2), 50, "isn't HEVC"),
        (info(6000.0, "hevc", 1), 50, "same audio"),
        (info(6000.0, "hevc", 2), 95, "save only"),
    ] {
        let refused = check(&before, &after, 100, size).unwrap_err();
        assert!(refused.contains(why), "{refused}");
    }
}

#[tokio::test]
async fn only_files_that_compress_are_queued() {
    let pool = crate::db::open_in_memory().await;
    sqlx::raw_sql(
        r#"INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'A', 'A', 1), (2, 1, 'B', 'B', 1), (3, 1, 'C', 'C', 1);
         INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation)
             VALUES (1, 1, 'A/a.mkv', 1, 1, 1, 1, 1), (2, 1, 'B/b.mkv', 1, 1, 2, 1, 1),
                 (3, 1, 'C/c.mkv', 1, 1, 3, 1, 1);
         INSERT INTO media_probes (file_id, size, modified_at, probed_at, info) VALUES
             (1, 1, 1, 1, '{"video":[{"codec":"h264"}]}'),
             (2, 1, 1, 1, '{"video":[{"codec":"hevc"}]}'),
             (3, 1, 1, 1, '{"video":[{"codec":"h264","hdr":"Dolby Vision"}]}');"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(start(&pool, false, 1).await.unwrap(), Ok(1));
    assert_eq!(
        start(&pool, false, 1).await.unwrap(),
        Err(NotStarted::NothingToCompress),
        "queued"
    );
    assert_eq!(
        start(&pool, false, 2).await.unwrap(),
        Err(NotStarted::NothingToCompress),
        "HEVC"
    );
    assert_eq!(
        start(&pool, false, 3).await.unwrap(),
        Err(NotStarted::NothingToCompress),
        "DV"
    );
    assert_eq!(
        start(&pool, false, 9).await.unwrap(),
        Err(NotStarted::NotFound)
    );
    let shown = progress(&pool).await.unwrap();
    assert_eq!((shown[0].id, shown[0].waiting), (1, 1));
}
