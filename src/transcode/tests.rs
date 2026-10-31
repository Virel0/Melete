use super::*;
use crate::probe::{AudioStream, VideoStream};

fn info(codec: &str, height: i64, pixel_format: &str, hdr: Option<&str>) -> MediaInfo {
    let audio = |index: i64, channels: i64, default: bool| AudioStream {
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
        duration_seconds: Some(5400.0),
        bit_rate: None,
        video: vec![VideoStream {
            index: 0,
            codec: codec.into(),
            profile: None,
            width: Some(height * 16 / 9),
            height: Some(height),
            pixel_format: Some(pixel_format.into()),
            frame_rate: Some(23.976),
            hdr: hdr.map(Into::into),
            dolby_vision: None,
        }],
        audio: vec![audio(1, 6, false), audio(2, 2, true)],
        subtitles: Vec::new(),
    }
}

fn plan_for(info: &MediaInfo, settings: Settings, hw: Hardware) -> Plan {
    plan("/media/m.mkv".into(), info, &settings, hw).unwrap()
}

#[test]
fn plan_defaults_follow_the_source() {
    let p = plan_for(
        &info("h264", 1080, "yuv420p", None),
        Settings::default(),
        Hardware::Nvenc,
    );
    assert_eq!(p.height, None);
    assert_eq!(p.bitrate, 10_000_000);
    assert_eq!(p.audio, Some(2), "the default audio track");
    assert_eq!(p.channels, 2);
    assert!(p.gpu_decode && !p.tonemap && !p.high_bit_depth);
    let software = plan_for(
        &info("h264", 1080, "yuv420p", None),
        Settings::default(),
        Hardware::Software,
    );
    assert!(!software.gpu_decode);
}

#[test]
fn plan_applies_settings() {
    let source = info("h264", 2160, "yuv420p", None);
    let settings = Settings {
        max_height: Some(720),
        max_bitrate: Some(3_000_000),
        audio_stream: Some(1),
        audio_channels: Some(6),
        burn_subtitle: None,
        video_codec: None,
        audio_mode: None,
        audio_codecs: None,
        dolby_vision: None,
    };
    let p = plan_for(&source, settings, Hardware::Nvenc);
    assert_eq!(
        (p.height, p.bitrate, p.audio, p.channels),
        (Some(720), 3_000_000, Some(1), 6)
    );
    let taller = Settings {
        max_height: Some(4320),
        ..Settings::default()
    };
    assert_eq!(
        plan_for(&source, taller, Hardware::Nvenc).height,
        None,
        "never upscales"
    );
    let missing = Settings {
        audio_stream: Some(9),
        ..Settings::default()
    };
    assert_eq!(
        plan("/m".into(), &source, &missing, Hardware::Nvenc),
        Err("no such audio stream")
    );
}

#[test]
fn plan_tone_maps_hdr_and_handles_high_bit_depth() {
    let p = plan_for(
        &info("hevc", 2160, "yuv420p10le", Some("HDR10")),
        Settings::default(),
        Hardware::Nvenc,
    );
    assert!(p.tonemap && p.high_bit_depth && p.gpu_decode);
    let hi10p = plan_for(
        &info("h264", 1080, "yuv420p10le", None),
        Settings::default(),
        Hardware::Nvenc,
    );
    assert!(!hi10p.gpu_decode, "NVDEC can't decode 10-bit H.264");
}

#[test]
fn plan_needs_a_duration_and_video() {
    let mut no_duration = info("h264", 1080, "yuv420p", None);
    no_duration.duration_seconds = None;
    assert!(
        plan(
            "/m".into(),
            &no_duration,
            &Settings::default(),
            Hardware::Nvenc
        )
        .is_err()
    );
    let mut no_video = info("h264", 1080, "yuv420p", None);
    no_video.video.clear();
    assert_eq!(
        plan(
            "/m".into(),
            &no_video,
            &Settings::default(),
            Hardware::Nvenc
        ),
        Err("the file has no video stream")
    );
}

