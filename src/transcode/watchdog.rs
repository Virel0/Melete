//! The speed watchdog: a conversion that can't keep well ahead of real time stalls its viewer
//! sooner or later, so one going slower than [`MIN_SPEED`] once it has worked for
//! [`WATCH_AFTER`] steps down a rung, to a smaller picture at a lower bit rate, from the next
//! segment it hasn't made, and says so in the log. The step stays with the session, through
//! seeks; another slow spell steps down again, to the smallest rung. Only MPEG-TS runs step
//! down: a run in fragmented MP4 shares one init section across the session, which a new
//! picture size would change, so a slow one is only logged.

use super::*;

/// How long a run works before its speed is judged.
pub const WATCH_AFTER: Duration = Duration::from_secs(30);
/// The slowest a run may convert, in seconds of video a second.
pub const MIN_SPEED: f64 = 1.3;
/// The picture heights a slow conversion steps down through.
pub const HEIGHTS: [i64; 5] = [2160, 1440, 1080, 720, 480];

/// The run's speed when it's too slow: it has worked [`WATCH_AFTER`] (paused time aside) and
/// converted less than [`MIN_SPEED`] times that.
pub fn too_slow(converted_seconds: f64, working: Duration) -> Option<f64> {
    if working < WATCH_AFTER {
        return None;
    }
    let speed = converted_seconds / working.as_secs_f64();
    (speed < MIN_SPEED).then_some(speed)
}

/// The rung below a picture `height` tall: the tallest of [`HEIGHTS`] shorter than it.
pub fn rung_below(height: i64) -> Option<i64> {
    HEIGHTS.into_iter().find(|h| *h < height)
}

impl Plan {
    /// The plan `steps` rungs down from its picture: `steps` times the next of [`HEIGHTS`]
    /// below, the bit rate no more than that height's default. Itself for no steps; None when
    /// there's no rung that low, or the video is copied.
    pub fn stepped_down(&self, steps: u8) -> Option<Plan> {
        if steps == 0 {
            return Some(self.clone());
        }
        // A picture burned in was sized for the plan's picture.
        if self.codec == VideoCodec::Copy || self.burn.is_some() {
            return None;
        }
        let mut height = self.height.unwrap_or(self.about.source_size.1);
        for _ in 0..steps {
            height = rung_below(height)?;
        }
        let mut plan = self.clone();
        plan.height = Some(height);
        plan.bitrate = plan.bitrate.min(default_bitrate(height));
        Some(plan)
    }
}

impl Transcoder {
    /// Steps each run that's too slow down a rung ([`too_slow`]), from the next segment it
    /// hasn't made; one in fragmented MP4, or already at the smallest rung, is only logged,
    /// once.
    pub(super) fn watch_speed(&self, running: &mut HashMap<Vec<u8>, Running>) {
        let mut slow = Vec::new();
        for (session, r) in running.iter_mut() {
            if r.keyframes.is_some() || r.speed_told || r.exited().is_some() {
                continue;
            }
            let Some(speed) = too_slow(r.converted_seconds(), r.working()) else {
                continue;
            };
            let lower = r.asked.stepped_down(r.steps + 1).filter(|_| !r.asked.fmp4);
            match lower {
                Some(lower) => slow.push((session.clone(), speed, lower.height)),
                None => {
                    r.speed_told = true;
                    tracing::warn!(
                        run = r.run,
                        speed = format!("{speed:.2}x real time"),
                        "a video conversion is too slow to keep ahead, and has no smaller picture to step down to here"
                    );
                }
            }
        }
        for (session, speed, height) in slow {
            let Some(r) = running.get_mut(&session) else {
                continue;
            };
            r.steps += 1;
            let from = r.progress().finished.map_or(r.start, |f| f + 1);
            let asked = r.asked.clone();
            tracing::warn!(
                run = r.run,
                speed = format!("{speed:.2}x real time"),
                height,
                from_segment = from,
                "a video conversion was too slow to keep ahead; stepping down a rung"
            );
            if let Err(err) = self.start(running, &session, &asked, from, false) {
                tracing::warn!(error = %err, "couldn't start the smaller conversion");
            }
        }
    }
}

#[cfg(test)]
mod tests;
