//! HLS transcoding with ffmpeg: NVENC on the GPU, or libx264 on the CPU.
//!
//! The playlist lists the whole file in fixed segments up front, so players know its length and
//! can seek anywhere. ffmpeg encodes ahead of the viewer; a request outside what it has made, or
//! is about to make, restarts it there. It is paused when far ahead, segments the viewer has
//! passed are deleted, and an idle session's ffmpeg is stopped.
//!
//! Output is H.264 in MPEG-TS, HEVC in fragmented MP4 (keeping HDR10 and HLG), or the file's own
//! video copied unchanged with only the audio converted. Copied video can only be cut at its own
//! keyframes, so its playlist lists keyframes found through the file's index (see
//! [`keyframes`]); ffmpeg writes a chunk per keyframe and a segment is put together from the
//! chunks between two of them.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

use crate::probe::{MediaInfo, Picture};

mod args;
mod audio;
mod gpu;
mod keyframes;
mod plan;
mod running;
mod settings;
mod whole;

pub use args::*;
pub use audio::*;
pub use gpu::*;
pub use keyframes::*;
pub use plan::*;
pub use running::*;
pub use settings::*;
pub use whole::*;

/// How long each segment of an encoded (not copied) video is, in seconds.
pub const SEGMENT_SECONDS: f64 = 6.0;
/// Pause ffmpeg this many segments (3 minutes) ahead of the viewer…
const AHEAD_PAUSE: i64 = 30;
/// …and let it carry on once the viewer is within this many.
const AHEAD_RESUME: i64 = 15;
/// Segments kept behind the viewer at most: ten minutes of six-second segments, so going back
/// plays at once instead of starting ffmpeg again…
const KEEP_BEHIND: i64 = 100;
/// …and the most they may take on disk for one viewer, whatever their length (a remux's
/// segments are large).
const KEEP_BEHIND_BYTES: u64 = 1 << 30;
/// A request this close past what's made waits for ffmpeg instead of restarting it.
const WAIT_AHEAD: i64 = 4;
const IDLE: Duration = Duration::from_secs(60);
/// A transcode not asked for in this long gives up its slot to a new one when all are taken.
/// A viewer who's playing asks every few seconds; one who left the player stops asking, and
/// one who paused gets ffmpeg started again where they are when they play on.
const TAKE_OVER_AFTER: Duration = Duration::from_secs(20);
const SEGMENT_WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(200);
/// Codecs NVDEC decodes on current NVIDIA cards (10-bit H.264 excepted).
const NVDEC_CODECS: &[&str] = &[
    "h264",
    "hevc",
    "av1",
    "vp9",
    "vp8",
    "mpeg2video",
    "vc1",
    "mpeg4",
];

/// Where video is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hardware {
    /// On an NVIDIA GPU, with NVENC (and NVDEC where the codec allows).
    Nvenc,
    /// On the CPU, with libx264 or libx265.
    Software,
}

impl Hardware {
    /// The hardware `MELETE_HWACCEL` names: "nvenc" or "none"; None for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "nvenc" => Some(Self::Nvenc),
            "none" => Some(Self::Software),
            _ => None,
        }
    }
}

/// fMP4 timestamps start this far in, so none is negative (B-frames, AAC priming): fragments
/// carry their real times, which can't be negative, and no edit list is needed.
const FMP4_START: f64 = 10.0;

/// Folders the transcoder makes end in this, and it deletes nothing else.
const RUN_SUFFIX: &str = ".hls";

/// Refuses a transcode folder that overlaps the data folder or a library: the transcoder
/// deletes folders in it.
pub fn check_dir(dir: &Path, protected: &[&Path]) -> anyhow::Result<()> {
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = absolute(dir);
    for other in protected {
        let other = absolute(other);
        if dir.starts_with(&other) || other.starts_with(&dir) {
            anyhow::bail!(
                "MELETE_TRANSCODE_DIR {} overlaps {}; give transcoding its own folder",
                dir.display(),
                other.display()
            );
        }
    }
    Ok(())
}

