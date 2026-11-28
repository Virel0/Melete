//! What the log says about each run of ffmpeg for a viewer: a line when it starts (why it
//! converts, from what to what, where) and one when it stops (how quickly it went, and why it
//! stopped). Never a stream token, a file's path or who is watching.

use super::*;

/// What a conversion makes from what, and why, for the log.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct About {
    /// Why the file is converted rather than played as it is, in words.
    pub why: String,
    /// The source's video codec, as ffprobe names it.
    pub source_codec: String,
    /// The source's picture, width by height.
    pub source_size: (i64, i64),
    /// The source's HDR, or SDR.
    pub source_picture: &'static str,
    /// The file's overall bits a second, when known.
    pub source_bitrate: Option<i64>,
    /// The output's picture, width by height.
    pub out_size: (i64, i64),
}

impl About {
    /// What a plan of `video` with `settings` makes, at `out_size`.
    pub(super) fn of(
        video: &crate::probe::VideoStream,
        info: &MediaInfo,
        settings: &Settings,
        out_size: (i64, i64),
        tonemap: bool,
    ) -> Self {
        let codec = settings.video_codec.unwrap_or_default();
        let mut why: Vec<&str> = Vec::new();
        if codec == VideoCodec::Copy {
            why.push("only the container and audio converted for the player");
        }
        if settings.burn_subtitle.is_some() {
            why.push("subtitles drawn into the picture");
        }
        if settings.max_bitrate.is_some() {
            why.push("held to a bitrate");
        }
        if settings.max_height.is_some() {
            why.push("a smaller picture asked for");
        }
        if tonemap {
            why.push("HDR tone mapped to SDR");
        }
        if why.is_empty() {
            why.push(match codec {
                VideoCodec::Hevc => "the player asked for HEVC",
                _ => "the player asked for H.264",
            });
        }
        let source_size = (
            video.width.unwrap_or_default(),
            video.height.unwrap_or_default(),
        );
        Self {
            why: why.join("; "),
            source_codec: video.codec.clone(),
            source_size,
            source_picture: match video.picture() {
                Picture::Sdr => "SDR",
                Picture::Hdr10 => "HDR10",
                Picture::Hlg => "HLG",
                Picture::DolbyVisionOnly => "Dolby Vision",
            },
            source_bitrate: info.bit_rate.filter(|b| *b > 0),
            out_size: if codec == VideoCodec::Copy {
                source_size
            } else {
                out_size
            },
        }
    }
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stopped {
    /// Nobody asked it for anything in a while: the viewer left or paused.
    Idle,
    /// Started again somewhere else in the file: the viewer jumped, or it had finished.
    Restarted,
    /// It failed on the GPU, and starts again on the CPU.
    OnCpu,
    /// ffmpeg failed.
    Failed,
    /// Its slot went to another viewer's conversion.
    TakenOver,
    /// Its viewer, behind a watch party's shared run for a moment, caught up with it.
    CaughtUp,
}

impl Stopped {
    fn words(self) -> &'static str {
        match self {
            Self::Idle => "nobody asked for it in a while",
            Self::Restarted => "started again elsewhere in the file",
            Self::OnCpu => "it failed on the GPU; going on on the CPU",
            Self::Failed => "ffmpeg failed",
            Self::TakenOver => "its slot went to another viewer",
            Self::CaughtUp => "its viewer caught up with the party's shared run",
        }
    }
}

/// Bits a second in words, as "7.6 Mbit/s".
fn rate(bits: i64) -> String {
    format!("{:.1} Mbit/s", bits as f64 / 1e6)
}

fn size((width, height): (i64, i64)) -> String {
    format!("{width}x{height}")
}

/// Says a run started: `run` numbers it, so its stop line can be found.
pub(super) fn started(plan: &Plan, hw: Hardware, run: u64, at_seconds: f64) {
    let a = &plan.about;
    let output_codec = match plan.codec {
        VideoCodec::Copy => "copied",
        VideoCodec::Hevc => "hevc",
        VideoCodec::H264 => "h264",
    };
    let output_bitrate = match plan.codec {
        VideoCodec::Copy => "as the source".to_owned(),
        _ => rate(plan.bitrate),
    };
    let hardware = match (plan.codec, hw) {
        (VideoCodec::Copy, _) => "CPU (no encoding)",
        (_, Hardware::Nvenc) if plan.gpu_decode => "GPU (decoding too)",
        (_, Hardware::Nvenc) => "GPU",
        (_, Hardware::Software) => "CPU",
    };
    tracing::info!(
        run,
        why = a.why,
        source = format!(
            "{} {} {} {}",
            a.source_codec,
            size(a.source_size),
            a.source_picture,
            a.source_bitrate
                .map_or_else(|| "bitrate unknown".into(), rate)
        ),
        output = format!("{output_codec} {} {output_bitrate}", size(a.out_size)),
        hardware,
        from_seconds = at_seconds.round() as i64,
        "a video conversion started"
    );
}

impl Running {
    /// How many seconds of the file this run has made: up to the end of the last segment it
    /// finished, or to the end of the file once it finished cleanly.
    pub(super) fn converted_seconds(&mut self) -> f64 {
        let first = self.seconds_at(self.start);
        let finished = match self.exited() {
            Some(Ok(())) => return (self.duration - first).max(0.0),
            _ => self.progress().finished,
        };
        finished.map_or(0.0, |f| (self.seconds_at(f + 1) - first).max(0.0))
    }

    /// Where segment `n` starts, in seconds; the file's end past its last.
    fn seconds_at(&self, n: i64) -> f64 {
        let at = match &self.keyframes {
            Some(k) => usize::try_from(n)
                .ok()
                .and_then(|n| k.starts.get(n).copied()),
            None => Some(n as f64 * SEGMENT_SECONDS),
        };
        at.unwrap_or(self.duration).min(self.duration)
    }

    /// How long it has been working: since it started until it ended (or now), less the time
    /// it was paused.
    fn working(&self) -> Duration {
        let paused = self.paused_for + self.paused_at.map_or(Duration::ZERO, |at| at.elapsed());
        let until = self.ended_at.unwrap_or_else(Instant::now);
        until.duration_since(self.started).saturating_sub(paused)
    }

    /// Notes when ffmpeg ended, the first time it's seen to have: the supervisor looks every
    /// couple of seconds.
    pub(super) fn note_end(&mut self) {
        if self.ended_at.is_none() && self.exited().is_some() {
            self.ended_at = Some(Instant::now());
        }
    }

    /// Pauses or resumes ffmpeg, keeping count of the time it spent paused.
    pub(super) fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.signal(if paused { libc::SIGSTOP } else { libc::SIGCONT });
        self.paused = paused;
        match (paused, self.paused_at.take()) {
            (true, _) => self.paused_at = Some(Instant::now()),
            (false, Some(at)) => self.paused_for += at.elapsed(),
            (false, None) => {}
        }
    }

    /// Says the run stopped, how quickly it went and why, then stops ffmpeg and deletes its
    /// segments.
    pub(super) fn stop(mut self, why: Stopped) {
        let converted = self.converted_seconds();
        let took = self.working().as_secs_f64();
        let speed = if took > 0.0 { converted / took } else { 0.0 };
        tracing::info!(
            run = self.run,
            converted_seconds = converted.round() as i64,
            took_seconds = took.round() as i64,
            viewers = self.most_viewers.max(1),
            speed = format!("{speed:.2}x real time"),
            why = why.words(),
            "a video conversion stopped"
        );
        self.end();
    }
}
