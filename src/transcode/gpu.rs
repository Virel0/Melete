//! Whether the GPU can encode. A short test encode runs at startup, again after a run fails on
//! the GPU, and every few minutes while the GPU is failing; until one passes, transcodes run on
//! the CPU instead of failing.

use super::*;

/// How often the GPU is tried again while it's failing.
pub(super) const GPU_RECHECK: Duration = Duration::from_secs(5 * 60);
/// A test encode that takes longer than this counts as failed.
const GPU_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest reason kept from a failed test encode.
const REASON_CHARS: usize = 300;

/// What the latest checks found about the GPU.
#[derive(Debug, Default)]
pub(super) struct GpuState {
    /// Why the latest test encode failed; None when it passed or none has run yet.
    pub(super) failed: Option<String>,
    /// When the latest check started; None before the first.
    pub(super) checked: Option<Instant>,
    /// A run failed on the GPU since the latest check, so the next tick checks again.
    pub(super) asked: bool,
    /// A check is running now.
    pub(super) checking: bool,
}

impl GpuState {
    /// Whether a check should start now: never run yet, asked for, or the GPU has been
    /// failing for [`GPU_RECHECK`].
    pub(super) fn due(&self) -> bool {
        !self.checking
            && match self.checked {
                None => true,
                Some(at) => self.asked || (self.failed.is_some() && at.elapsed() >= GPU_RECHECK),
            }
    }
}

/// The test encode: a second of generated video, marked as HDR, through the same GPU filters a
/// transcode uses (upload, tone mapping, scaling) into NVENC, and thrown away.
pub fn gpu_check_args() -> Vec<OsString> {
    let plan = Plan {
        input: PathBuf::new(),
        duration: 1.0,
        video: 0,
        gpu_decode: false,
        height: Some(144),
        tonemap: true,
        high_bit_depth: true,
        codec: VideoCodec::H264,
        ten_bit: false,
        fmp4: false,
        dolby_vision_only: false,
        dolby_vision: false,
        keyframes: None,
        bitrate: 500_000,
        audio: None,
        audio_codec: AudioCodec::Aac,
        channels: 2,
        burn: None,
        audio_filter: None,
    };
    let mut filters =
        vec!["setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc".to_owned()];
    filters.extend(video_filters(&plan, Hardware::Nvenc, Abilities::default()));
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=duration=1:size=320x240:rate=24",
        "-vf",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(filters.join(",").into());
    video_encoder_args(&plan, Hardware::Nvenc, &mut args);
    args.extend(["-f", "null", "-"].map(OsString::from));
    args
}

/// Runs the test encode; why it failed, in ffmpeg's words, when it did.
pub(super) async fn gpu_works(ffmpeg: &Path) -> Result<(), String> {
    let run = Command::new(ffmpeg)
        .args(gpu_check_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(GPU_CHECK_TIMEOUT, run).await {
        Err(_) => return Err("the test encode took longer than 30 seconds".into()),
        Ok(Err(err)) => return Err(format!("ffmpeg couldn't be started: {err}")),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    Err(reason(&String::from_utf8_lossy(&output.stderr)))
}

/// The first few lines ffmpeg wrote, where it says what went wrong first, without the
/// `[component @ 0x…]` each may start with.
fn reason(log: &str) -> String {
    let lines: Vec<&str> = log
        .lines()
        .map(|l| match l.trim().split_once("] ") {
            Some((context, rest)) if context.starts_with('[') => rest,
            _ => l.trim(),
        })
        .filter(|l| !l.is_empty())
        .take(3)
        .collect();
    if lines.is_empty() {
        return "ffmpeg failed without saying why".into();
    }
    lines.join("; ").chars().take(REASON_CHARS).collect()
}

impl Transcoder {
    /// Where new runs encode now: on the GPU when it's set up and passed its latest check (or
    /// none has run yet), else on the CPU.
    pub fn hardware(&self) -> Hardware {
        match self.hw {
            Hardware::Nvenc if self.gpu_problem().is_none() => Hardware::Nvenc,
            _ => Hardware::Software,
        }
    }

    /// Why video conversions use the CPU although the GPU is set up: its latest test encode
    /// failed. None when the GPU works, or isn't set up.
    pub fn gpu_problem(&self) -> Option<String> {
        if self.hw != Hardware::Nvenc {
            return None;
        }
        self.gpu
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .failed
            .clone()
    }

    /// Asks for the GPU to be checked again at the next tick, after a run failed on it.
    pub(super) fn ask_gpu_check(&self) {
        if self.hw == Hardware::Nvenc {
            self.gpu
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .asked = true;
        }
    }

    /// Starts a check in the background when one is due; see [`GpuState::due`].
    pub(super) fn check_gpu_if_due(self: &Arc<Self>) {
        if self.hw != Hardware::Nvenc {
            return;
        }
        {
            let mut gpu = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
            if !gpu.due() {
                return;
            }
            gpu.checking = true;
            gpu.asked = false;
            gpu.checked = Some(Instant::now());
        }
        let transcoder = self.clone();
        tokio::spawn(async move { transcoder.check_gpu().await });
    }

    /// Runs the test encode now and keeps what it found.
    pub async fn check_gpu(&self) {
        crate::tasks::started("gpu-check");
        let found = gpu_works(&self.ffmpeg).await;
        let said = match &found {
            Ok(()) => "the GPU encoder works",
            Err(_) => "the GPU encoder failed its test; using the CPU",
        };
        crate::tasks::finished("gpu-check", Some(said.into()), None);
        let mut gpu = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        match (&found, &gpu.failed) {
            (Err(reason), None) => {
                tracing::warn!(reason, "the GPU encoder failed its test; using the CPU")
            }
            (Ok(()), Some(_)) => tracing::info!("the GPU encoder works again"),
            _ => {}
        }
        gpu.failed = found.err();
        gpu.checking = false;
        gpu.checked.get_or_insert_with(Instant::now);
    }
}

/// A stand-in ffmpeg for tests.
#[cfg(test)]
pub mod testing {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// Writes an ffmpeg that fails whatever touches the GPU (NVENC, CUDA), as one in a
    /// container that lost its GPU does, and runs the real ffmpeg for everything else. Once a
    /// file named as it is plus `.ok` exists, GPU runs succeed without doing anything. It's
    /// kept under `target/`, as the system's temporary folder may not allow running programs;
    /// the folder goes when the returned guard does.
    pub fn ffmpeg_without_gpu() -> (tempfile::TempDir, PathBuf) {
        let target = concat!(env!("CARGO_MANIFEST_DIR"), "/target");
        std::fs::create_dir_all(target).unwrap();
        let dir = tempfile::tempdir_in(target).unwrap();
        let path = dir.path().join("ffmpeg-without-gpu");
        let script = r#"#!/bin/sh
for a in "$@"; do
  case "$a" in *nvenc*|*cuda*)
    [ -e "$0.ok" ] && exit 0
    echo "[h264_nvenc @ 0x1] Cannot load libcuda.so.1" >&2
    exit 1;;
  esac
done
exec ffmpeg "$@"
"#;
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }
}

#[cfg(test)]
mod tests;
