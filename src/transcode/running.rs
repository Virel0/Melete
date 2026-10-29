//! A running ffmpeg: its progress, the segments it has finished, chunks assembled into segments,
//! pausing it when far ahead, and which run gives up its slot when all are taken.

use super::*;

/// Why a segment or init section couldn't be served.
#[derive(Debug)]
pub enum TranscodeError {
    /// Too many transcodes running already.
    Busy,
    /// ffmpeg failed; the end of its log.
    Failed(String),
    /// The segment wasn't ready in time.
    TimedOut,
}

impl std::fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("too many transcodes are running"),
            Self::Failed(log) => write!(f, "ffmpeg failed: {log}"),
            Self::TimedOut => f.write_str("the segment wasn't ready in time"),
        }
    }
}

pub(super) struct Running {
    pub(super) child: Child,
    pub(super) start: i64,
    pub(super) dir: PathBuf,
    pub(super) last_request: Instant,
    pub(super) last_segment: i64,
    pub(super) paused: bool,
    /// Segments below this are already deleted.
    pub(super) deleted_below: i64,
    pub(super) extension: &'static str,
    /// For copied video: ffmpeg writes chunks, put together into segments starting here.
    pub(super) keyframes: Option<Arc<Keyframes>>,
    /// Where it encodes.
    pub(super) hw: Hardware,
    /// Its session fell back to the CPU after a run failed on the GPU, and stays there.
    pub(super) on_cpu: bool,
}

/// What a run has made so far.
pub(super) struct Progress {
    /// The highest segment finished.
    pub(super) finished: Option<i64>,
    /// For copied video, each finished chunk's segment and file name.
    pub(super) chunks: Vec<(i64, String)>,
}

impl Running {
    pub(super) fn progress(&self) -> Progress {
        let playlist = std::fs::read_to_string(self.dir.join("ffmpeg.m3u8")).unwrap_or_default();
        match &self.keyframes {
            None => Progress {
                finished: finished_segment(&playlist, self.extension),
                chunks: Vec::new(),
            },
            Some(k) => {
                let (chunks, finished) = chunk_segments(&playlist, &k.starts, self.start);
                Progress { finished, chunks }
            }
        }
    }

    /// The end of ffmpeg's log if it failed; `Some(Ok)` if it finished.
    pub(super) fn exited(&mut self) -> Option<Result<(), String>> {
        match self.child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                let log = std::fs::read_to_string(self.dir.join("ffmpeg.log")).unwrap_or_default();
                let tail: Vec<&str> = log.lines().rev().take(5).collect();
                Some(Err(tail.join(" | ")))
            }
            Ok(Some(_)) => Some(Ok(())),
            _ => None,
        }
    }

    pub(super) fn signal(&self, signal: libc::c_int) {
        if let Some(pid) = self.child.id() {
            // SAFETY: kill(2) only sends a signal to our own child process.
            unsafe { libc::kill(pid as libc::pid_t, signal) };
        }
    }

    /// Stops ffmpeg and, once it has exited, deletes its segments.
    pub(super) fn stop(mut self) {
        tokio::spawn(async move {
            let _ = self.child.kill().await;
            let _ = tokio::fs::remove_dir_all(self.dir).await;
        });
    }
}

/// The highest segment ffmpeg has finished: it lists a segment in its own playlist once the
/// segment is complete.
pub(super) fn finished_segment(playlist: &str, extension: &str) -> Option<i64> {
    playlist
        .lines()
        .filter_map(|line| {
            let name = line.strip_prefix("seg")?.strip_suffix(extension)?;
            name.strip_suffix('.')?.parse().ok()
        })
        .max()
}

/// Which segment each chunk of a copy run belongs to, and the highest segment finished.
/// ffmpeg lists each finished chunk with its length; the run's first chunk starts at segment
/// `start`'s keyframe, so the sums give each chunk's start, and a chunk belongs to the last
/// segment starting at or before it. A segment is finished once a chunk past it is.
pub(super) fn chunk_segments(
    playlist: &str,
    starts: &[f64],
    start: i64,
) -> (Vec<(i64, String)>, Option<i64>) {
    let Some(&first) = usize::try_from(start).ok().and_then(|s| starts.get(s)) else {
        return (Vec::new(), None);
    };
    // Lengths are printed to the microsecond; the margin covers their sum's rounding.
    let segment_at = |t: f64| starts.partition_point(|s| *s <= t + 0.01) as i64 - 1;
    let (mut at, mut length, mut ended) = (first, 0.0, false);
    let mut chunks = Vec::new();
    for line in playlist.lines() {
        if let Some(extinf) = line.strip_prefix("#EXTINF:") {
            length = extinf
                .split(',')
                .next()
                .and_then(|l| l.parse().ok())
                .unwrap_or(0.0);
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if !line.is_empty() && !line.starts_with('#') {
            // Chunks are deleted by these names: only ever c{i}.{extension} in the run's folder.
            let ours = line
                .strip_prefix('c')
                .and_then(|l| l.split_once('.'))
                .is_some_and(|(i, extension)| {
                    !i.is_empty()
                        && i.bytes().all(|b| b.is_ascii_digit())
                        && !extension.contains('/')
                });
            if ours {
                chunks.push((segment_at(at), line.to_owned()));
            }
            at += length;
        }
    }
    let finished = if ended {
        starts.len() as i64 - 1
    } else {
        segment_at(at) - 1
    };
    (chunks, Some(finished).filter(|f| *f >= start))
}

/// Puts a copy run's segment together from its chunks, through a temporary file.
pub(super) async fn assemble(chunks: &[PathBuf], path: &Path) -> std::io::Result<()> {
    static PARTS: AtomicU64 = AtomicU64::new(0);
    let part = path.with_extension(format!("{}.part", PARTS.fetch_add(1, Ordering::Relaxed)));
    let mut out = tokio::fs::File::create(&part).await?;
    for chunk in chunks {
        tokio::io::copy(&mut tokio::fs::File::open(chunk).await?, &mut out).await?;
    }
    out.flush().await?;
    tokio::fs::rename(&part, path).await
}

/// What to do with a running ffmpeg, given how far ahead of the viewer it is.
pub(super) fn throttle(ahead: i64, paused: bool) -> Option<libc::c_int> {
    if !paused && ahead > AHEAD_PAUSE {
        Some(libc::SIGSTOP)
    } else if paused && ahead < AHEAD_RESUME {
        Some(libc::SIGCONT)
    } else {
        None
    }
}

/// Stops the run asked for least recently, freeing its slot, if nobody has asked it for
/// anything in [`TAKE_OVER_AFTER`]; [`TranscodeError::Busy`] when every run is in use.
pub(super) fn take_quietest_slot(
    running: &mut HashMap<Vec<u8>, Running>,
) -> Result<(), TranscodeError> {
    let quietest = running
        .iter()
        .min_by_key(|(_, r)| r.last_request)
        .filter(|(_, r)| r.last_request.elapsed() >= TAKE_OVER_AFTER)
        .map(|(id, _)| id.clone())
        .ok_or(TranscodeError::Busy)?;
    if let Some(r) = running.remove(&quietest) {
        r.stop();
    }
    Ok(())
}