#[test]
fn settings_are_bounded() {
    assert!(Settings::default().validate().is_ok());
    for bad in [
        Settings {
            max_height: Some(100),
            ..Settings::default()
        },
        Settings {
            max_bitrate: Some(1),
            ..Settings::default()
        },
        Settings {
            audio_channels: Some(0),
            ..Settings::default()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}

#[test]
fn playlist_lists_the_whole_file() {
    assert_eq!(
        playlist(14.0),
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:6.000000,\n0.ts\n#EXTINF:6.000000,\n1.ts\n\
             #EXTINF:2.000000,\n2.ts\n#EXT-X-ENDLIST\n"
    );
    assert_eq!(segment_count(12.0), 2);
    assert_eq!(segment_count(0.5), 1);
}

#[test]
fn copy_keeps_the_picture_as_it_is() {
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    assert!(copy.validate().is_ok());
    for bad in [
        Settings {
            max_height: Some(720),
            ..copy.clone()
        },
        Settings {
            max_bitrate: Some(1_000_000),
            ..copy.clone()
        },
        Settings {
            burn_subtitle: Some("s3".into()),
            ..copy.clone()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    let hdr = info("hevc", 2160, "yuv420p10le", Some("HDR10"));
    let p = plan_for(&hdr, copy.clone(), Hardware::Nvenc);
    assert!(p.fmp4 && !p.gpu_decode && !p.tonemap && p.height.is_none());
    assert_eq!(p.extension(), "m4s");
    assert!(
        !plan_for(
            &info("h264", 1080, "yuv420p", None),
            copy.clone(),
            Hardware::Nvenc
        )
        .fmp4
    );

    let refused = |info: &MediaInfo| plan("/m.mkv".into(), info, &copy, Hardware::Nvenc);
    assert!(refused(&info("vc1", 1080, "yuv420p", None)).is_err());
    let mut transport = info("h264", 1080, "yuv420p", None);
    transport.container = Some("mpegts".into());
    assert!(
        refused(&transport).is_err(),
        "no index to find keyframes in"
    );
    let mut mp4 = transport.clone();
    mp4.container = Some("mov,mp4,m4a,3gp,3g2,mj2".into());
    assert!(refused(&mp4).is_ok());
}

#[test]
fn hevc_keeps_hdr10_and_hlg_in_ten_bits() {
    let hevc = Settings {
        video_codec: Some(VideoCodec::Hevc),
        max_height: Some(1080),
        ..Settings::default()
    };
    let hdr = info("hevc", 2160, "yuv420p10le", Some("HDR10"));
    let p = plan_for(&hdr, hevc.clone(), Hardware::Nvenc);
    assert!(p.fmp4 && p.ten_bit && !p.tonemap && p.gpu_decode);
    let a = args(&p, Hardware::Nvenc, 0);
    assert_eq!(value_after(&a, "-c:v"), Some("hevc_nvenc"));
    assert_eq!(value_after(&a, "-profile:v"), Some("main10"));
    assert_eq!(value_after(&a, "-tag:v"), Some("hvc1"));
    assert_eq!(
        value_after(&a, "-vf"),
        Some("scale_cuda=w=-2:h=1080:format=p010le")
    );
    assert_eq!(value_after(&a, "-hls_segment_type"), Some("fmp4"));
    assert_eq!(value_after(&a, "-hls_fmp4_init_filename"), Some("init.mp4"));
    assert!(
        value_after(&a, "-hls_segment_filename")
            .unwrap()
            .ends_with("seg%d.m4s")
    );

    let software = plan_for(&hdr, hevc.clone(), Hardware::Software);
    let a = args(&software, Hardware::Software, 0);
    assert_eq!(value_after(&a, "-c:v"), Some("libx265"));
    assert_eq!(
        value_after(&a, "-vf"),
        Some("scale=-2:1080,format=yuv420p10le")
    );
    let hlg = info("hevc", 2160, "yuv420p10le", Some("HLG"));
    assert!(!plan_for(&hlg, hevc.clone(), Hardware::Software).tonemap);
    // CPU-decoded 10-bit frames go up to the GPU as p010.
    let av1 = info("av1", 2160, "yuv420p12le", Some("HDR10"));
    let mut cpu = plan_for(&av1, hevc.clone(), Hardware::Nvenc);
    cpu.gpu_decode = false;
    let a = args(&cpu, Hardware::Nvenc, 0);
    assert_eq!(
        value_after(&a, "-vf"),
        Some("format=p010le,hwupload_cuda,scale_cuda=w=-2:h=1080:format=p010le")
    );

    // Dolby Vision may have no HDR10 picture, and subtitles are drawn on 8-bit SDR frames.
    let dv = info("hevc", 2160, "yuv420p10le", Some("Dolby Vision"));
    let p = plan_for(&dv, hevc.clone(), Hardware::Software);
    assert!(p.tonemap && !p.ten_bit);
    assert_eq!(
        value_after(&args(&p, Hardware::Software, 0), "-profile:v"),
        Some("main")
    );
    let burned = Settings {
        burn_subtitle: Some("s3".into()),
        ..hevc.clone()
    };
    let p = plan_for(&with_subtitles(hdr.clone()), burned, Hardware::Software);
    assert!(p.tonemap && !p.ten_bit && p.fmp4);
    // A 10-bit SDR source stays 10-bit; an 8-bit one is Main.
    let p = plan_for(
        &info("hevc", 1080, "yuv420p10le", None),
        hevc.clone(),
        Hardware::Software,
    );
    assert!(p.ten_bit && !p.tonemap);
    let p = plan_for(
        &info("h264", 1080, "yuv420p", None),
        hevc,
        Hardware::Software,
    );
    assert!(!p.ten_bit);
    assert_eq!(
        value_after(&args(&p, Hardware::Software, 0), "-profile:v"),
        Some("main")
    );
}

/// A Dolby Vision HEVC source whose base layer is `fallback` to other players.
fn dolby_vision(profile: i64, fallback: Option<&str>) -> MediaInfo {
    let mut source = info("hevc", 2160, "yuv420p10le", Some("Dolby Vision"));
    source.video[0].dolby_vision = Some(crate::probe::DolbyVision {
        profile: Some(profile),
        fallback: fallback.map(Into::into),
    });
    source
}

#[test]
fn dolby_vision_keeps_the_picture_other_players_see() {
    let hevc = Settings {
        video_codec: Some(VideoCodec::Hevc),
        ..Settings::default()
    };
    // Profiles 7 and 8.1 are HDR10 underneath: kept, in 10 bits.
    for source in [
        dolby_vision(8, Some("hdr10")),
        dolby_vision(7, Some("hdr10")),
    ] {
        let p = plan_for(&source, hevc.clone(), Hardware::Software);
        assert!(!p.tonemap && p.ten_bit && !p.dolby_vision_only, "{p:?}");
        let a = args(&p, Hardware::Software, 0);
        assert_eq!(value_after(&a, "-dolbyvision"), Some("0"), "plain HDR10");
    }
    let hlg = plan_for(&dolby_vision(8, Some("hlg")), hevc.clone(), Hardware::Nvenc);
    assert!(!hlg.tonemap && hlg.ten_bit);
    // Profile 5 has nothing else to keep, and a probe without the profile counts as it.
    let mut unknown = dolby_vision(5, None);
    unknown.video[0].dolby_vision = None;
    for source in [dolby_vision(5, None), unknown] {
        let p = plan_for(&source, hevc.clone(), Hardware::Software);
        assert!(p.tonemap && !p.ten_bit && p.dolby_vision_only);
    }
    // An SDR base layer (8.2) is never tone-mapped, whatever the output.
    for settings in [hevc.clone(), Settings::default()] {
        let p = plan_for(&dolby_vision(8, Some("sdr")), settings, Hardware::Nvenc);
        assert!(!p.tonemap, "{p:?}");
    }
    assert!(
        plan_for(
            &dolby_vision(8, Some("hdr10")),
            Settings::default(),
            Hardware::Nvenc
        )
        .tonemap,
        "H.264 is SDR"
    );
}

#[test]
fn profile_5_is_tone_mapped_with_its_metadata_on_the_cpu_when_ffmpeg_can() {
    let vf = |source: &MediaInfo, abilities: Abilities| {
        let p = plan_for(source, Settings::default(), Hardware::Software);
        let a: Vec<String> = ffmpeg_args_for(&p, Hardware::Software, abilities, 0, Path::new("/t"))
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        value_after(&a, "-vf").unwrap().to_owned()
    };
    let jellyfin = Abilities { tonemapx: true };
    let five = dolby_vision(5, None);
    assert!(
        vf(&five, jellyfin).starts_with("tonemapx=")
            && vf(&five, jellyfin).ends_with("format=yuv420p")
    );
    assert!(
        vf(&five, Abilities::default()).starts_with("zscale="),
        "plain ffmpeg has no tonemapx"
    );
    let hdr10 = info("hevc", 2160, "yuv420p10le", Some("HDR10"));
    assert!(
        vf(&hdr10, jellyfin).starts_with("zscale="),
        "everything else is as before"
    );
    // Burning subtitles in tone-maps the same way.
    let mut burned = plan_for(&five, Settings::default(), Hardware::Software);
    burned.burn = Some(Burn::text("/c/subtitles/k.ass".into()).unwrap());
    let graph = burn_graph(
        &burned,
        burned.burn.as_ref().unwrap(),
        Hardware::Software,
        jellyfin,
        "0.000",
        0,
    );
    assert!(graph.contains("tonemapx="), "{graph}");
    assert_eq!(
        Abilities::of(Path::new("/nonexistent/ffmpeg")),
        Abilities::default()
    );
}

#[test]
fn copied_dolby_vision_profile_5_is_tagged_as_dolby_vision() {
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    let tag = |source: MediaInfo| {
        let mut p = plan_for(&source, copy.clone(), Hardware::Nvenc);
        p.keyframes = Some(Arc::new(Keyframes {
            starts: vec![0.0, 6.0],
            seek_past: 0.0005,
            audio_from: Vec::new(),
        }));
        value_after(&args(&p, Hardware::Nvenc, 1), "-tag:v").map(str::to_owned)
    };
    assert_eq!(tag(dolby_vision(5, None)).as_deref(), Some("dvh1"));
    assert_eq!(tag(dolby_vision(8, Some("hdr10"))).as_deref(), Some("hvc1"));
    assert_eq!(
        tag(info("hevc", 2160, "yuv420p10le", Some("HDR10"))).as_deref(),
        Some("hvc1")
    );
}

fn copy_plan(starts: &[f64], duration: f64, fmp4: bool, seek_past: f64) -> Plan {
    let source = if fmp4 { "hevc" } else { "h264" };
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    let mut p = plan_for(
        &info(source, 1080, "yuv420p", None),
        copy,
        Hardware::Software,
    );
    p.duration = duration;
    p.keyframes = Some(Arc::new(Keyframes {
        starts: starts.to_vec(),
        seek_past,
        audio_from: Vec::new(),
    }));
    p
}

#[test]
fn copied_video_is_listed_by_keyframes() {
    let p = copy_plan(&[0.0, 4.171, 10.427], 14.0, false, 0.0005);
    assert_eq!(
        p.playlist(),
        "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:7\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:4.171000,\n0.ts\n#EXTINF:6.256000,\n1.ts\n\
             #EXTINF:3.573000,\n2.ts\n#EXT-X-ENDLIST\n"
    );
    assert_eq!(p.segment_count(), 3);
    let hevc = copy_plan(&[0.0, 5.839], 9.0, true, 0.0005);
    assert_eq!(
        hevc.playlist(),
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:0\n\
             #EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:5.839000,\n0.m4s\n\
             #EXTINF:3.161000,\n1.m4s\n#EXT-X-ENDLIST\n"
    );
}

#[test]
fn copy_runs_seek_just_past_the_keyframe() {
    let p = copy_plan(&[0.0, 4.171, 10.427], 14.0, false, 0.130934);
    let a = args(&p, Hardware::Nvenc, 2);
    assert_eq!(value_after(&a, "-ss"), Some("10.557934"));
    assert!(a.contains(&"-copyts".to_string()) && !a.contains(&"-output_ts_offset".to_string()));
    assert_eq!(value_after(&a, "-c:v"), Some("copy"));
    assert_eq!(value_after(&a, "-c:a"), Some("aac"));
    assert_eq!(value_after(&a, "-hls_time"), Some("0.001"));
    assert_eq!(value_after(&a, "-start_number"), Some("0"));
    assert!(
        value_after(&a, "-hls_segment_filename")
            .unwrap()
            .ends_with("c%d.ts")
    );
    assert!(!a.contains(&"-hwaccel".to_string()) && !a.contains(&"-vf".to_string()));
    let first = args(&p, Hardware::Nvenc, 0);
    assert!(!first.contains(&"-ss".to_string()));
    let hevc = copy_plan(&[0.0, 5.839], 9.0, true, 0.0005);
    assert_eq!(
        value_after(&args(&hevc, Hardware::Nvenc, 1), "-tag:v"),
        Some("hvc1")
    );
}

#[test]
fn chunks_belong_to_the_segment_they_start_in() {
    let starts = [0.0, 4.171, 10.427, 16.683];
    // A run from segment 1, keyframes every 2.085 s or so.
    let playlist = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n\
            #EXTINF:2.085000,\nc0.ts\n#EXTINF:2.086000,\nc1.ts\n#EXTINF:2.085000,\nc2.ts\n\
            #EXTINF:2.085000,\nc3.ts\n";
    let (chunks, finished) = chunk_segments(playlist, &starts, 1);
    let names = |n: i64| -> Vec<&str> {
        chunks
            .iter()
            .filter(|(s, _)| *s == n)
            .map(|(_, c)| c.as_str())
            .collect()
    };
    assert_eq!(names(1), ["c0", "c1", "c2"].map(|c| format!("{c}.ts")));
    assert_eq!(names(2), ["c3.ts"]);
    assert_eq!(
        finished,
        Some(1),
        "segment 2 isn't finished until a chunk past it is"
    );
    let (_, finished) = chunk_segments(&format!("{playlist}#EXT-X-ENDLIST\n"), &starts, 1);
    assert_eq!(finished, Some(3), "the end finishes them all");
    let (chunks, finished) = chunk_segments("#EXTM3U\n", &starts, 1);
    assert!(chunks.is_empty() && finished.is_none());
    let odd = "#EXTINF:2.0,\n../../library/film.mkv\n#EXTINF:2.0,\n/etc/x\n#EXTINF:2.0,\nc.ts\n";
    let (chunks, _) = chunk_segments(odd, &starts, 1);
    assert!(
        chunks.is_empty(),
        "never a name of anything but a chunk: {chunks:?}"
    );
    assert_eq!(chunk_segments(playlist, &starts, 9), (Vec::new(), None));
}

fn args(plan: &Plan, hw: Hardware, start: i64) -> Vec<String> {
    ffmpeg_args(plan, hw, start, Path::new("/t/s"))
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect()
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .map(|i| args[i + 1].as_str())
}

#[test]
fn software_args_from_the_start_and_after_a_seek() {
    let p = plan_for(
        &info("h264", 1080, "yuv420p", None),
        Settings::default(),
        Hardware::Software,
    );
    let a = args(&p, Hardware::Software, 0);
    assert!(!a.contains(&"-ss".to_string()) && !a.contains(&"-output_ts_offset".to_string()));
    assert_eq!(value_after(&a, "-i"), Some("file:/media/m.mkv"));
    assert_eq!(value_after(&a, "-c:v"), Some("libx264"));
    assert_eq!(value_after(&a, "-vf"), Some("format=yuv420p"));
    assert_eq!(
        value_after(&a, "-force_key_frames"),
        Some("expr:gte(t,n_forced*6)")
    );
    assert_eq!(a.last().map(String::as_str), Some("/t/s/ffmpeg.m3u8"));

    let seeked = args(&p, Hardware::Software, 3);
    let ss = seeked.iter().position(|x| x == "-ss").unwrap();
    let input = seeked.iter().position(|x| x == "-i").unwrap();
    assert!(ss < input, "-ss before -i seeks fast");
    assert_eq!(value_after(&seeked, "-ss"), Some("18.000"));
    assert_eq!(value_after(&seeked, "-output_ts_offset"), Some("18.000"));
    assert_eq!(value_after(&seeked, "-start_number"), Some("3"));
}

#[test]
fn nvenc_args_decode_scale_and_tone_map_on_the_gpu() {
    let hdr = Settings {
        max_height: Some(1080),
        ..Settings::default()
    };
    let p = plan_for(
        &info("hevc", 2160, "yuv420p10le", Some("HDR10")),
        hdr,
        Hardware::Nvenc,
    );
    let a = args(&p, Hardware::Nvenc, 0);
    assert_eq!(value_after(&a, "-hwaccel"), Some("cuda"));
    assert_eq!(value_after(&a, "-c:v"), Some("h264_nvenc"));
    assert_eq!(
        value_after(&a, "-vf"),
        Some(
            "tonemap_cuda=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0,scale_cuda=w=-2:h=1080:format=nv12"
        )
    );

    let p = plan_for(
        &info("h264", 1080, "yuv420p10le", None),
        Settings::default(),
        Hardware::Nvenc,
    );
    let a = args(&p, Hardware::Nvenc, 0);
    assert!(!a.contains(&"-hwaccel".to_string()));
    assert_eq!(
        value_after(&a, "-vf"),
        Some("hwupload_cuda,scale_cuda=format=nv12")
    );
}

#[test]
fn throttling_pauses_far_ahead_and_resumes_close() {
    assert_eq!(throttle(AHEAD_PAUSE + 1, false), Some(libc::SIGSTOP));
    assert_eq!(throttle(AHEAD_PAUSE + 1, true), None);
    assert_eq!(throttle(AHEAD_RESUME - 1, true), Some(libc::SIGCONT));
    assert_eq!(throttle(AHEAD_RESUME - 1, false), None);
    assert_eq!(throttle(20, true), None, "between the two: leave it");
}

fn real_plan(clip: &Path) -> Plan {
    Plan {
        input: clip.into(),
        duration: 60.0,
        video: 0,
        gpu_decode: false,
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

#[tokio::test]
async fn encodes_segments_and_restarts_on_a_far_seek() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 60);
    let root = dir.path().join("transcode");
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, root.clone(), 4);
    let plan = real_plan(&clip);

    let first = t.segment(b"one", &plan, 0).await.unwrap();
    let info = crate::probe::probe(Path::new("ffprobe"), &first)
        .await
        .unwrap();
    assert_eq!(info.video[0].codec, "h264");
    assert_eq!(info.audio[0].codec, "aac");
    assert!(
        info.duration_seconds.unwrap() > 5.0,
        "{:?}",
        info.duration_seconds
    );
    assert_eq!(t.running_start(b"one"), Some(0));

    let far = t.segment(b"one", &plan, 9).await.unwrap();
    assert!(far.ends_with("seg9.ts"), "{}", far.display());
    assert_eq!(
        t.running_start(b"one"),
        Some(9),
        "ffmpeg restarted at the seek"
    );
    let seeked = crate::probe::probe(Path::new("ffprobe"), &far)
        .await
        .unwrap();
    assert!(seeked.duration_seconds.unwrap() > 5.0);
}

#[tokio::test]
async fn too_many_transcodes_are_refused_and_idle_ones_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 20);
    let root = dir.path().join("transcode");
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, root.clone(), 1);
    let plan = Plan {
        duration: 20.0,
        ..real_plan(&clip)
    };

    t.segment(b"one", &plan, 0).await.unwrap();
    assert!(matches!(
        t.segment(b"two", &plan, 0).await,
        Err(TranscodeError::Busy)
    ));

    t.sweep(Duration::ZERO);
    assert_eq!(t.running_start(b"one"), None);
    // Stopping kills ffmpeg, waits for it to exit and then deletes the run's folder, in a
    // task of its own: wait for that rather than for a fixed time, which a busy machine
    // can overrun. The deadline only keeps a real failure from hanging the test.
    let entries = || std::fs::read_dir(&root).unwrap().count();
    let deadline = Instant::now() + SEGMENT_WAIT;
    while entries() > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(entries(), 0, "segments deleted");
    t.segment(b"two", &plan, 0).await.unwrap();
}

