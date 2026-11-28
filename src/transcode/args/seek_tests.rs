//! Copied video started mid-file: its first segment's audio starts with its picture.

use super::*;

/// A minute of H.264 with B-frames and keyframes 5 s apart, and AC-3 audio, which a player
/// asking for AAC has converted.
fn sparse_keyframes(dir: &Path) -> PathBuf {
    let clip = dir.join("sparse.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc=size=320x240:rate=24:duration=60")
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=60"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-g", "120"])
        .args(["-keyint_min", "120", "-sc_threshold", "0", "-bf", "3"])
        .args(["-c:a", "ac3"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    clip
}

/// Each stream's start in a segment, by ffprobe.
fn starts(segment: &Path) -> (f64, f64) {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,start_time",
        ])
        .args(["-of", "csv=p=0"])
        .arg(segment)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let start = |kind: &str| -> f64 {
        text.lines()
            .find_map(|l| l.strip_prefix(kind)?.strip_prefix(',')?.parse().ok())
            .unwrap_or_else(|| panic!("no {kind} in {text}"))
    };
    (start("video"), start("audio"))
}

#[tokio::test]
async fn a_copy_started_mid_file_has_audio_from_its_first_frame() {
    let dir = tempfile::tempdir().unwrap();
    let clip = sparse_keyframes(dir.path());
    let ffprobe = Path::new("ffprobe");
    let info = crate::probe::probe(ffprobe, &clip).await.unwrap();
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    let mut p = plan(clip.clone(), &info, &copy, Hardware::Software).unwrap();
    assert_eq!(p.audio_codec, AudioCodec::Aac, "the audio is converted");
    let found = keyframes(ffprobe, &clip, p.video, p.duration)
        .await
        .unwrap();
    assert!(found.starts.len() > 4, "{found:?}");
    p.keyframes = Some(Arc::new(found));
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    for n in [2, 3] {
        let segment = t
            .segment(format!("at {n}").as_bytes(), &p, n)
            .await
            .unwrap();
        let (video, audio) = starts(&segment);
        assert!(
            audio <= video + 0.001,
            "segment {n}: audio starts {:.3} s after the picture",
            audio - video
        );
        assert!(
            audio > video - 0.25,
            "segment {n}: audio starts {:.3} s before the picture",
            video - audio
        );
    }
}
