use super::*;

#[test]
fn the_command_reads_keyframes_only_and_tiles_them() {
    let args: Vec<String> = args(
        Path::new("/m/a film.mkv"),
        WIDTH,
        180,
        Path::new("/t/1-2-3"),
    )
    .into_iter()
    .map(|a| a.into_string().unwrap())
    .collect();
    let at = |flag: &str| args.iter().position(|a| a == flag).unwrap();
    assert!(at("-skip_frame") < at("-i"), "decoding skips non-keyframes");
    assert_eq!(args[at("-skip_frame") + 1], "nokey");
    assert_eq!(args[at("-i") + 1], "file:/m/a film.mkv");
    assert_eq!(
        args[at("-vf") + 1],
        "select='isnan(prev_selected_t)+gte(t-prev_selected_t,10)',scale=320:180,showinfo,tile=10x10"
    );
    assert_eq!(args.last().unwrap(), "/t/1-2-3/%d.jpg");
}

#[test]
fn times_come_from_showinfo() {
    let log = "Input #0, matroska\n\
        [Parsed_showinfo_2 @ 0x55] config in time_base: 1/1000\n\
        [Parsed_showinfo_2 @ 0x55] n:   0 pts:      0 pts_time:0       duration:1 fmt:yuv420p\n\
        [Parsed_showinfo_2 @ 0x55] n:   1 pts:  10010 pts_time:10.01   duration:1 fmt:yuv420p\n\
        [Parsed_showinfo_2 @ 0x55] n:   2 pts:  20021 pts_time:20.021  duration:1 fmt:yuv420p\n\
        frame=    1 fps=0.0 q=5.0 size=N/A time=00:00:20.02\n";
    assert_eq!(logged_times(log), [0.0, 10.01, 20.021]);
}

#[test]
fn pictures_keep_the_videos_shape_in_even_heights() {
    assert_eq!(picture_height(WIDTH, 1920, 1080), 180);
    assert_eq!(picture_height(WIDTH, 1920, 800), 134);
    assert_eq!(picture_height(WIDTH, 720, 576), 256);
    assert_eq!(picture_height(WIDTH, 4000, 10), 2, "never nothing");
}

/// A `seconds`-long clip with a keyframe every 2 seconds.
pub(crate) fn keyframed_clip(dir: &Path, seconds: u32) -> PathBuf {
    let clip = dir.join("keyframed.mkv");
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg(format!("testsrc=size=320x180:rate=24:duration={seconds}"))
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-g", "48"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());
    clip
}

#[tokio::test]
async fn makes_a_picture_every_ten_seconds_from_keyframes() {
    let dir = tempfile::tempdir().unwrap();
    let clip = keyframed_clip(dir.path(), 45);
    let out = dir.path().join("previews");
    let index = make(Path::new("ffmpeg"), &clip, (320, 180), WIDTH, &out)
        .await
        .unwrap();
    assert_eq!(index.times, [0.0, 10.0, 20.0, 30.0, 40.0]);
    assert_eq!((index.width, index.height, index.sheets), (320, 180, 1));
    let sheet = std::fs::read(out.join("0.jpg")).unwrap();
    assert_eq!(&sheet[..3], b"\xff\xd8\xff", "a JPEG");
    let info = crate::probe::probe(Path::new("ffprobe"), &out.join("0.jpg"))
        .await
        .unwrap();
    assert_eq!(
        (info.video[0].width, info.video[0].height),
        (Some(3200), Some(1800)),
        "10 by 10 pictures"
    );
    let stored: Index =
        serde_json::from_slice(&std::fs::read(out.join("index.json")).unwrap()).unwrap();
    assert_eq!(stored, index);

    let junk = dir.path().join("junk.mkv");
    std::fs::write(&junk, b"not a video").unwrap();
    let failed = make(
        Path::new("ffmpeg"),
        &junk,
        (320, 180),
        WIDTH,
        &dir.path().join("j"),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{failed:#}").contains("couldn't read it"),
        "{failed:#}"
    );
}

/// A library with the clip as file 1, probed, and previews turned on.
async fn library(dir: &Path, clip: &Path) -> SqlitePool {
    let pool = crate::db::open_in_memory().await;
    let size = std::fs::metadata(clip).unwrap().len() as i64;
    sqlx::query("INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', ?)")
        .bind(dir.to_string_lossy().into_owned())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'clip', 'Clip', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO media_files (id, library_id, rel_path, size, modified_at, movie_id,
             first_seen_generation, seen_generation) VALUES (1, 1, ?, ?, 7, 1, 1, 1)",
    )
    .bind(clip.file_name().unwrap().to_string_lossy().into_owned())
    .bind(size)
    .execute(&pool)
    .await
    .unwrap();
    crate::probe::probe_pending(&pool, Path::new("ffprobe"))
        .await
        .unwrap();
    pool
}