#[tokio::test]
async fn a_new_transcode_takes_the_slot_of_one_nobody_asks_for() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 20);
    let t = Transcoder::new(
        "ffmpeg".into(),
        Hardware::Software,
        dir.path().join("transcode"),
        1,
    );
    let plan = Plan {
        duration: 20.0,
        ..real_plan(&clip)
    };

    t.segment(b"left", &plan, 0).await.unwrap();
    // Its viewer left the player: nothing has asked for it since.
    t.running
        .lock()
        .unwrap()
        .get_mut(b"left".as_slice())
        .unwrap()
        .last_request = Instant::now() - TAKE_OVER_AFTER;

    t.segment(b"new", &plan, 0).await.unwrap();
    assert_eq!(
        t.running_start(b"left"),
        None,
        "the quiet run gave up its slot"
    );
    assert_eq!(t.running_start(b"new"), Some(0));
    // A run asked for just now keeps its slot.
    assert!(matches!(
        t.segment(b"third", &plan, 0).await,
        Err(TranscodeError::Busy)
    ));
}

#[tokio::test]
async fn rewinding_to_where_ffmpeg_last_started_works() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 60);
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let plan = real_plan(&clip);
    for n in 0..=8 {
        t.segment(b"one", &plan, n).await.unwrap();
    }
    // The supervisor deletes segments 0-2, well behind the viewer at 8.
    t.sweep(IDLE);
    // Going back to 0 restarts ffmpeg at 0, where the deleted run also started.
    let back = t.segment(b"one", &plan, 0).await.unwrap();
    assert!(back.exists());
    assert_eq!(
        t.segment(b"one", &plan, 1)
            .await
            .unwrap()
            .file_name()
            .unwrap(),
        "seg1.ts"
    );
}

