//! The ffmpeg command line for a plan: video on the GPU or the CPU, audio, HLS, copying, and
//! burned-in subtitles.

use super::*;

/// What the ffmpeg in use can do beyond plain ffmpeg.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Abilities {
    /// jellyfin-ffmpeg's CPU tone mapper, which applies Dolby Vision metadata.
    pub tonemapx: bool,
}

impl Abilities {
    /// Asks an ffmpeg which of these it has; none when it can't be asked.
    pub fn of(ffmpeg: &Path) -> Self {
        let filters = std::process::Command::new(ffmpeg)
            .args(["-hide_banner", "-filters"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let listed = filters.map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
        Self {
            tonemapx: listed.is_ok_and(|l| l.contains(" tonemapx ")),
        }
    }
}

/// HDR to SDR on the CPU. Dolby Vision with no HDR10 picture (profile 5) needs its metadata
/// applied, which only `tonemapx` does; everything else keeps zscale's tone mapping.
pub(super) fn software_tonemap(plan: &Plan, abilities: Abilities) -> &'static str {
    if plan.dolby_vision_only && abilities.tonemapx {
        "tonemapx=tonemap=bt2390:desat=0:peak=100:t=bt709:m=bt709:p=bt709:r=tv"
    } else {
        "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv"
    }
}

/// [`ffmpeg_args_for`] with plain ffmpeg.
#[cfg(test)]
pub fn ffmpeg_args(plan: &Plan, hw: Hardware, start: i64, dir: &Path) -> Vec<OsString> {
    ffmpeg_args_for(plan, hw, Abilities::default(), start, dir)
}

/// The ffmpeg command line for a plan, starting at segment `start`, writing into `dir`.
pub fn ffmpeg_args_for(
    plan: &Plan,
    hw: Hardware,
    abilities: Abilities,
    start: i64,
    dir: &Path,
) -> Vec<OsString> {
    if plan.codec == VideoCodec::Copy {
        return copy_args(plan, start, dir);
    }
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&["-hide_banner", "-nostdin", "-v", "error", "-y"]);
    if plan.gpu_decode {
        push(&["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
    }
    let offset = format!("{:.3}", start as f64 * SEGMENT_SECONDS);
    if start > 0 {
        push(&["-ss", &offset]);
    }
    push(&["-i"]);
    let mut input = OsString::from("file:");
    input.push(&plan.input);
    args.push(input);

    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    match &plan.burn {
        None => push(&["-map", &format!("0:{}", plan.video)]),
        Some(burn) => push(&[
            "-filter_complex",
            &burn_graph(plan, burn, hw, abilities, &offset, start),
            "-map",
            "[out]",
        ]),
    }
    if let Some(audio) = plan.audio {
        push(&["-map", &format!("0:{audio}")]);
    }
    push(&["-sn", "-dn", "-max_muxing_queue_size", "2048"]);

    let filters = video_filters(plan, hw, abilities);
    if !filters.is_empty() && plan.burn.is_none() {
        push(&["-vf", &filters.join(",")]);
    }

    video_encoder_args(plan, hw, &mut args);
    audio_args(plan, &mut args);
    let segment_seconds = (SEGMENT_SECONDS as i64).to_string();
    let offset = match plan.fmp4 {
        true => Some(format!(
            "{:.3}",
            FMP4_START + start as f64 * SEGMENT_SECONDS
        )),
        false => (start > 0).then_some(offset),
    };
    hls_args(
        plan,
        &segment_seconds,
        start,
        offset.as_deref(),
        dir,
        "seg",
        &mut args,
    );
    args
}

/// The video filters: upload to the GPU, tone mapping, scaling and the pixel format.
pub(super) fn video_filters(plan: &Plan, hw: Hardware, abilities: Abilities) -> Vec<String> {
    let mut filters: Vec<String> = Vec::new();
    match hw {
        Hardware::Nvenc => {
            if !plan.gpu_decode {
                // tonemap_cuda and hevc_nvenc won't take 10-bit frames uploaded as yuv420p10le.
                if plan.tonemap || plan.ten_bit {
                    filters.push("format=p010le".into());
                }
                filters.push("hwupload_cuda".into());
            }
            if plan.tonemap {
                filters.push(
                    "tonemap_cuda=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0"
                        .into(),
                );
            }
            let format = if plan.ten_bit { "p010le" } else { "nv12" };
            match plan.height {
                Some(h) => filters.push(format!("scale_cuda=w=-2:h={h}:format={format}")),
                None if plan.high_bit_depth && !plan.tonemap && !plan.ten_bit => {
                    filters.push("scale_cuda=format=nv12".into())
                }
                None => {}
            }
        }
        Hardware::Software => {
            if plan.tonemap {
                filters.push(software_tonemap(plan, abilities).into());
            }
            if let Some(h) = plan.height {
                filters.push(format!("scale=-2:{h}"));
            }
            filters.push(
                if plan.ten_bit {
                    "format=yuv420p10le"
                } else {
                    "format=yuv420p"
                }
                .into(),
            );
        }
    }
    filters
}

/// The video encoder, its profile and bitrate, and keyframes on segment boundaries.
pub(super) fn video_encoder_args(plan: &Plan, hw: Hardware, args: &mut Vec<OsString>) {
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    let bitrate = plan.bitrate.to_string();
    let bufsize = (plan.bitrate * 2).to_string();
    let hevc = plan.codec == VideoCodec::Hevc;
    match (hw, hevc) {
        (Hardware::Nvenc, false) => {
            push(&["-c:v", "h264_nvenc", "-preset", "p4", "-forced-idr", "1"])
        }
        (Hardware::Nvenc, true) => {
            push(&["-c:v", "hevc_nvenc", "-preset", "p4", "-forced-idr", "1"])
        }
        (Hardware::Software, false) => push(&["-c:v", "libx264", "-preset", "veryfast"]),
        (Hardware::Software, true) => push(&[
            "-c:v",
            "libx265",
            "-preset",
            "veryfast",
            "-forced-idr",
            "1",
            // Plain HDR10, as from NVENC, rather than Dolby Vision metadata carried over.
            "-dolbyvision",
            "0",
            "-x265-params",
            "log-level=error",
        ]),
    }
    if hevc {
        // Apple players want the parameter sets in the sample entry, tagged hvc1.
        push(&["-tag:v", "hvc1"]);
    }
    let profile = match (hevc, plan.ten_bit) {
        (false, _) => "high",
        (true, false) => "main",
        (true, true) => "main10",
    };
    push(&[
        "-profile:v",
        profile,
        "-b:v",
        &bitrate,
        "-maxrate",
        &bitrate,
        "-bufsize",
        &bufsize,
    ]);
    // Keyframes on segment boundaries, so every segment starts cleanly and a restart at any
    // segment lines up with the playlist.
    let keyframes = format!("expr:gte(t,n_forced*{})", SEGMENT_SECONDS as i64);
    push(&["-force_key_frames", &keyframes]);
}

pub(super) fn audio_args(plan: &Plan, args: &mut Vec<OsString>) {
    if plan.audio.is_some() {
        if let Some(filter) = &plan.audio_filter {
            args.extend(["-af", filter].map(OsString::from));
        }
        let audio_bitrate = if plan.channels > 2 { "384k" } else { "192k" };
        let channels = plan.channels.to_string();
        let items = ["-c:a", "aac", "-ac", &channels, "-b:a", audio_bitrate];
        args.extend(items.map(OsString::from));
    }
}

/// The HLS muxer's options: segments of `hls_time` seconds named `{prefix}{n}`, numbered from
/// `start_number`, timestamps moved by `offset` (-ss resets them to 0).
pub(super) fn hls_args(
    plan: &Plan,
    hls_time: &str,
    start_number: i64,
    offset: Option<&str>,
    dir: &Path,
    prefix: &str,
    args: &mut Vec<OsString>,
) {
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    let start_number = start_number.to_string();
    // Negative timestamps at the start (AAC priming, B-frames) would otherwise make the
    // muxers, the HLS one and the segments' own, shift a run from the start, and only that
    // one, out of line with runs started later.
    push(&["-avoid_negative_ts", "disabled"]);
    if plan.fmp4 {
        // Fragments carry their real times and the init section has no edit list, so every
        // run of a session writes the same init section. Without a sidx, the MP4 muxer also
        // leaves the first frame of each fragment alone; with one, it rewrites that frame's
        // time to follow the previous fragment's last, which is wrong when frames shown before
        // a keyframe come after it (open-GOP HEVC), moving every segment's first keyframe
        // (libavformat/movenc.c, ff_mov_write_packet).
        push(&[
            "-hls_segment_options",
            "avoid_negative_ts=disabled:use_editlist=0:movflags=+skip_sidx+frag_discont",
        ]);
    } else {
        push(&["-hls_segment_options", "avoid_negative_ts=disabled"]);
    }
    push(&["-f", "hls", "-hls_time", hls_time, "-hls_list_size", "0"]);
    if plan.fmp4 {
        push(&[
            "-hls_segment_type",
            "fmp4",
            "-hls_fmp4_init_filename",
            "init.mp4",
        ]);
    } else {
        push(&["-hls_segment_type", "mpegts"]);
    }
    push(&["-start_number", &start_number]);
    if let Some(offset) = offset {
        // Put timestamps back where the playlist expects them.
        push(&["-output_ts_offset", offset]);
    }
    push(&["-hls_segment_filename"]);
    args.push(dir.join(format!("{prefix}%d.{}", plan.extension())).into());
    args.push(dir.join("ffmpeg.m3u8").into());
}

/// Copies the video and converts the audio, writing a chunk per keyframe (`c{i}`, numbered from
/// 0 in every run); see [`Running::chunks`].
pub(super) fn copy_args(plan: &Plan, start: i64, dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    // The file's own timestamps in every run, so runs line up exactly: putting them back after
    // -ss, as encoding does, rounds to the stream's time base.
    push(&["-hide_banner", "-nostdin", "-v", "error", "-y", "-copyts"]);
    // Just past the keyframe the segment starts at, so ffmpeg lands on it. Audio before that
    // point is dropped, so the run's first chunk starts at the keyframe too.
    let seek = plan
        .keyframes
        .as_ref()
        .and_then(|k| Some(k.starts.get(usize::try_from(start).ok()?)? + k.seek_past))
        .filter(|_| start > 0)
        .map(|at| format!("{at:.6}"));
    if let Some(seek) = &seek {
        push(&["-ss", seek]);
    }
    push(&["-i"]);
    let mut input = OsString::from("file:");
    input.push(&plan.input);
    args.push(input);
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&["-map", &format!("0:{}", plan.video)]);
    if let Some(audio) = plan.audio {
        push(&["-map", &format!("0:{audio}")]);
    }
    push(&[
        "-sn",
        "-dn",
        "-max_muxing_queue_size",
        "2048",
        "-c:v",
        "copy",
    ]);
    if plan.fmp4 {
        push(&[
            "-tag:v",
            if plan.dolby_vision_only {
                "dvh1"
            } else {
                "hvc1"
            },
        ]);
    } else {
        // MPEG-TS carries H.264's parameter sets in the stream, and the conversion puts them
        // before IDR frames only: a segment starting on an open-GOP keyframe would have none,
        // and couldn't be decoded after a seek. fMP4 has them in the init section.
        push(&["-bsf:v", "h264_mp4toannexb,dump_extra=freq=keyframe"]);
    }
    audio_args(plan, &mut args);
    // A split at every keyframe: each is past any target this small.
    let offset = plan.fmp4.then(|| format!("{FMP4_START:.3}"));
    hls_args(plan, "0.001", 0, offset.as_deref(), dir, "c", &mut args);
    args
}

/// The filter graph for burning subtitles in. Tone mapping and scaling stay on the GPU with
/// NVENC; frames come back to the CPU only to draw the subtitles.
pub(super) fn burn_graph(
    plan: &Plan,
    burn: &Burn,
    hw: Hardware,
    abilities: Abilities,
    offset: &str,
    start: i64,
) -> String {
    let mut before: Vec<String> = Vec::new();
    let encoder_format = match hw {
        Hardware::Nvenc => {
            let mut gpu: Vec<String> = Vec::new();
            if plan.tonemap {
                gpu.push(
                    "tonemap_cuda=format=nv12:p=bt709:t=bt709:m=bt709:tonemap=bt2390:peak=100:desat=0"
                        .into(),
                );
            }
            match plan.height {
                Some(h) => gpu.push(format!("scale_cuda=w=-2:h={h}:format=nv12")),
                None if plan.high_bit_depth && !plan.tonemap => {
                    gpu.push("scale_cuda=format=nv12".into())
                }
                None => {}
            }
            if !gpu.is_empty() {
                // tonemap_cuda won't take 10-bit frames uploaded as yuv420p10le.
                if plan.tonemap {
                    before.push("format=p010le".into());
                }
                before.push("hwupload_cuda".into());
                before.extend(gpu);
                before.extend(["hwdownload".into(), "format=nv12".into()]);
            }
            "format=nv12"
        }
        Hardware::Software => {
            if plan.tonemap {
                before.push(software_tonemap(plan, abilities).into());
            }
            if let Some(h) = plan.height {
                before.push(format!("scale=-2:{h}"));
            }
            "format=yuv420p"
        }
    };
    let video = format!("[0:{}]", plan.video);
    match burn {
        Burn::Picture {
            index,
            width,
            height,
        } => {
            let main = if before.is_empty() {
                video
            } else {
                format!("{video}{}[main];[main]", before.join(","))
            };
            format!(
                // Fitted inside the frame with its shape kept, centred and on the bottom edge:
                // a disc's 1920x1080 subtitles on a 1920x800 scope encode aren't squashed.
                "[0:{index}]scale={width}:{height}:force_original_aspect_ratio=decrease[sub];{main}[sub]overlay=x=(W-w)/2:y=H-h:eof_action=pass:repeatlast=0,{encoder_format}[out]"
            )
        }
        Burn::Text { ass, fonts } => {
            // -ss resets timestamps to 0; the subtitles need the file's own times.
            let mut chain = before;
            if start > 0 {
                chain.push(format!("setpts=PTS+{offset}/TB"));
            }
            let fonts = fonts
                .as_ref()
                .map(|dir| format!(":fontsdir={}", dir.display()))
                .unwrap_or_default();
            chain.push(format!("subtitles=filename={}{fonts}", ass.display()));
            if start > 0 {
                chain.push(format!("setpts=PTS-{offset}/TB"));
            }
            chain.push(encoder_format.into());
            format!("{video}{}[out]", chain.join(","))
        }
    }
}
