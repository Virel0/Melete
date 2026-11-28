//! Starting ffmpeg for a session: on the GPU or the CPU, in a free slot or one taken over, and
//! again on the CPU when the GPU fails.

use super::*;

impl Transcoder {
    /// The session a viewer's ask for segment `n` is served from: the shared one, unless the
    /// viewer hasn't caught up with where that run starts and others are watching it, as when
    /// a viewer's player still loads what came before everyone jumped ahead. Starting the
    /// shared run again there would take it away from the others, so that viewer gets a run of
    /// their own until they catch up, and it stops as they do. A
    /// viewer who was on the shared run and goes back moves it, as a party jumping back does.
    pub(super) fn behind_or_shared<'a>(
        &self,
        session: &'a [u8],
        viewer: &'a [u8],
        n: i64,
    ) -> &'a [u8] {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        let behind = running
            .get(session)
            .is_some_and(|r| n < r.start && !r.caught_up(viewer) && r.others_asking(viewer));
        if behind {
            return viewer;
        }
        // Caught up: the run of their own isn't needed any more.
        if session != viewer
            && let Some(own) = running.remove(viewer)
        {
            own.stop(Stopped::CaughtUp);
        }
        session
    }

    /// Starts a session's run again on the CPU after it failed on the GPU before finishing a
    /// segment, and has the GPU checked. A session that fell back stays on the CPU.
    pub(super) fn retry_on_cpu(
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

    /// Starts ffmpeg for a session at segment `n`, in place of any run it has (keeping its
    /// viewers), on the CPU when
    /// `on_cpu`, the session fell back before, or the GPU isn't working. When every slot is
    /// taken, the run asked for least recently gives up its slot if it has been quiet for
    /// [`TAKE_OVER_AFTER`]; otherwise it's [`TranscodeError::Busy`].
    pub(super) fn start(
        &self,
        running: &mut HashMap<Vec<u8>, Running>,
        session: &[u8],
        plan: &Plan,
        n: i64,
        on_cpu: bool,
    ) -> Result<(), TranscodeError> {
        let mut on_cpu = on_cpu;
        // Viewers sharing it stay with it when it starts again elsewhere, and so does any step
        // down for being too slow.
        let mut viewers = HashMap::new();
        let mut steps = 0;
        if let Some(mut old) = running.remove(session) {
            on_cpu |= old.on_cpu;
            steps = old.steps;
            viewers = std::mem::take(&mut old.viewers);
            old.stop(if on_cpu {
                Stopped::OnCpu
            } else {
                Stopped::Restarted
            });
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
        let asked = plan.clone();
        let mut plan = plan.stepped_down(steps).unwrap_or_else(|| plan.clone());
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
        let at = plan
            .segment_starts()
            .get(n as usize)
            .copied()
            .unwrap_or(0.0);
        started(&plan, hw, run, at);
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
                run,
                started: Instant::now(),
                paused_at: None,
                paused_for: Duration::ZERO,
                ended_at: None,
                duration: plan.duration,
                most_viewers: viewers.len(),
                viewers,
                asked,
                steps,
                speed_told: false,
            },
        );
        Ok(())
    }
}