#[tokio::test]
async fn sessions_never_share_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let a = crate::probe::testing::make_long_clip(dir.path(), 12);
    let a_moved = dir.path().join("a.mkv");
    std::fs::rename(&a, &a_moved).unwrap();
    let b = crate::probe::testing::make_long_clip(dir.path(), 12);
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let one = t
        .segment(
            b"one",
            &Plan {
                duration: 12.0,
                ..real_plan(&a_moved)
            },
            0,
        )
        .await
        .unwrap();
    let two = t
        .segment(
            b"two",
            &Plan {
                duration: 12.0,
                ..real_plan(&b)
            },
            0,
        )
        .await
        .unwrap();
    assert_ne!(one.parent(), two.parent());
}

#[test]
fn startup_cleanup_deletes_only_its_own_folders() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("7.hls")).unwrap();
    std::fs::write(dir.path().join("7.hls/seg0.ts"), b"x").unwrap();
    std::fs::create_dir(dir.path().join("artwork")).unwrap();
    std::fs::write(dir.path().join("melete.db"), b"x").unwrap();
    let _t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().into(), 4);
    let mut left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["artwork", "melete.db"]);
}

#[test]
fn transcode_dir_must_not_overlap_data_or_libraries() {
    let config = Path::new("/config");
    let movies = Path::new("/media/movies");
    assert!(check_dir(Path::new("/transcode"), &[config, movies]).is_ok());
    for bad in [
        "/config",
        "/config/transcode",
        "/media",
        "/media/movies/tmp",
        "/",
    ] {
        assert!(
            check_dir(Path::new(bad), &[config, movies]).is_err(),
            "{bad}"
        );
    }
}

fn with_subtitles(mut info: MediaInfo) -> MediaInfo {
    let track = |index: i64, codec: &str, text: bool| crate::probe::SubtitleStream {
        index,
        codec: codec.into(),
        language: None,
        title: None,
        default: false,
        forced: false,
        text,
    };
    info.subtitles = vec![
        track(3, "hdmv_pgs_subtitle", false),
        track(4, "subrip", true),
    ];
    info
}

#[test]
fn picture_tracks_burn_at_the_output_size_and_decode_on_the_cpu() {
    let source = with_subtitles(info("hevc", 1080, "yuv420p", None));
    let burn = |track: &str, max_height: Option<i64>| {
        let settings = Settings {
            burn_subtitle: Some(track.into()),
            max_height,
            ..Settings::default()
        };
        plan("/m".into(), &source, &settings, Hardware::Nvenc)
    };
    let p = burn("s3", Some(720)).unwrap();
    assert_eq!(
        p.burn,
        Some(Burn::Picture {
            index: 3,
            width: 1280,
            height: 720
        })
    );
    assert!(!p.gpu_decode, "subtitles are drawn on the CPU");
    let full = burn("s3", None).unwrap();
    assert_eq!(
        full.burn,
        Some(Burn::Picture {
            index: 3,
            width: 1920,
            height: 1080
        })
    );
    let text = burn("s4", None).unwrap();
    assert_eq!(text.burn, None, "text tracks are rendered by the caller");
    assert!(!text.gpu_decode);
    assert!(burn("x0", None).unwrap().burn.is_none());
    assert_eq!(burn("s9", None), Err("no such subtitle track"));
}

#[test]
fn burn_track_ids_are_checked() {
    for good in ["s3", "x1a2b3c4d", "s12345"] {
        let settings = Settings {
            burn_subtitle: Some(good.into()),
            ..Settings::default()
        };
        assert!(settings.validate().is_ok(), "{good}");
    }
    for bad in [
        "3",
        "s",
        "sx",
        "s123456",
        "../a",
        "y1",
        "x0",
        "x1A2B3C4D",
        "x1a2b3c4",
    ] {
        let settings = Settings {
            burn_subtitle: Some(bad.into()),
            ..Settings::default()
        };
        assert!(settings.validate().is_err(), "{bad}");
    }
}

#[test]
fn text_burn_paths_must_be_plain() {
    assert!(Burn::text("/config/subtitles/1-s2-3-4.ass".into()).is_ok());
    for bad in ["/my subs/a.ass", "/x'.ass", "/a:b.ass", "/a,b.ass"] {
        assert!(Burn::text(bad.into()).is_err(), "{bad}");
    }
}

