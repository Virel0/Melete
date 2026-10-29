use super::*;
use crate::transcode::{Hardware, Settings, Transcoder};

fn plan_of(codec: VideoCodec, bitrate: i64, channels: i64) -> Plan {
    Plan {
        input: "/m/film.mkv".into(),
        duration: 3600.0,
        video: 0,
        gpu_decode: true,
        height: Some(720),
        tonemap: false,
        high_bit_depth: false,
        codec,
        ten_bit: false,
        fmp4: false,
        dolby_vision_only: false,
        keyframes: None,
        bitrate,
        audio: Some(1),
        channels,
        burn: None,
        audio_filter: None,
    }
}

#[test]
fn a_copy_is_about_its_bitrate_over_its_length() {
    let h264 = plan_of(VideoCodec::H264, 5_000_000, 2);
    assert_eq!(estimate(&h264, Some(40_000_000)), (5_192_000 * 3600) / 8);
    let surround = plan_of(VideoCodec::H264, 5_000_000, 6);
    assert_eq!(estimate(&surround, None), (5_384_000 * 3600) / 8);
    let copied = plan_of(VideoCodec::Copy, 5_000_000, 2);
    assert_eq!(
        estimate(&copied, Some(8_000_000)),
        (8_192_000 * 3600) / 8,
        "copied video keeps the file's bitrate"
    );
}

fn strings(args: Vec<std::ffi::OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.into_string().unwrap()).collect()
}

#[test]
fn one_mp4_with_its_index_at_the_front_and_progress_said() {
    let out = Path::new("/t/offline/7.mp4.part");
    let plan = plan_of(VideoCodec::H264, 5_000_000, 2);
    let a = strings(crate::transcode::whole_file_args(
        &plan,
        Hardware::Nvenc,
        Default::default(),
        out,
    ));
    let has = |pair: [&str; 2]| a.windows(2).any(|w| w == pair);
    assert!(has(["-progress", "pipe:1"]) && has(["-hwaccel", "cuda"]));
    assert!(has(["-c:v", "h264_nvenc"]) && has(["-c:a", "aac"]));
    assert!(has(["-movflags", "+faststart"]) && has(["-f", "mp4"]));
    assert_eq!(a.last().unwrap(), "/t/offline/7.mp4.part");
    let cpu = strings(crate::transcode::whole_file_args(
        &plan,
        Hardware::Software,
        Default::default(),
        out,
    ));
    assert!(!cpu.contains(&"-hwaccel".to_string()) && cpu.contains(&"libx264".to_string()));
    let copied = strings(crate::transcode::whole_file_args(
        &plan_of(VideoCodec::Copy, 0, 2),
        Hardware::Nvenc,
        Default::default(),
        out,
    ));
    assert!(copied.windows(2).any(|w| w == ["-c:v", "copy"]));
    assert!(
        !copied.contains(&"-hwaccel".to_string()),
        "nothing to decode"
    );
    assert_eq!(
        crate::transcode::progress_of("out_time_us=900000000", 3600.0),
        Some(0.25)
    );
    assert_eq!(crate::transcode::progress_of("speed=2x", 3600.0), None);
}

/// A library with file 1, a 20-second clip with a subtitle file beside it, probed, and user 1.
pub(crate) async fn library(dir: &Path) -> SqlitePool {
    let library = dir.join("library");
    std::fs::create_dir_all(&library).unwrap();
    let clip = crate::probe::testing::make_long_clip(dir, 20);
    std::fs::rename(&clip, library.join("clip.mkv")).unwrap();
    std::fs::write(
        library.join("clip.en.srt"),
        "1\n00:00:01,000 --> 00:00:05,000\nKept beside it\n",
    )
    .unwrap();
    let size = std::fs::metadata(library.join("clip.mkv")).unwrap().len() as i64;
    let pool = crate::db::open_in_memory().await;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)")
        .bind(library.to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'clip.mkv', 'Clip', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, 'clip.mkv', ?, 1, 1, 1, 1)",
    )
    .bind(size)
    .execute(&pool)
    .await
    .unwrap();
    crate::probe::probe_pending(&pool, Path::new("ffprobe"))
        .await
        .unwrap();
    crate::auth::create_user(&pool, "alice", "alice-password".into(), false)
        .await
        .unwrap()
        .unwrap();
    pool
}

