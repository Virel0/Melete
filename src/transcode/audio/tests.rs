use super::*;

#[test]
fn surround_sound_downmixed_to_stereo_keeps_the_centre_up() {
    let down = "aresample=ochl=stereo:clev=1:slev=0.707:lfe_mix_level=0.316:rematrix_maxval=1";
    assert_eq!(audio_filter(AudioMode::Normal, 6, 2).as_deref(), Some(down));
    assert_eq!(audio_filter(AudioMode::Normal, 8, 2).as_deref(), Some(down));
    assert_eq!(
        audio_filter(AudioMode::Normal, 2, 2),
        None,
        "stereo as it is"
    );
    assert_eq!(audio_filter(AudioMode::Normal, 6, 6), None, "5.1 kept");
    assert_eq!(
        audio_filter(AudioMode::Normal, 6, 1),
        None,
        "mono: ffmpeg's"
    );
    assert_eq!(
        audio_filter(AudioMode::Dialogue, 6, 2),
        Some(format!("{down},{DIALOGUE}"))
    );
    assert_eq!(
        audio_filter(AudioMode::Dialogue, 2, 2).as_deref(),
        Some(DIALOGUE)
    );
    assert_eq!(
        audio_filter(AudioMode::Levelled, 6, 6).as_deref(),
        Some(LEVELLED)
    );
}

fn surround_info() -> MediaInfo {
    let audio = |index: i64, channels: i64, default: bool| crate::probe::AudioStream {
        index,
        codec: "eac3".into(),
        profile: None,
        channels: Some(channels),
        channel_layout: None,
        language: None,
        title: None,
        default,
    };
    MediaInfo {
        container: Some("matroska,webm".into()),
        duration_seconds: Some(60.0),
        bit_rate: None,
        video: vec![crate::probe::VideoStream {
            index: 0,
            codec: "h264".into(),
            profile: None,
            width: Some(1920),
            height: Some(1080),
            pixel_format: Some("yuv420p".into()),
            frame_rate: Some(24.0),
            hdr: None,
            dolby_vision: None,
        }],
        audio: vec![audio(1, 6, true), audio(2, 2, false)],
        subtitles: Vec::new(),
    }
}

fn args_of(plan: &Plan) -> Vec<String> {
    ffmpeg_args(plan, Hardware::Software, 0, Path::new("/t"))
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect()
}

#[test]
fn plans_filter_the_audio_they_convert() {
    let info = surround_info();
    let plan_with =
        |settings: Settings| plan("/m.mkv".into(), &info, &settings, Hardware::Software).unwrap();
    let surround = plan_with(Settings::default());
    assert!(
        surround
            .audio_filter
            .as_deref()
            .unwrap()
            .starts_with("aresample=ochl=stereo")
    );
    let a = args_of(&surround);
    let af = a.iter().position(|x| x == "-af").expect("-af");
    assert!(af < a.iter().position(|x| x == "-c:a").unwrap());

    let stereo = plan_with(Settings {
        audio_stream: Some(2),
        ..Settings::default()
    });
    assert_eq!(stereo.audio_filter, None);
    assert!(!args_of(&stereo).contains(&"-af".to_string()));

    // Copying the video converts the audio, so it can be filtered too.
    let copy = plan_with(Settings {
        video_codec: Some(VideoCodec::Copy),
        audio_mode: Some(AudioMode::Dialogue),
        audio_channels: Some(6),
        ..Settings::default()
    });
    assert_eq!(copy.audio_filter.as_deref(), Some(DIALOGUE));
    let a = copy_args(&copy, 0, Path::new("/t"));
    assert!(a.windows(2).any(|w| w[0] == "-af" && w[1] == DIALOGUE));
}