#[test]
fn burn_graphs_for_pictures_and_text() {
    let hdr = with_subtitles(info("hevc", 2160, "yuv420p10le", Some("HDR10")));
    let settings = Settings {
        burn_subtitle: Some("s3".into()),
        max_height: Some(1080),
        ..Settings::default()
    };
    let p = plan("/m.mkv".into(), &hdr, &settings, Hardware::Nvenc).unwrap();
    let a = args(&p, Hardware::Nvenc, 0);
    assert!(!a.contains(&"-vf".to_string()) && !a.contains(&"-hwaccel".to_string()));
    assert_eq!(value_after(&a, "-map"), Some("[out]"));
    assert_eq!(
        value_after(&a, "-filter_complex"),
        Some(
            "[0:3]scale=1920:1080:force_original_aspect_ratio=decrease[sub];[0:0]format=p010le,hwupload_cuda,tonemap_cuda=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0,scale_cuda=w=-2:h=1080:format=nv12,hwdownload,format=nv12[main];[main][sub]overlay=x=(W-w)/2:y=H-h:eof_action=pass:repeatlast=0,format=nv12[out]"
        )
    );

    let sdr = with_subtitles(info("h264", 1080, "yuv420p", None));
    let mut p = plan_for(
        &sdr,
        Settings {
            burn_subtitle: Some("s4".into()),
            ..Settings::default()
        },
        Hardware::Software,
    );
    p.burn = Some(Burn::text("/c/subtitles/k.ass".into()).unwrap());
    assert_eq!(
        value_after(&args(&p, Hardware::Software, 0), "-filter_complex"),
        Some("[0:0]subtitles=filename=/c/subtitles/k.ass,format=yuv420p[out]")
    );
    assert_eq!(
        value_after(&args(&p, Hardware::Software, 2), "-filter_complex"),
        Some(
            "[0:0]setpts=PTS+12.000/TB,subtitles=filename=/c/subtitles/k.ass,setpts=PTS-12.000/TB,format=yuv420p[out]"
        ),
        "after a restart the subtitles keep the file's own times"
    );
}

#[test]
fn text_burns_can_carry_the_videos_fonts() {
    let sdr = with_subtitles(info("h264", 1080, "yuv420p", None));
    let mut p = plan_for(
        &sdr,
        Settings {
            burn_subtitle: Some("s4".into()),
            ..Settings::default()
        },
        Hardware::Software,
    );
    p.burn = Some(
        Burn::text("/c/subtitles/k.ass".into())
            .unwrap()
            .with_fonts("/c/subtitles/fonts-1-2-3".into())
            .unwrap(),
    );
    assert_eq!(
        value_after(&args(&p, Hardware::Software, 0), "-filter_complex"),
        Some(
            "[0:0]subtitles=filename=/c/subtitles/k.ass:fontsdir=/c/subtitles/fonts-1-2-3,format=yuv420p[out]"
        )
    );
    assert!(
        Burn::text("/c/k.ass".into())
            .unwrap()
            .with_fonts("/c/my fonts".into())
            .is_err()
    );
}

/// The first frame of a segment as grey pixels.
fn first_frame(segment: &Path) -> Vec<u8> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(segment)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(out.status.success() && !out.stdout.is_empty());
    out.stdout
}

#[tokio::test]
async fn burned_in_text_changes_the_picture() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 12);
    let ass = dir.path().join("subs.ass");
    let srt = dir.path().join("subs.srt");
    std::fs::write(&srt, "1\n00:00:00,000 --> 00:00:11,000\nBURNED IN\n").unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&srt)
        .arg(&ass)
        .status()
        .unwrap();
    assert!(status.success());
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let plain = Plan {
        duration: 12.0,
        ..real_plan(&clip)
    };
    let burned = Plan {
        burn: Some(Burn::text(ass).unwrap()),
        ..plain.clone()
    };

    let without = first_frame(&t.segment(b"plain", &plain, 0).await.unwrap());
    let again = first_frame(&t.segment(b"plain again", &plain, 0).await.unwrap());
    assert_eq!(
        without, again,
        "encoding is repeatable, so a difference means the subtitle"
    );
    let with = first_frame(&t.segment(b"burned", &burned, 0).await.unwrap());
    assert_eq!(without.len(), with.len());
    assert_ne!(without, with, "the subtitle is drawn into the frame");
}

#[tokio::test]
async fn burned_in_text_keeps_its_timing_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 18);
    let srt = dir.path().join("late.srt");
    let ass = dir.path().join("late.ass");
    // Only on screen during segment 1 (6-12 s).
    std::fs::write(&srt, "1\n00:00:06,000 --> 00:00:11,000\nLATE CUE\n").unwrap();
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&srt)
        .arg(&ass)
        .status()
        .unwrap();
    assert!(status.success());
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let plain = Plan {
        duration: 18.0,
        ..real_plan(&clip)
    };
    let burned = Plan {
        burn: Some(Burn::text(ass).unwrap()),
        ..plain.clone()
    };

    // Asking for segment 1 first starts ffmpeg there, with -ss 6.
    let plain_1 = first_frame(&t.segment(b"plain", &plain, 1).await.unwrap());
    let burned_1 = first_frame(&t.segment(b"burned", &burned, 1).await.unwrap());
    assert_ne!(
        plain_1, burned_1,
        "the 6-11 s cue shows in segment 1 after the restart"
    );

    let plain_2 = first_frame(&t.segment(b"plain 2", &plain, 2).await.unwrap());
    let burned_2 = first_frame(&t.segment(b"burned 2", &burned, 2).await.unwrap());
    assert_eq!(plain_2, burned_2, "and not in segment 2 (12 s on)");
}

#[tokio::test]
async fn a_failing_ffmpeg_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let plan = Plan {
        input: dir.path().join("missing.mkv"),
        ..real_plan(dir.path())
    };
    match t.segment(b"one", &plan, 0).await {
        Err(TranscodeError::Failed(log)) => assert!(!log.is_empty()),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert_eq!(t.running_start(b"one"), None, "the dead run is dropped");
    let broken = Transcoder::new(
        "/nonexistent/ffmpeg".into(),
        Hardware::Software,
        dir.path().join("u"),
        4,
    );
    assert!(matches!(
        broken.segment(b"one", &plan, 0).await,
        Err(TranscodeError::Failed(_))
    ));
}

/// A 60-second clip with keyframes every `gop` frames (24 fps) and B-frames.
fn b_frame_clip(dir: &Path, name: &str, encoder: &[&str]) -> PathBuf {
    let clip = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=24:duration=60",
        ])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=60"])
        .args(encoder)
        .args(["-c:a", "ac3"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    clip
}

/// Video packets of a segment, as display time and flags.
fn video_packets(segment: &Path, init: Option<&Path>) -> Vec<(f64, String)> {
    let file = match init {
        Some(init) => {
            let whole = segment.with_extension("whole.mp4");
            let mut bytes = std::fs::read(init).unwrap();
            bytes.extend(std::fs::read(segment).unwrap());
            std::fs::write(&whole, bytes).unwrap();
            whole
        }
        None => segment.to_path_buf(),
    };
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,flags",
        ])
        .args(["-of", "csv=p=0"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| {
            let (pts, flags) = l.split_once(',').unwrap();
            (pts.parse().unwrap(), flags.to_owned())
        })
        .collect()
}