/// Running transcodes, keyed by the SHA-256 of their stream token, which is never reused.
pub struct Transcoder {
    ffmpeg: PathBuf,
    hw: Hardware,
    abilities: Abilities,
    root: PathBuf,
    max_running: usize,
    running: Mutex<HashMap<Vec<u8>, Running>>,
    /// Whether the GPU passed its latest test encode, when it's set up.
    gpu: Mutex<GpuState>,
    /// Names each ffmpeg run's folder, so a restart never shares one with the run it replaces.
    runs: AtomicU64,
    /// Segments, and bytes, kept behind a viewer at most ([`KEEP_BEHIND`],
    /// [`KEEP_BEHIND_BYTES`]).
    keep_segments: AtomicI64,
    keep_bytes: AtomicU64,
}

impl Transcoder {
    /// Clears out segments a previous run of the server left, and only those.
    pub fn new(ffmpeg: PathBuf, hw: Hardware, root: PathBuf, max_running: usize) -> Arc<Self> {
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let ours = entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.ends_with(RUN_SUFFIX));
                if ours && entry.file_type().is_ok_and(|t| t.is_dir()) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        Arc::new(Self {
            abilities: Abilities::of(&ffmpeg),
            ffmpeg,
            hw,
            root,
            max_running,
            running: Mutex::new(HashMap::new()),
            gpu: Mutex::default(),
            runs: AtomicU64::new(0),
            keep_segments: AtomicI64::new(KEEP_BEHIND),
            keep_bytes: AtomicU64::new(KEEP_BEHIND_BYTES),
        })
    }

    /// Other limits on what's kept behind a viewer, for tests.
    #[cfg(test)]
    pub(super) fn keep_behind(&self, segments: i64, bytes: u64) {
        self.keep_segments.store(segments, Ordering::Relaxed);
        self.keep_bytes.store(bytes, Ordering::Relaxed);
    }

    /// The ffmpeg binary it runs.
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }

    /// Whether any video is being transcoded now: the stem separator, which shares the GPU,
    /// waits its turn.
    pub fn busy(&self) -> bool {
        !self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// The transcode folder; music conversions keep a folder of their own inside it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Pauses, resumes and stops ffmpeg processes every couple of seconds, and checks the GPU
    /// when that's due.
    pub fn spawn_supervisor(self: &Arc<Self>) {
        let transcoder = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tick.tick().await;
                let Some(transcoder) = transcoder.upgrade() else {
                    return;
                };
                transcoder.sweep(IDLE);
                transcoder.check_gpu_if_due();
            }
        });
    }

    fn sweep(&self, idle: Duration) {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        let stale: Vec<Vec<u8>> = running
            .iter()
            .filter(|(_, r)| r.last_request.elapsed() >= idle)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(r) = running.remove(&id) {
                r.stop();
            }
        }
        for r in running.values_mut() {
            let progress = r.progress();
            let finished = progress.finished.unwrap_or(r.start - 1);
            let ahead = finished - r.last_segment;
            let signal = if r.keyframes.is_some() {
                throttle_copy(ahead, r.paused)
            } else {
                throttle(ahead, r.paused)
            };
            if let Some(signal) = signal {
                r.signal(signal);
                r.paused = signal == libc::SIGSTOP;
            }
            let keep_from = keep_from(
                r,
                &progress.chunks,
                self.keep_segments.load(Ordering::Relaxed),
                self.keep_bytes.load(Ordering::Relaxed),
            );
            let passed = r.deleted_below.max(r.start)..keep_from;
            for n in passed.clone() {
                let _ = std::fs::remove_file(r.dir.join(format!("seg{n}.{}", r.extension)));
            }
            for (_, chunk) in progress.chunks.iter().filter(|(n, _)| passed.contains(n)) {
                let _ = std::fs::remove_file(r.dir.join(chunk));
            }
            r.deleted_below = r.deleted_below.max(keep_from);
        }
    }

    /// Waits for segment `n` of a session and returns its file, starting or restarting ffmpeg
    /// as needed.
    pub async fn segment(
        &self,
        session: &[u8],
        plan: &Plan,
        n: i64,
    ) -> Result<PathBuf, TranscodeError> {
        enum Next {
            Wait,
            Restart,
            Failed(String),
            /// A run on the GPU failed before finishing a segment: try it on the CPU.
            OnCpu(String),
            /// A copy run's segment, to put together from these chunks.
            Assemble(Vec<PathBuf>, PathBuf),
        }
        let deadline = Instant::now() + SEGMENT_WAIT;
        loop {
            let assemble_from = {
                let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
                let next = match running.get_mut(session) {
                    Some(r) if n >= r.start => {
                        r.last_request = Instant::now();
                        r.last_segment = n;
                        if r.paused {
                            r.signal(libc::SIGCONT);
                            r.paused = false;
                        }
                        let path = r.dir.join(format!("seg{n}.{}", r.extension));
                        let progress = r.progress();
                        let finished = progress.finished;
                        if finished.is_some_and(|f| f >= n) && path.exists() {
                            return Ok(path);
                        }
                        let chunks: Vec<PathBuf> = progress
                            .chunks
                            .iter()
                            .filter(|(segment, _)| *segment == n)
                            .map(|(_, chunk)| r.dir.join(chunk))
                            .collect();
                        let assemble = r.keyframes.is_some()
                            && finished.is_some_and(|f| f >= n)
                            && n >= r.deleted_below
                            && !chunks.is_empty();
                        match r.exited() {
                            _ if assemble => Next::Assemble(chunks, path),
                            Some(Err(log)) if r.hw == Hardware::Nvenc && finished.is_none() => {
                                Next::OnCpu(log)
                            }
                            Some(Err(log)) => Next::Failed(log),
                            // Finished cleanly without it: the segment was deleted.
                            Some(Ok(())) => Next::Restart,
                            None => {
                                let frontier = finished.map_or(r.start, |f| f + 1);
                                // Starting again at the segment is quicker than waiting for
                                // ffmpeg to get there, even for a copy (measured).
                                if n > frontier + WAIT_AHEAD || n < r.deleted_below {
                                    Next::Restart
                                } else {
                                    Next::Wait
                                }
                            }
                        }
                    }
                    // Nothing running for this session, or the viewer went back before it.
                    _ => Next::Restart,
                };
                match next {
                    Next::Wait => None,
                    Next::Restart => {
                        self.start(&mut running, session, plan, n, false)?;
                        None
                    }
                    Next::OnCpu(log) => {
                        self.retry_on_cpu(&mut running, session, plan, n, &log)?;
                        None
                    }
                    Next::Failed(log) => {
                        // Drop the dead run, so the next request starts afresh instead of
                        // finding it again (and it stops holding a slot).
                        if let Some(dead) = running.remove(session) {
                            if dead.hw == Hardware::Nvenc {
                                self.ask_gpu_check();
                            }
                            dead.stop();
                        }
                        return Err(TranscodeError::Failed(log));
                    }
                    Next::Assemble(chunks, path) => Some((chunks, path)),
                }
            };
            if let Some((chunks, path)) = assemble_from {
                // Outside the lock: a remux's segment can be tens of megabytes. If the
                // supervisor deleted a chunk meanwhile, the next round restarts ffmpeg.
                match assemble(&chunks, &path).await {
                    Ok(()) => return Ok(path),
                    Err(err) => tracing::debug!(error = %err, "putting a segment together failed"),
                }
            }
            if Instant::now() >= deadline {
                return Err(TranscodeError::TimedOut);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Waits for a session's fMP4 init section, starting ffmpeg at the first segment if nothing
    /// runs. Every run of a session writes the same one.
    pub async fn init(&self, session: &[u8], plan: &Plan) -> Result<PathBuf, TranscodeError> {
        let deadline = Instant::now() + SEGMENT_WAIT;
        loop {
            {
                let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
                match running.get_mut(session) {
                    Some(r) => {
                        r.last_request = Instant::now();
                        let path = r.dir.join("init.mp4");
                        // Written with the first segment, before that is listed.
                        let listed = std::fs::read_to_string(r.dir.join("ffmpeg.m3u8"))
                            .is_ok_and(|p| p.lines().any(|l| !l.is_empty() && !l.starts_with('#')));
                        if listed && path.exists() {
                            return Ok(path);
                        }
                        match r.exited() {
                            Some(Err(log)) if r.hw == Hardware::Nvenc && !listed => {
                                let start = r.start;
                                self.retry_on_cpu(&mut running, session, plan, start, &log)?;
                            }
                            Some(Err(log)) => {
                                if let Some(dead) = running.remove(session) {
                                    dead.stop();
                                }
                                return Err(TranscodeError::Failed(log));
                            }
                            Some(Ok(())) => {
                                let start = r.start;
                                self.start(&mut running, session, plan, start, false)?;
                            }
                            None => {}
                        }
                    }
                    None => self.start(&mut running, session, plan, 0, false)?,
                }
            }
            if Instant::now() >= deadline {
                return Err(TranscodeError::TimedOut);
            }
            tokio::time::sleep(POLL).await;
        }
    }

    /// Starts a session's run again on the CPU after it failed on the GPU before finishing a
    /// segment, and has the GPU checked. A session that fell back stays on the CPU.
    fn retry_on_cpu(
        &self,
        running: &mut HashMap<Vec<u8>, Running>,
        session: &[u8],
        plan: &Plan,
        n: i64,
        log: &str,
    ) -> Result<(), TranscodeError> {
        tracing::warn!(log, "transcoding on the GPU failed; trying the CPU");
        self.ask_gpu_check();
        self.start(running, session, plan, n, true)
    }

    /// Starts ffmpeg for a session at segment `n`, in place of any run it has, on the CPU when
    /// `on_cpu`, the session fell back before, or the GPU isn't working. When every slot is
    /// taken, the run asked for least recently gives up its slot if it has been quiet for
    /// [`TAKE_OVER_AFTER`]; otherwise it's [`TranscodeError::Busy`].
    fn start(
        &self,
        running: &mut HashMap<Vec<u8>, Running>,
        session: &[u8],
        plan: &Plan,
        n: i64,
        on_cpu: bool,
    ) -> Result<(), TranscodeError> {
        let mut on_cpu = on_cpu;
        if let Some(old) = running.remove(session) {
            on_cpu |= old.on_cpu;
            old.stop();
        }
        if running.len() >= self.max_running {
            take_quietest_slot(running)?;
        }
        let keyframes = match plan.codec {
            VideoCodec::Copy => Some(plan.keyframes.clone().ok_or_else(|| {
                TranscodeError::Failed("copying needs the file's keyframes".into())
            })?),
            _ => None,
        };
        // Copied video isn't encoded, so it never needs the GPU.
        let hw = match plan.codec {
            VideoCodec::Copy => Hardware::Software,
            _ if on_cpu => Hardware::Software,
            _ => self.hardware(),
        };
        let mut plan = plan.clone();
        plan.gpu_decode &= hw == Hardware::Nvenc;
        let run = self.runs.fetch_add(1, Ordering::Relaxed);
        let dir = self.root.join(format!("{run}{RUN_SUFFIX}"));
        let failed = |err: std::io::Error| TranscodeError::Failed(err.to_string());
        std::fs::create_dir_all(&dir).map_err(failed)?;
        let log = std::fs::File::create(dir.join("ffmpeg.log")).map_err(failed)?;
        let child = Command::new(&self.ffmpeg)
            .args(ffmpeg_args_for(&plan, hw, self.abilities, n, &dir))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| {
                TranscodeError::Failed(format!("starting {}: {err}", self.ffmpeg.display()))
            })?;
        running.insert(
            session.to_vec(),
            Running {
                child,
                start: n,
                dir,
                last_request: Instant::now(),
                last_segment: n,
                paused: false,
                deleted_below: n,
                extension: plan.extension(),
                keyframes,
                hw,
                on_cpu,
            },
        );
        Ok(())
    }

    #[cfg(test)]
    fn running_start(&self, session: &[u8]) -> Option<i64> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(session)
            .map(|r| r.start)
    }
}

#[cfg(test)]
mod tests;
