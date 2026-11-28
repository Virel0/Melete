use super::*;
use crate::probe::VideoStream;

fn info(height: i64, pixel_format: &str, hdr: Option<&str>) -> MediaInfo {
    MediaInfo {
        container: Some("matroska,webm".into()),
        duration_seconds: Some(5400.0),
        bit_rate: None,
        video: vec![VideoStream {
            index: 0,
            codec: "hevc".into(),
            profile: None,
            width: Some(height * 16 / 9),
            height: Some(height),
            pixel_format: Some(pixel_format.into()),
            frame_rate: Some(23.976),
            hdr: hdr.map(Into::into),
            dolby_vision: None,
            parameter_sets_in_stream_only: false,
        }],
        audio: Vec::new(),
        subtitles: Vec::new(),
    }
}

fn plan_of(info: &MediaInfo, codec: VideoCodec, hw: Hardware) -> Plan {
    let settings = Settings {
        video_codec: Some(codec),
        ..Settings::default()
    };
    plan("/path/to/film.mkv".into(), info, &settings, hw).unwrap()
}

fn args(plan: &Plan, hw: Hardware) -> Vec<String> {
    ffmpeg_args(plan, hw, 0, Path::new("/path/to/run"))
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_run_is_judged_after_thirty_seconds_against_one_point_three_times_real_time() {
    let secs = Duration::from_secs;
    assert_eq!(too_slow(10.0, secs(29)), None, "not judged yet");
    assert_eq!(too_slow(30.0, secs(30)), Some(1.0));
    assert_eq!(
        too_slow(38.0, secs(30)).map(|s| (s * 100.0).round()),
        Some(127.0)
    );
    assert_eq!(too_slow(40.0, secs(30)), None, "1.33x keeps up");
    assert_eq!(too_slow(600.0, secs(120)), None);
}

#[test]
fn a_slow_conversion_steps_down_one_rung_at_a_time() {
    assert_eq!(rung_below(2160), Some(1440));
    assert_eq!(rung_below(1440), Some(1080));
    assert_eq!(rung_below(1081), Some(1080));
    assert_eq!(rung_below(1080), Some(720));
    assert_eq!(rung_below(720), Some(480));
    assert_eq!(rung_below(480), None);

    let film = info(1080, "yuv420p", None);
    let plan = plan_of(&film, VideoCodec::H264, Hardware::Nvenc);
    assert_eq!(plan.stepped_down(0), Some(plan.clone()));
    let one = plan.stepped_down(1).unwrap();
    assert_eq!((one.height, one.bitrate), (Some(720), 5_000_000));
    let two = plan.stepped_down(2).unwrap();
    assert_eq!((two.height, two.bitrate), (Some(480), 2_500_000));
    assert_eq!(plan.stepped_down(3), None, "no rung that low");
    assert!(
        args(&one, Hardware::Nvenc)
            .iter()
            .any(|a| a.contains("scale_cuda=w=-2:h=720"))
    );
    assert!(
        args(&two, Hardware::Software)
            .iter()
            .any(|a| a.contains("scale=-2:480"))
    );
    let copy = plan_of(&film, VideoCodec::Copy, Hardware::Nvenc);
    assert_eq!(copy.stepped_down(1), None);
}