/// Copies `clip` through to segment `n`, and again starting at `n` as after a seek: the two
/// must be the same segment, starting on the keyframe the playlist says.
async fn copy_lines_up(dir: &Path, clip: &Path, b_frames: bool, fmp4: bool, n: i64) {
    let ffprobe = Path::new("ffprobe");
    let info = crate::probe::probe(ffprobe, clip).await.unwrap();
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    let mut p = plan(clip.into(), &info, &copy, Hardware::Software).unwrap();
    assert_eq!(p.fmp4, fmp4);
    let found = keyframes(ffprobe, clip, p.video, p.duration).await.unwrap();
    let expected_past = if b_frames { 0.130934 } else { 0.0005 };
    assert!((found.seek_past - expected_past).abs() < 1e-9, "{found:?}");
    assert!(found.starts.len() as i64 > n + 1, "{found:?}");
    p.keyframes = Some(Arc::new(found.clone()));

    let name = clip.file_name().unwrap().to_str().unwrap();
    let t = Transcoder::new(
        "ffmpeg".into(),
        Hardware::Software,
        dir.join(format!("t-{name}")),
        4,
    );
    let init_of = |session: &'static [u8]| {
        let (t, p) = (&t, &p);
        async move {
            match fmp4 {
                true => Some(t.init(session, p).await.unwrap()),
                false => None,
            }
        }
    };
    let through_init = init_of(b"through").await;
    let mut first_pts = Vec::new();
    for i in 0..=n {
        let segment = t.segment(b"through", &p, i).await.unwrap();
        let packets = video_packets(&segment, through_init.as_deref());
        assert!(
            packets[0].1.contains('K'),
            "segment {i} starts on a keyframe"
        );
        first_pts.push(packets[0].0);
    }
    assert_eq!(t.running_start(b"through"), Some(0), "one run all along");
    for i in 1..=n as usize {
        let gap = first_pts[i] - first_pts[0];
        assert!(
            (gap - found.starts[i]).abs() < 0.002,
            "segment {i} starts {gap} s in, the playlist says {}",
            found.starts[i]
        );
    }
    let through = t.segment(b"through", &p, n).await.unwrap();

    let seeked = t.segment(b"seeked", &p, n).await.unwrap();
    assert_eq!(t.running_start(b"seeked"), Some(n));
    let seeked_init = init_of(b"seeked").await;
    if let (Some(a), Some(b)) = (&through_init, &seeked_init) {
        assert_eq!(
            std::fs::read(a).unwrap(),
            std::fs::read(b).unwrap(),
            "one init for every run"
        );
    }
    assert_eq!(
        video_packets(&seeked, seeked_init.as_deref()),
        video_packets(&through, through_init.as_deref()),
        "{name}: segment {n} after a seek is the same segment"
    );
}

#[tokio::test]
async fn copied_video_lines_up_after_a_seek() {
    let dir = tempfile::tempdir().unwrap();
    let plain = crate::probe::testing::make_long_clip(dir.path(), 60);
    copy_lines_up(dir.path(), &plain, false, false, 4).await;
    let h264 = b_frame_clip(
        dir.path(),
        "b-frames.mkv",
        &[
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-g",
            "50",
            "-sc_threshold",
            "0",
            "-bf",
            "3",
        ],
    );
    copy_lines_up(dir.path(), &h264, true, false, 6).await;
    let hevc = b_frame_clip(
        dir.path(),
        "hevc.mp4",
        &[
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "keyint=70:min-keyint=70:scenecut=0:bframes=3:log-level=error",
        ],
    );
    copy_lines_up(dir.path(), &hevc, true, true, 5).await;
}

#[tokio::test]
async fn copied_chunks_are_deleted_behind_the_viewer() {
    let dir = tempfile::tempdir().unwrap();
    let clip = b_frame_clip(
        dir.path(),
        "b-frames.mkv",
        &["-c:v", "libx264", "-preset", "ultrafast", "-g", "24"],
    );
    let info = crate::probe::probe(Path::new("ffprobe"), &clip)
        .await
        .unwrap();
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    let mut p = plan(clip.clone(), &info, &copy, Hardware::Software).unwrap();
    p.keyframes = Some(Arc::new(
        keyframes(Path::new("ffprobe"), &clip, p.video, p.duration)
            .await
            .unwrap(),
    ));
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    let mut files = Vec::new();
    for n in 0..=8 {
        files.push(t.segment(b"one", &p, n).await.unwrap());
    }
    let run = files[0].parent().unwrap().to_path_buf();
    let chunk_count = |below: i64| {
        let playlist = std::fs::read_to_string(run.join("ffmpeg.m3u8")).unwrap();
        let (chunks, _) = chunk_segments(&playlist, &p.keyframes.as_ref().unwrap().starts, 0);
        chunks
            .iter()
            .filter(|(n, _)| *n < below)
            .filter(|(_, c)| run.join(c).exists())
            .count()
    };
    assert!(chunk_count(3) > 0);
    // Within ten minutes and a gigabyte behind the viewer, nothing goes: going back is instant.
    t.sweep(IDLE);
    assert!(chunk_count(3) > 0, "kept within the limits");
    assert!(files[0].exists() && files[7].exists());
    // Past the number of segments kept, the oldest go…
    t.keep_behind(5, u64::MAX);
    t.sweep(IDLE);
    assert_eq!(chunk_count(3), 0, "chunks of segments 0-2 deleted");
    assert!(!files[2].exists() && files[3].exists());
    // …and past the bytes kept, whatever their number.
    t.keep_behind(100, 1);
    t.sweep(IDLE);
    assert!(
        !files[7].exists() && files[8].exists(),
        "nothing behind fits in a byte"
    );
    // Going back starts afresh.
    let back = t.segment(b"one", &p, 1).await.unwrap();
    assert!(back.exists());
    assert_eq!(t.running_start(b"one"), Some(1));
}

#[tokio::test]
async fn encoded_segments_line_up_after_a_seek() {
    let dir = tempfile::tempdir().unwrap();
    let clip = crate::probe::testing::make_long_clip(dir.path(), 30);
    let t = Transcoder::new("ffmpeg".into(), Hardware::Software, dir.path().join("t"), 4);
    for codec in [VideoCodec::H264, VideoCodec::Hevc] {
        let plan = Plan {
            duration: 30.0,
            codec,
            fmp4: codec == VideoCodec::Hevc,
            ..real_plan(&clip)
        };
        let name = format!("{codec:?}");
        let (through, seeked) = (format!("{name} through"), format!("{name} seeked"));
        let init = |session: String| {
            let (t, plan) = (&t, &plan);
            async move {
                match plan.fmp4 {
                    true => Some(t.init(session.as_bytes(), plan).await.unwrap()),
                    false => None,
                }
            }
        };
        let through_init = init(through.clone()).await;
        for n in 0..3 {
            t.segment(through.as_bytes(), &plan, n).await.unwrap();
        }
        let a = t.segment(through.as_bytes(), &plan, 3).await.unwrap();
        let b = t.segment(seeked.as_bytes(), &plan, 3).await.unwrap();
        assert_eq!(t.running_start(seeked.as_bytes()), Some(3));
        let seeked_init = init(seeked).await;
        if let (Some(x), Some(y)) = (&through_init, &seeked_init) {
            assert_eq!(
                std::fs::read(x).unwrap(),
                std::fs::read(y).unwrap(),
                "{name}: one init"
            );
        }
        // Display times: a fresh encoder may order B-frames differently.
        let shown = |packets: Vec<(f64, String)>| {
            assert!(packets[0].1.contains('K'));
            let mut times: Vec<f64> = packets.into_iter().map(|(pts, _)| pts).collect();
            times.sort_by(f64::total_cmp);
            times
        };
        assert_eq!(
            shown(video_packets(&a, through_init.as_deref())),
            shown(video_packets(&b, seeked_init.as_deref())),
            "{name}: segment 3 after a seek has frames at the same times"
        );
    }
}

#[test]
fn copied_h264_repeats_its_parameter_sets_at_keyframes() {
    let p = copy_plan(&[0.0, 4.171, 10.427], 14.0, false, 0.130934);
    assert_eq!(
        value_after(&args(&p, Hardware::Nvenc, 1), "-bsf:v"),
        Some("h264_mp4toannexb,dump_extra=freq=keyframe")
    );
    let hevc = copy_plan(&[0.0, 5.839], 9.0, true, 0.0005);
    assert_eq!(
        value_after(&args(&hevc, Hardware::Nvenc, 1), "-bsf:v"),
        None
    );
}

