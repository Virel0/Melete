use super::testing::ffmpeg_without_gpu;
use super::*;

fn on_gpu(clip: &Path) -> Plan {
    Plan {
        input: clip.into(),
        duration: 20.0,
        video: 0,
        gpu_decode: true,
        height: None,
        tonemap: false,
        high_bit_depth: false,
        codec: VideoCodec::H264,
        ten_bit: false,
        fmp4: false,
        dolby_vision_only: false,
        dolby_vision: false,
        keyframes: None,
        bitrate: 500_000,
        audio: Some(1),
        audio_codec: AudioCodec::Aac,
        channels: 2,
        burn: None,
        audio_filter: None,
    }
}

fn run_of(t: &Transcoder, session: &[u8]) -> (Hardware, bool) {
    let running = t.running.lock().unwrap();
    let r = &running[session];
    (r.hw, r.on_cpu)
}

#[tokio::test]
async fn a_run_that_fails_on_the_gpu_plays_on_the_cpu() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 60);
    let (_bin, ffmpeg) = ffmpeg_without_gpu();
    let t = Transcoder::new(ffmpeg, Hardware::Nvenc, dir.path().join("t"), 4);
    assert_eq!(t.hardware(), Hardware::Nvenc, "trusted until a check fails");
    let plan = Plan {
        duration: 60.0,
        ..on_gpu(&clip)
    };

    let first = t.segment(b"one", &plan, 0).await.unwrap();
    let info = crate::probe::probe(Path::new("ffprobe"), &first)
        .await
        .unwrap();
    assert_eq!(info.video[0].codec, "h264");
    assert_eq!(run_of(&t, b"one"), (Hardware::Software, true));
    assert!(t.gpu.lock().unwrap().asked, "the GPU is to be checked");

    // A far seek restarts the session's run, still on the CPU.
    t.segment(b"one", &plan, 9).await.unwrap();
    assert_eq!(t.running_start(b"one"), Some(9));
    assert_eq!(run_of(&t, b"one"), (Hardware::Software, true));
}

#[tokio::test]
async fn a_failing_check_sends_new_runs_to_the_cpu_until_one_passes() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 20);
    let (_bin, ffmpeg) = ffmpeg_without_gpu();
    let t = Transcoder::new(ffmpeg.clone(), Hardware::Nvenc, dir.path().join("t"), 4);

    t.check_gpu().await;
    let problem = t.gpu_problem().expect("the test encode failed");
    assert!(problem.contains("Cannot load libcuda.so.1"), "{problem}");
    assert_eq!(t.hardware(), Hardware::Software);
    t.segment(b"one", &on_gpu(&clip), 0).await.unwrap();
    assert_eq!(
        run_of(&t, b"one"),
        (Hardware::Software, false),
        "started on the CPU, not retried there"
    );

    std::fs::write(ffmpeg.with_extension("ok"), "").unwrap();
    t.check_gpu().await;
    assert_eq!(t.gpu_problem(), None);
    assert_eq!(t.hardware(), Hardware::Nvenc);
}

#[tokio::test]
async fn the_cpu_alone_is_never_checked() {
    let dir = tempfile::tempdir().unwrap();
    let (_bin, ffmpeg) = ffmpeg_without_gpu();
    let t = Transcoder::new(ffmpeg, Hardware::Software, dir.path().join("t"), 4);
    t.check_gpu_if_due();
    t.ask_gpu_check();
    {
        let gpu = t.gpu.lock().unwrap();
        assert!(gpu.checked.is_none() && !gpu.asked);
    }
    t.check_gpu().await;
    assert_eq!(t.gpu_problem(), None, "no GPU, so no GPU problem");
    assert_eq!(t.hardware(), Hardware::Software);
}

#[test]
fn checks_are_due_at_first_when_asked_and_every_few_minutes_while_failing() {
    let mut state = GpuState::default();
    assert!(state.due(), "never checked");
    state.checked = Some(Instant::now());
    assert!(!state.due(), "working, checked just now");
    state.failed = Some("no GPU".into());
    assert!(!state.due(), "failing, checked just now");
    state.checked = Some(Instant::now() - GPU_RECHECK);
    assert!(state.due(), "failing for long enough");
    state.failed = None;
    assert!(!state.due(), "working: no need");
    state.asked = true;
    assert!(state.due(), "a run failed on the GPU");
    state.checking = true;
    assert!(!state.due(), "one at a time");
}

#[test]
fn the_check_runs_the_gpu_filters_and_nvenc() {
    let args: Vec<String> = gpu_check_args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let filters = &args[args.iter().position(|a| a == "-vf").unwrap() + 1];
    for part in ["hwupload_cuda", "tonemap_cuda", "scale_cuda"] {
        assert!(filters.contains(part), "{filters}");
    }
    assert!(args.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
    assert!(args.windows(2).any(|w| w == ["-f", "lavfi"]));
    assert!(args.ends_with(&["-f".into(), "null".into(), "-".into()]));
}

#[test]
fn the_reason_is_what_ffmpeg_said_first() {
    let log = "[AVFilterGraph @ 0x5fa] No such filter: 'tonemap_cuda'\nError opening output file -.\nError opening output files: Filter not found\nmore\n";
    assert_eq!(
        reason(log),
        "No such filter: 'tonemap_cuda'; Error opening output file -.; Error opening output files: Filter not found"
    );
    assert_eq!(reason("\n  \n"), "ffmpeg failed without saying why");
    assert_eq!(reason(&"x".repeat(1000)).len(), REASON_CHARS);
}