async fn turn_on(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO settings (name, value, updated_at) VALUES ('MELETE_TRICKPLAY', 'on', 0)",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn files_get_previews_once_while_its_turned_on() {
    let dir = tempfile::tempdir().unwrap();
    let clip = keyframed_clip(dir.path(), 25);
    let pool = library(dir.path(), &clip).await;
    let root = dir.path().join("trickplay");
    let ffmpeg = Path::new("ffmpeg");
    assert_eq!(
        make_pending(&pool, ffmpeg, &root).await.unwrap(),
        Report::default(),
        "off by default"
    );
    assert!(!root.exists());

    turn_on(&pool).await;
    let report = make_pending(&pool, ffmpeg, &root).await.unwrap();
    assert_eq!((report.made, report.failed), (1, 0));
    let index = index_of(&pool, &root, 1).await.unwrap().unwrap();
    assert_eq!(index.times, [0.0, 10.0, 20.0]);
    let size = std::fs::metadata(&clip).unwrap().len() as i64;
    let sheet = sheet_path(&pool, &root, 1, 0).await.unwrap().unwrap();
    assert_eq!(sheet, version_dir(&root, 1, size, 7).join("0.jpg"));
    assert!(sheet.exists());
    assert_eq!(sheet_path(&pool, &root, 1, 1).await.unwrap(), None);
    assert_eq!(
        make_pending(&pool, ffmpeg, &root).await.unwrap(),
        Report::default(),
        "once per version"
    );

    // A changed file has none until they're made again; the old ones are cleaned up.
    sqlx::query("UPDATE media_files SET modified_at = 8 WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(index_of(&pool, &root, 1).await.unwrap(), None);
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::create_dir_all(root.join(".making-1-2-3")).unwrap();
    assert_eq!(
        clean(&pool, &root).await.unwrap(),
        1,
        "the old version only"
    );
    assert!(root.join("notes").exists() && root.join(".making-1-2-3").exists());
}

#[tokio::test]
async fn a_file_ffmpeg_cant_read_isnt_tried_again_until_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let clip = keyframed_clip(dir.path(), 5);
    let pool = library(dir.path(), &clip).await;
    turn_on(&pool).await;
    // Spoiled after it was probed.
    std::fs::write(
        &clip,
        b"x".repeat(std::fs::metadata(&clip).unwrap().len() as usize),
    )
    .unwrap();
    let root = dir.path().join("trickplay");
    let report = make_pending(&pool, Path::new("ffmpeg"), &root)
        .await
        .unwrap();
    assert_eq!((report.made, report.failed), (0, 1));
    let error: Option<String> = sqlx::query_scalar("SELECT error FROM media_trickplay")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(error.unwrap().contains("couldn't read it"));
    assert_eq!(
        make_pending(&pool, Path::new("ffmpeg"), &root)
            .await
            .unwrap(),
        Report::default()
    );
    assert_eq!(index_of(&pool, &root, 1).await.unwrap(), None);
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        0,
        "nothing half-made left"
    );
}

/// Chooses the previews' width as Settings does.
async fn choose_width(pool: &SqlitePool, width: &str) -> Result<(), crate::settings::Refused> {
    let changes = [(WIDTH_SETTING.to_owned(), Some(width.to_owned()))].into();
    let none = |_: &str| None;
    crate::settings::save(pool, "trickplay", &changes, &none)
        .await
        .unwrap()
        .map(|_| ())
}

/// The width and height of a picture file, as ffprobe reads it.
async fn picture_size(path: &Path) -> (Option<i64>, Option<i64>) {
    let info = crate::probe::probe(Path::new("ffprobe"), path)
        .await
        .unwrap();
    (info.video[0].width, info.video[0].height)
}

#[tokio::test]
async fn a_new_width_makes_the_previews_again_and_the_old_ones_serve_until_then() {
    let dir = tempfile::tempdir().unwrap();
    let clip = keyframed_clip(dir.path(), 25);
    let pool = library(dir.path(), &clip).await;
    let root = dir.path().join("trickplay");
    let ffmpeg = Path::new("ffmpeg");
    turn_on(&pool).await;
    assert_eq!(make_pending(&pool, ffmpeg, &root).await.unwrap().made, 1);
    assert_eq!(index_of(&pool, &root, 1).await.unwrap().unwrap().width, 320);

    assert!(matches!(
        choose_width(&pool, "500").await,
        Err(crate::settings::Refused::Invalid(_))
    ));
    choose_width(&pool, "640").await.unwrap();
    assert_eq!(super::super::width(&pool).await.unwrap(), 640);
    let old = index_of(&pool, &root, 1).await.unwrap().unwrap();
    assert_eq!((old.width, old.height), (320, 180), "served until replaced");

    let report = make_pending(&pool, ffmpeg, &root).await.unwrap();
    assert_eq!((report.made, report.failed), (1, 0), "made again");
    let index = index_of(&pool, &root, 1).await.unwrap().unwrap();
    assert_eq!((index.width, index.height), (640, 360));
    assert_eq!(index.times, [0.0, 10.0, 20.0]);
    let sheet = sheet_path(&pool, &root, 1, 0).await.unwrap().unwrap();
    assert_eq!(picture_size(&sheet).await, (Some(6400), Some(3600)));
    assert_eq!(
        make_pending(&pool, ffmpeg, &root).await.unwrap(),
        Report::default(),
        "once per width"
    );
}

#[tokio::test]
async fn previews_that_cant_be_made_at_a_new_width_keep_the_old_ones() {
    let dir = tempfile::tempdir().unwrap();
    let clip = keyframed_clip(dir.path(), 5);
    let pool = library(dir.path(), &clip).await;
    let root = dir.path().join("trickplay");
    let ffmpeg = Path::new("ffmpeg");
    turn_on(&pool).await;
    assert_eq!(make_pending(&pool, ffmpeg, &root).await.unwrap().made, 1);
    // Spoiled after its previews were made, as it was probed.
    std::fs::write(
        &clip,
        b"x".repeat(std::fs::metadata(&clip).unwrap().len() as usize),
    )
    .unwrap();
    choose_width(&pool, "480").await.unwrap();
    let report = make_pending(&pool, ffmpeg, &root).await.unwrap();
    assert_eq!((report.made, report.failed), (0, 1));
    let kept = index_of(&pool, &root, 1).await.unwrap().unwrap();
    assert_eq!(kept.width, 320, "the old previews stay");
    assert_eq!(
        make_pending(&pool, ffmpeg, &root).await.unwrap(),
        Report::default(),
        "not tried at that width again"
    );
}