#[tokio::test]
async fn open_gop_h264_copies_decode_after_a_seek() {
    let dir = tempfile::tempdir().unwrap();
    let clip = b_frame_clip(
        dir.path(),
        "open-gop.mkv",
        &[
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-bf",
            "3",
            "-x264-params",
            "open-gop=1:keyint=48:min-keyint=48:scenecut=0",
        ],
    );
    copy_lines_up(dir.path(), &clip, true, false, 4).await;

    // A player seeking to segment 4 has nothing from before it.
    let info = crate::probe::probe(Path::new("ffprobe"), &clip)
        .await
        .unwrap();
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        ..Settings::default()
    };
    let mut p = plan(clip.clone(), &info, &copy, Hardware::Software).unwrap();
    p.keyframes = Some(Arc::new(
        keyframes(Path::new("ffprobe"), &clip, p.video, p.duration)
            .await
            .unwrap(),
    ));
    let t = Transcoder::new(
        "ffmpeg".into(),
        Hardware::Software,
        dir.path().join("alone"),
        4,
    );
    let segment = t.segment(b"alone", &p, 4).await.unwrap();
    let packets = video_packets(&segment, None).len();
    let decoded = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&segment)
        .args(["-map", "0:v", "-f", "framecrc", "-"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&decoded.stderr);
    assert!(!log.contains("non-existing PPS"), "{log}");
    let frames = String::from_utf8_lossy(&decoded.stdout)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .count();
    // At most the few frames shown before the keyframe, which need the segment before, go.
    assert!(
        frames + 4 >= packets,
        "{frames} of {packets} frames decoded"
    );
}

/// A copy of `source`'s video for a player that takes `takes`, asking for `channels`.
fn copy_for(takes: &[&str], channels: i64) -> Settings {
    Settings {
        video_codec: Some(VideoCodec::Copy),
        audio_channels: Some(channels),
        audio_codecs: Some(takes.iter().map(|c| c.to_string()).collect()),
        ..Settings::default()
    }
}

/// An HEVC (fMP4) or H.264 (MPEG-TS) source whose default audio is `codec` with `channels`.
fn with_audio(video: &str, codec: &str, profile: Option<&str>, channels: i64) -> MediaInfo {
    let mut source = info(video, 2160, "yuv420p10le", None);
    source.audio[1].codec = codec.into();
    source.audio[1].profile = profile.map(Into::into);
    source.audio[1].channels = Some(channels);
    source
}

#[test]
fn copy_plans_the_audio_by_what_the_player_takes() {
    let audio = |source: MediaInfo, settings: Settings| {
        plan_for(&source, settings, Hardware::Software).audio_codec
    };
    let atmos = || with_audio("hevc", "eac3", None, 6);
    assert_eq!(audio(atmos(), copy_for(&["eac3"], 6)), AudioCodec::Copy);
    assert_eq!(audio(atmos(), copy_for(&["eac3"], 8)), AudioCodec::Copy);
    // A downmix, or a sound mode, changes the sound, so it's converted.
    assert_eq!(audio(atmos(), copy_for(&["eac3"], 2)), AudioCodec::Aac);
    let dialogue = Settings {
        audio_mode: Some(AudioMode::Dialogue),
        ..copy_for(&["eac3"], 6)
    };
    assert_eq!(audio(atmos(), dialogue), AudioCodec::Aac);
    assert_eq!(
        audio(atmos(), copy_for(&["aac", "ac3"], 6)),
        AudioCodec::Aac
    );
    let eac3_on_ts = with_audio("h264", "eac3", None, 6);
    assert_eq!(audio(eac3_on_ts, copy_for(&["eac3"], 6)), AudioCodec::Copy);

    let truehd = || with_audio("hevc", "truehd", Some("Dolby TrueHD + Dolby Atmos"), 8);
    let both = copy_for(&["eac3", "alac", "flac"], 8);
    assert_eq!(audio(truehd(), both), AudioCodec::Flac);
    assert_eq!(audio(truehd(), copy_for(&["alac"], 8)), AudioCodec::Alac);
    assert_eq!(audio(truehd(), copy_for(&["eac3"], 8)), AudioCodec::Aac);
    // MPEG-TS can't carry FLAC or ALAC.
    let truehd_on_ts = with_audio("h264", "truehd", None, 8);
    assert_eq!(
        audio(truehd_on_ts, copy_for(&["flac", "alac"], 8)),
        AudioCodec::Aac
    );
    let flac_on_ts = with_audio("h264", "flac", None, 2);
    assert_eq!(audio(flac_on_ts, copy_for(&["flac"], 2)), AudioCodec::Aac);
    let master_audio = with_audio("hevc", "dts", Some("DTS-HD MA + DTS:X"), 8);
    assert_eq!(
        audio(master_audio, copy_for(&["flac"], 8)),
        AudioCodec::Flac
    );
    let pcm = with_audio("hevc", "pcm_s24le", None, 2);
    assert_eq!(audio(pcm, copy_for(&["flac"], 2)), AudioCodec::Flac);
    let lossy_dts = with_audio("hevc", "dts", Some("DTS"), 6);
    assert_eq!(audio(lossy_dts, copy_for(&["flac"], 6)), AudioCodec::Aac);
    let flac = || with_audio("hevc", "flac", None, 2);
    assert_eq!(audio(flac(), copy_for(&["flac"], 2)), AudioCodec::Copy);
    // Kept lossless when a downmix is asked for.
    let stereo = audio(truehd(), copy_for(&["flac"], 2));
    assert_eq!(stereo, AudioCodec::Flac);

    // Without a list, and for converted video, it's AAC as ever.
    let copy = Settings {
        video_codec: Some(VideoCodec::Copy),
        audio_channels: Some(6),
        ..Settings::default()
    };
    assert_eq!(audio(atmos(), copy), AudioCodec::Aac);
    let hevc = Settings {
        video_codec: Some(VideoCodec::Hevc),
        ..copy_for(&["eac3"], 6)
    };
    assert_eq!(audio(atmos(), hevc), AudioCodec::Aac);
}

#[test]
fn audio_codecs_are_known_ones() {
    assert!(
        copy_for(&["aac", "ac3", "eac3", "flac", "alac"], 2)
            .validate()
            .is_ok()
    );
    assert!(copy_for(&["truehd"], 2).validate().is_err());
    assert!(copy_for(&["dts"], 2).validate().is_err());
    assert!(copy_for(&["EAC3"], 2).validate().is_err());
    assert!(copy_for(&["aac"; 11], 2).validate().is_err());
}

#[test]
fn copied_audio_is_copied_or_kept_lossless() {
    let audio_of = |source: MediaInfo, settings: Settings| {
        let p = plan_for(&source, settings, Hardware::Software);
        let a = args(&p, Hardware::Software, 0);
        (
            value_after(&a, "-c:a").map(str::to_owned),
            value_after(&a, "-ac").map(str::to_owned),
            a.iter().any(|x| x == "-af"),
        )
    };
    let atmos = with_audio("hevc", "eac3", None, 6);
    assert_eq!(
        audio_of(atmos, copy_for(&["eac3"], 6)),
        (Some("copy".into()), None, false)
    );
    let truehd = || with_audio("hevc", "truehd", None, 8);
    assert_eq!(
        audio_of(truehd(), copy_for(&["flac"], 8)),
        (Some("flac".into()), Some("8".into()), false)
    );
    assert_eq!(
        audio_of(truehd(), copy_for(&["alac"], 8)),
        (Some("alac".into()), Some("8".into()), false)
    );
    // A downmix to stereo keeps its filter.
    assert_eq!(
        audio_of(truehd(), copy_for(&["flac"], 2)),
        (Some("flac".into()), Some("2".into()), true)
    );
}