#[test]
fn audio_modes_are_named_in_lower_case() {
    let settings: Settings = serde_json::from_str(r#"{"audio_mode": "levelled"}"#).unwrap();
    assert_eq!(settings.audio_mode, Some(AudioMode::Levelled));
    assert!(serde_json::from_str::<Settings>(r#"{"audio_mode": "loud"}"#).is_err());
}

/// A 6-second 5.1 clip with a 1 kHz tone in one channel at a time, each for 0.8 s with 0.4 s
/// of silence between: the centre, front left and LFE at the same level, then the centre 20 dB
/// quieter (a whisper after a shout).
fn tone_clip(dir: &Path) -> PathBuf {
    let clip = dir.join("surround.mkv");
    // Commas in a filter's option are escaped.
    let tone = |level: f64, from: f64| {
        format!(
            "{level}*sin(2*PI*1000*t)*between(t\\,{from}\\,{})",
            from + 0.8
        )
    };
    let channels = [
        tone(0.5, 1.6),                                    // FL
        "0".to_owned(),                                    // FR
        format!("{}+{}", tone(0.5, 0.4), tone(0.05, 4.0)), // FC
        tone(0.5, 2.8),                                    // LFE
        "0".to_owned(),                                    // BL
        "0".to_owned(),                                    // BR
    ];
    let audio = format!("aevalsrc=exprs={}:c=5.1:s=48000:d=6", channels.join("|"));
    let made = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=160x90:rate=24:duration=6")
        .args(["-f", "lavfi", "-i", &audio])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-c:a", "flac"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());
    clip
}

/// The mean volume, in dB, of the left channel from `from` to `to` seconds into the audio.
fn left_volume(file: &Path, from: f64, to: f64) -> f64 {
    let filter = format!("asetpts=N/SR/TB,atrim=start={from}:end={to},pan=mono|c0=c0,volumedetect");
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(file)
        .args(["-map", "0:a", "-af", &filter, "-f", "null", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    log.split("mean_volume: ")
        .nth(1)
        .and_then(|v| v.split(' ').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no volume in {log}"))
}

#[tokio::test]
async fn the_downmix_keeps_the_centre_at_the_fronts_level_and_the_lfe() {
    let dir = tempfile::tempdir().unwrap();
    let clip = tone_clip(dir.path());
    let info = crate::probe::probe(Path::new("ffprobe"), &clip)
        .await
        .unwrap();
    assert_eq!(info.audio[0].channels, Some(6));
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let mut volumes = Vec::new();
    for mode in [AudioMode::Normal, AudioMode::Dialogue, AudioMode::Levelled] {
        let settings = Settings {
            audio_mode: Some(mode),
            ..Settings::default()
        };
        let p = plan(clip.clone(), &info, &settings, Hardware::Software).unwrap();
        let session = format!("{mode:?}");
        let segment = t.segment(session.as_bytes(), &p, 0).await.unwrap();
        let out = crate::probe::probe(Path::new("ffprobe"), &segment)
            .await
            .unwrap();
        assert_eq!(out.audio[0].channels, Some(2), "{mode:?}");
        // The middle of each tone: centre, front, LFE, quiet centre.
        volumes.push([0.6, 1.8, 3.0, 4.2].map(|at| left_volume(&segment, at, at + 0.4)));
    }
    let [centre, front, lfe, quiet] = volumes[0];
    assert!(
        (centre - front).abs() < 1.0,
        "centre {centre} dB, front {front} dB"
    );
    // 10 dB lower, split between the two sides: 3 dB less again in each.
    let lfe_below = front - lfe;
    assert!(
        (11.5..14.5).contains(&lfe_below),
        "LFE {lfe_below} dB below"
    );
    assert!((19.0..21.0).contains(&(centre - quiet)), "{volumes:?}");
    // Dialogue brings the whisper up, closer to the shout.
    let [centre_dialogue, _, _, quiet_dialogue] = volumes[1];
    assert!(quiet_dialogue > quiet + 4.0, "{volumes:?}");
    assert!(
        centre_dialogue - quiet_dialogue < centre - quiet - 4.0,
        "{volumes:?}"
    );
    // Levelled brings the whole quiet mix up towards −16 LUFS (over seconds, so a clip this
    // short is mostly raised as a whole).
    let [centre_levelled, _, _, quiet_levelled] = volumes[2];
    assert!(centre_levelled > centre + 2.0, "{volumes:?}");
    assert!(quiet_levelled > quiet + 2.0, "{volumes:?}");
}