pub(crate) async fn turn_on(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_OFFLINE_COPIES', 'on', 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

pub(crate) fn tools(dir: &Path, ffmpeg: PathBuf, hw: Hardware) -> Tools {
    Tools {
        transcoder: Transcoder::new(ffmpeg, hw, dir.join("transcode"), 4),
        root: folder(&dir.join("transcode")),
        subtitle_cache: dir.join("subtitles"),
    }
}

/// The id of the subtitle file beside the clip.
fn sidecar(dir: &Path) -> String {
    let found = crate::subtitles::sidecars(&dir.join("library/clip.mkv")).unwrap();
    format!("x{}", found[0].id)
}

#[tokio::test]
async fn a_copy_is_made_into_the_transcode_folder_with_its_subtitles() {
    let dir = tempfile::tempdir().unwrap();
    let pool = library(dir.path()).await;
    let tools = tools(dir.path(), "ffmpeg".into(), Hardware::Software);
    let settings = serde_json::to_string(&Settings {
        max_height: Some(180),
        ..Settings::default()
    })
    .unwrap();
    let track = sidecar(dir.path());
    let id = create(&pool, 1, 1, &settings, std::slice::from_ref(&track), 1)
        .await
        .unwrap();
    assert_eq!(
        make_next(&pool, &tools).await.unwrap(),
        None,
        "off by default"
    );

    turn_on(&pool).await;
    assert_eq!(make_next(&pool, &tools).await.unwrap(), Some(id));
    let copy = get(&pool, 1, id).await.unwrap().unwrap();
    assert_eq!(
        (copy.status.as_str(), copy.progress),
        ("ready", 1.0),
        "{copy:?}"
    );
    let video = video_path(&tools.root, id);
    assert_eq!(
        copy.bytes,
        Some(std::fs::metadata(&video).unwrap().len() as i64)
    );
    assert_eq!(copy.delete_at, Some(copy.ready_at.unwrap() + KEPT));
    let info = crate::probe::probe(Path::new("ffprobe"), &video)
        .await
        .unwrap();
    assert!(info.container.unwrap().contains("mp4"));
    assert_eq!(
        (info.video[0].codec.as_str(), info.video[0].height),
        ("h264", Some(180))
    );
    assert_eq!(info.audio[0].codec, "aac");
    let bytes = std::fs::read(&video).unwrap();
    let at = |atom: &[u8]| bytes.windows(4).position(|w| w == atom).unwrap();
    assert!(
        at(b"moov") < at(b"mdat"),
        "its index first, so it plays as it downloads"
    );
    let vtt = std::fs::read_to_string(subtitle_path(&tools.root, id, &track)).unwrap();
    assert!(vtt.contains("Kept beside it"), "{vtt}");
    assert!(
        dir.path().join("library").read_dir().unwrap().count() == 2,
        "nothing in the library"
    );
}

#[tokio::test]
async fn a_copy_that_fails_on_the_gpu_is_made_on_the_cpu() {
    let dir = tempfile::tempdir().unwrap();
    let pool = library(dir.path()).await;
    turn_on(&pool).await;
    let (_bin, ffmpeg) = crate::transcode::testing::ffmpeg_without_gpu();
    let tools = tools(dir.path(), ffmpeg, Hardware::Nvenc);
    let id = create(&pool, 1, 1, "{}", &[], 1).await.unwrap();
    make_next(&pool, &tools).await.unwrap();
    let copy = get(&pool, 1, id).await.unwrap().unwrap();
    assert_eq!(copy.status, "ready", "{copy:?}");
}

#[tokio::test]
async fn copies_wait_while_live_conversions_take_over_half_the_slots() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 60);
    let transcoder = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let plan = crate::transcode::plan(
        clip.clone(),
        &crate::probe::probe(Path::new("ffprobe"), &clip)
            .await
            .unwrap(),
        &Settings::default(),
        Hardware::Software,
    )
    .unwrap();
    for session in [b"one".as_slice(), b"two"] {
        transcoder.segment(session, &plan, 0).await.unwrap();
    }
    assert!(room_for_one(&transcoder), "two of four is half");
    transcoder.segment(b"three", &plan, 0).await.unwrap();
    assert!(!room_for_one(&transcoder), "three of four is more");
}

#[tokio::test]
async fn copies_are_deleted_when_their_time_is_up() {
    let dir = tempfile::tempdir().unwrap();
    let pool = library(dir.path()).await;
    let root = dir.path().join("offline");
    std::fs::create_dir_all(&root).unwrap();
    let now = unix_now();
    for (id, status, created, ready, downloaded) in [
        (1, "ready", now, Some(now - KEPT - 5), Some(now - KEPT - 1)),
        (2, "ready", now, Some(now - KEPT - 5), Some(now - 60)),
        (3, "ready", now, Some(now - KEPT - 1), None),
        (4, "failed", now - FAILED_KEPT - 1, None, None),
        (5, "queued", now - 30 * 86400, None, None),
    ] {
        sqlx::query(
            "INSERT INTO offline_copies (id, user_id, file_id, settings, subtitles, status,
                 estimated_bytes, created_at, ready_at, last_download_at)
             VALUES (?, 1, 1, '{}', '[\"s2\"]', ?, 1, ?, ?, ?)",
        )
        .bind(id)
        .bind(status)
        .bind(created)
        .bind(ready)
        .bind(downloaded)
        .execute(&pool)
        .await
        .unwrap();
        std::fs::write(video_path(&root, id), b"mp4").unwrap();
        std::fs::write(subtitle_path(&root, id, "s2"), b"vtt").unwrap();
    }
    assert_eq!(sweep(&pool, &root).await.unwrap(), 3);
    let left: Vec<i64> = of_user(&pool, 1, None)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(left, [5, 2], "downloaded lately, and one waiting");
    assert!(!video_path(&root, 1).exists() && !subtitle_path(&root, 1, "s2").exists());
    assert!(video_path(&root, 2).exists());
}