#[test]
fn dolby_vision_is_labelled_for_players_that_show_it() {
    let label = |source: MediaInfo, shows: Option<bool>| {
        let settings = Settings {
            video_codec: Some(VideoCodec::Copy),
            dolby_vision: shows,
            ..Settings::default()
        };
        let mut p = plan_for(&source, settings, Hardware::Software);
        p.keyframes = Some(Arc::new(Keyframes {
            starts: vec![0.0, 6.0],
            seek_past: 0.0005,
            audio_from: Vec::new(),
        }));
        let a = args(&p, Hardware::Software, 1);
        (
            value_after(&a, "-tag:v").map(str::to_owned),
            value_after(&a, "-strict").map(str::to_owned),
        )
    };
    let dolby_vision_tag = (Some("dvh1".to_owned()), Some("unofficial".to_owned()));
    let hvc1 = (Some("hvc1".to_owned()), None);
    let p8 = || dolby_vision(8, Some("hdr10"));
    assert_eq!(label(p8(), Some(true)), dolby_vision_tag);
    assert_eq!(label(p8(), Some(false)), hvc1);
    assert_eq!(label(p8(), None), hvc1);
    assert_eq!(
        label(dolby_vision(8, Some("hlg")), Some(true)),
        dolby_vision_tag
    );
    // Profile 5 has no other picture; profile 7's enhancement layer is Blu-ray's own.
    assert_eq!(label(dolby_vision(5, None), None), dolby_vision_tag);
    assert_eq!(label(dolby_vision(7, Some("hdr10")), Some(true)), hvc1);
    let hdr10 = info("hevc", 2160, "yuv420p10le", Some("HDR10"));
    assert_eq!(label(hdr10, Some(true)), hvc1);

    // Converted video is never labelled so.
    let hevc = Settings {
        video_codec: Some(VideoCodec::Hevc),
        dolby_vision: Some(true),
        ..Settings::default()
    };
    assert!(!plan_for(&p8(), hevc, Hardware::Software).dolby_vision);
}

/// All of a segment's packets, video and audio, as stream, display time and size.
fn all_packets(segment: &Path, init: Option<&Path>) -> Vec<String> {
    let file = match init {
        Some(init) => {
            let whole = segment.with_extension("all.mp4");
            let mut bytes = std::fs::read(init).unwrap();
            bytes.extend(std::fs::read(segment).unwrap());
            std::fs::write(&whole, bytes).unwrap();
            whole
        }
        None => segment.to_path_buf(),
    };
    let out = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries"])
        .args(["packet=stream_index,pts_time,size", "-of", "csv=p=0"])
        .arg(&file)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(Into::into)
        .collect()
}

/// Copies `clip`'s video and its AC-3 audio through to segment `n`, and again from `n` as
/// after a seek: the segment must be the same, audio included.
async fn copied_audio_lines_up(dir: &Path, clip: &Path, n: i64) {
    let ffprobe = Path::new("ffprobe");
    let info = crate::probe::probe(ffprobe, clip).await.unwrap();
    let copy = copy_for(&["ac3"], 2);
    let mut p = plan(clip.into(), &info, &copy, Hardware::Software).unwrap();
    assert_eq!(p.audio_codec, AudioCodec::Copy);
    p.keyframes = Some(Arc::new(
        keyframes(ffprobe, clip, p.video, p.duration).await.unwrap(),
    ));
    let name = clip.file_name().unwrap().to_str().unwrap();
    let t = Transcoder::new(
        "ffmpeg".into(),
        Hardware::Software,
        dir.join(format!("audio-{name}")),
        4,
    );
    let mut inits = Vec::new();
    let mut segments = Vec::new();
    for (session, from) in [(&b"through"[..], 0), (&b"seeked"[..], n)] {
        for i in from..=n {
            let segment = t.segment(session, &p, i).await.unwrap();
            if i == n {
                segments.push(segment);
            }
        }
        assert_eq!(t.running_start(session), Some(from));
        if p.fmp4 {
            let init = t.init(session, &p).await.unwrap();
            let bytes = std::fs::read(&init).unwrap();
            assert!(bytes.windows(4).any(|w| w == b"ac-3"), "{name}: AC-3 kept");
            inits.push(init);
        }
    }
    let packets = |i: usize| all_packets(&segments[i], inits.get(i).map(PathBuf::as_path));
    let (through, seeked) = (packets(0), packets(1));
    assert!(
        through.iter().any(|p| p.starts_with("1,")),
        "{name}: audio in segment {n}"
    );
    // The same packets. Matroska keeps times in milliseconds, and ffmpeg works out the exact
    // ones differently after a seek, so they may differ by less than one.
    let fields = |p: &String| {
        let f: Vec<String> = p.split(',').map(Into::into).collect();
        let pts: f64 = f[1].parse().unwrap();
        (f[0].clone(), pts, f[2].clone())
    };
    assert_eq!(seeked.len(), through.len(), "{name}: segment {n}'s packets");
    for (a, b) in seeked.iter().map(fields).zip(through.iter().map(fields)) {
        assert!(
            a.0 == b.0 && a.2 == b.2 && (a.1 - b.1).abs() < 0.001,
            "{name}: segment {n} after a seek has {a:?} where the run from the start has {b:?}"
        );
    }
}

#[tokio::test]
async fn copied_audio_lines_up_after_a_seek() {
    let dir = tempfile::tempdir().unwrap();
    // HEVC in MP4 keeps decode times; H.264 in Matroska has them worked out.
    let hevc = b_frame_clip(
        dir.path(),
        "hevc.mp4",
        &[
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-x265-params",
            "keyint=70:min-keyint=70:scenecut=0:bframes=3:log-level=error",
        ],
    );
    copied_audio_lines_up(dir.path(), &hevc, 5).await;
    let h264 = b_frame_clip(
        dir.path(),
        "b-frames.mkv",
        &[
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-g",
            "50",
            "-sc_threshold",
            "0",
            "-bf",
            "3",
        ],
    );
    copied_audio_lines_up(dir.path(), &h264, 6).await;
}

#[tokio::test]
async fn lossless_audio_stays_lossless_in_the_segments() {
    let dir = tempfile::tempdir().unwrap();
    // A keyframe every 2 s and audio in frames of 30 samples, so the run starting at segment 1
    // (6 s) cuts the frame its seek lands in to 6 samples.
    let clip = dir.path().join("pcm.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi"])
        .args(["-i", "testsrc=size=320x240:rate=24:duration=10"])
        .args(["-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=10:sample_rate=48000:samples_per_frame=30")
        .args(["-c:v", "libx265", "-preset", "ultrafast", "-x265-params"])
        .arg("keyint=48:min-keyint=48:scenecut=0:bframes=0:log-level=error")
        .args(["-c:a", "pcm_s16le"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success());
    let ffprobe = Path::new("ffprobe");
    let info = crate::probe::probe(ffprobe, &clip).await.unwrap();
    for (takes, entry) in [("flac", &b"dfLa"[..]), ("alac", &b"alac"[..])] {
        let mut p = plan(
            clip.clone(),
            &info,
            &copy_for(&[takes], 2),
            Hardware::Software,
        )
        .unwrap();
        p.keyframes = Some(Arc::new(
            keyframes(ffprobe, &clip, p.video, p.duration)
                .await
                .unwrap(),
        ));
        let t = Transcoder::new(
            "ffmpeg".into(),
            Hardware::Software,
            dir.path().join(format!("t-{takes}")),
            4,
        );
        assert_eq!(p.segment_starts()[1], 6.0);
        for n in [0, 1] {
            let session = [n as u8];
            let segment = t.segment(&session, &p, n).await.unwrap();
            let init = t.init(&session, &p).await.unwrap();
            let bytes = std::fs::read(&init).unwrap();
            assert!(
                bytes.windows(4).any(|w| w == entry),
                "{takes} in the init section"
            );
            let audio = all_packets(&segment, Some(&init));
            assert!(
                audio.iter().any(|p| p.starts_with("1,")),
                "{takes} audio in segment {n}"
            );
        }
    }
}

#[test]
fn a_copy_run_pauses_further_ahead_than_an_encoding_one() {
    assert_eq!(throttle_copy(AHEAD_PAUSE + 1, false), None, "still copying");
    assert_eq!(throttle_copy(101, false), Some(libc::SIGSTOP));
    assert_eq!(throttle_copy(49, true), Some(libc::SIGCONT));
    assert_eq!(throttle_copy(75, true), None, "between the two: leave it");
}
