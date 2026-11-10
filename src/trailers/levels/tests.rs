use super::*;
use crate::db;

/// A real first pass's block, as ffmpeg prints it mid-log, numbers as quoted strings.
const LOG: &str = "[Parsed_loudnorm_0 @ 0x5581]
{
\t\"input_i\" : \"-12.31\",
\t\"input_tp\" : \"-0.42\",
\t\"input_lra\" : \"5.10\",
\t\"input_thresh\" : \"-22.47\",
\t\"output_i\" : \"-24.02\",
\t\"output_tp\" : \"-11.63\",
\t\"output_lra\" : \"4.90\",
\t\"output_thresh\" : \"-34.13\",
\t\"normalization_type\" : \"dynamic\",
\t\"target_offset\" : \"0.02\"
}
[out#0/null @ 0x5582] video:0KiB audio:938KiB";

#[test]
fn reads_every_value_the_second_pass_needs() {
    assert_eq!(
        parse(LOG),
        Some(Measured {
            i: -12.31,
            tp: -0.42,
            lra: 5.1,
            thresh: -22.47,
            offset: 0.02,
        })
    );
    assert_eq!(parse("no block here"), None);
}

/// A four-second clip at `path`: a test pattern, and a tone `volume` loud (none without).
fn clip(path: &Path, volume: Option<&str>) {
    let mut ffmpeg = std::process::Command::new("ffmpeg");
    ffmpeg.args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi"]);
    ffmpeg.args(["-i", "testsrc2=s=64x36:r=24:d=4"]);
    if let Some(volume) = volume {
        ffmpeg.args(["-f", "lavfi", "-i", "sine=f=440:d=4"]);
        ffmpeg.args(["-af", &format!("volume={volume}"), "-c:a", "aac"]);
    }
    let made = ffmpeg
        .args(["-pix_fmt", "yuv420p", "-shortest", "-f", "mp4"])
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success());
}

/// The clip's loudness, measured as levelling does.
async fn loudness(path: &Path) -> f64 {
    let out = tokio::process::Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-i"])
        .arg(path)
        .args(["-vn", "-af", &format!("{}:print_format=json", filter())])
        .args(["-f", "null", "-"])
        .output()
        .await
        .unwrap();
    parse(&String::from_utf8_lossy(&out.stderr)).unwrap().i
}

fn video_codec(path: &Path) -> String {
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["stream=codec_name", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[tokio::test]
async fn a_loud_trailer_is_brought_to_the_target_its_picture_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let loud = dir.path().join("movie-1.mp4");
    clip(&loud, Some("2.0"));
    let before = loudness(&loud).await;
    assert!(before > TARGET_LUFS + 5.0, "loud to begin with: {before}");
    let codec = video_codec(&loud);

    assert_eq!(level(Path::new("ffmpeg"), &loud).await, Ok(Levelled::Done));
    let after = loudness(&loud).await;
    assert!(
        (after - TARGET_LUFS).abs() <= 1.5,
        "{before} LUFS became {after}"
    );
    assert_eq!(video_codec(&loud), codec, "the picture copied as it was");
    assert!(
        !dir.path().join(".movie-1.mp4.levelling.mp4").exists(),
        "nothing left beside it"
    );
    // Once there, it's left as it is.
    assert!(matches!(
        level(Path::new("ffmpeg"), &loud).await,
        Ok(Levelled::Left(_))
    ));
}

#[tokio::test]
async fn one_with_no_sound_is_left_and_one_ffmpeg_cant_read_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mute = dir.path().join("movie-2.mp4");
    clip(&mute, None);
    let bytes = std::fs::read(&mute).unwrap();
    assert!(matches!(
        level(Path::new("ffmpeg"), &mute).await,
        Ok(Levelled::Left(_))
    ));
    assert_eq!(std::fs::read(&mute).unwrap(), bytes, "untouched");
    let broken = dir.path().join("movie-3.mp4");
    std::fs::write(&broken, b"not a video").unwrap();
    assert!(level(Path::new("ffmpeg"), &broken).await.is_err());
    assert_eq!(std::fs::read(&broken).unwrap(), b"not a video");
}

#[tokio::test]
async fn downloaded_trailers_are_levelled_once_each_and_again_when_downloaded_again() {
    let pool = db::open_in_memory().await;
    let scratch = tempfile::tempdir().unwrap();
    let folders = Folders {
        jobs: scratch.path().join("jobs"),
        videos: scratch.path().join("trailers"),
    };
    std::fs::create_dir_all(&folders.videos).unwrap();
    clip(&folders.videos.join("movie-1.mp4"), Some("2.0"));
    sqlx::raw_sql(
        "INSERT INTO libraries (id, name, kind, path) VALUES (1, 'movies', 'movies', '/m');
         INSERT INTO movies (id, library_id, rel_path, parsed_title, seen_generation)
             VALUES (1, 1, 'One', 'One', 1);
         INSERT INTO downloaded_trailers (id, movie_id, video, max_height, state, file, size,
             modified_at, queued_at, finished_at)
             VALUES (4, 1, 'aaaaaaaaaaa', 2160, 'done', 'movie-1.mp4', 1, 1, 0, 0);",
    )
    .execute(&pool)
    .await
    .unwrap();
    let ffmpeg = Path::new("ffmpeg");
    assert_eq!(level_some(&pool, &folders, ffmpeg).await.unwrap(), 1);
    let (note, size): (Option<String>, i64) =
        sqlx::query_as("SELECT level_note, size FROM downloaded_trailers WHERE id = 4")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(note.as_deref(), Some("levelled to -24 LUFS"));
    let on_disk = std::fs::metadata(folders.videos.join("movie-1.mp4"))
        .unwrap()
        .len();
    assert_eq!(
        size, on_disk as i64,
        "its size as it is now, so it's probed again"
    );
    assert_eq!(
        level_some(&pool, &folders, ffmpeg).await.unwrap(),
        0,
        "once"
    );
    // Downloaded again: levelled again.
    sqlx::query("UPDATE downloaded_trailers SET levelled_at = NULL, level_note = NULL")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(level_some(&pool, &folders, ffmpeg).await.unwrap(), 1);
}
