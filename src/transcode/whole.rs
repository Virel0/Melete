//! A whole file converted for keeping (offline copies): one MP4 with its index at the front, so
//! a player can start it before it's all there.

use super::*;

/// The ffmpeg command line converting a whole file as `plan` says into one MP4 at `out`,
/// writing its progress to standard output.
pub fn whole_file_args(
    plan: &Plan,
    hw: Hardware,
    abilities: Abilities,
    out: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&[
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-y",
        "-progress",
        "pipe:1",
        "-nostats",
    ]);
    let gpu = plan.gpu_decode && hw == Hardware::Nvenc && plan.codec != VideoCodec::Copy;
    if gpu {
        push(&["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"]);
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
    push(&["-sn", "-dn", "-max_muxing_queue_size", "2048"]);
    if plan.codec == VideoCodec::Copy {
        push(&["-c:v", "copy"]);
        if plan.fmp4 {
            copied_hevc_tag(plan, &mut args);
        }
    } else {
        let mut plan = plan.clone();
        plan.gpu_decode = gpu;
        let filters = video_filters(&plan, hw, abilities);
        if !filters.is_empty() {
            push(&["-vf", &filters.join(",")]);
        }
        video_encoder_args(&plan, hw, &mut args);
    }
    audio_args(plan, &mut args);
    args.extend(["-movflags", "+faststart", "-f", "mp4"].map(OsString::from));
    args.push(out.into());
    args
}

/// How far along a run writing `-progress` is, from 0 to 1, given the file's length: ffmpeg
/// reports `out_time_us=` as it goes.
pub fn progress_of(line: &str, duration: f64) -> Option<f64> {
    let micros: f64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    (duration > 0.0).then(|| (micros / 1e6 / duration).clamp(0.0, 1.0))
}

impl Transcoder {
    /// What the ffmpeg in use can do beyond plain ffmpeg.
    pub fn abilities(&self) -> Abilities {
        self.abilities
    }

    /// How many runs are going now, and how many may.
    pub fn load(&self) -> (usize, usize) {
        let running = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        (running, self.max_running)
    }
}
