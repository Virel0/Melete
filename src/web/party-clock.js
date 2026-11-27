// Keeping a watch party in step, for the household's player and guests' alike.
//
// - partyClock(): the server's clock as this browser sees it, NTP-style. Each answer timed from
//   ask to arrival gives a sample, offset = server time − the request's midpoint; the sample
//   with the shortest round trip among the latest few wins, so a slow answer can't skew it.
// - expected(p, clock): where the party's file is now, in seconds; held at its start until a
//   start set a moment ahead (so everyone starts together) comes.
// - correct(video, p, clock, coarse): the player brought to the party gently: drift under 80 ms
//   is left, up to the seek limit it's played a little faster or slower until it's gone, and
//   past it the player seeks. Conversions (HLS) seek coarser, so their limit is looser.

// Samples kept, and how long each counts.
const SAMPLES = 8;
const SAMPLE_MS = 5 * 60 * 1000;
// Drift left alone, nudged, and sought past, in seconds.
const CLOSE = 0.08;
const SEEK = 1.5;
const SEEK_COARSE = 2.5;
const NUDGE = 0.05;

export function partyClock() {
  let samples = [];
  let offset = 0;
  return {
    now: () => Date.now() + offset,
    // An answer's server_time, asked at `sent` and arrived at `arrived` (this browser's ms).
    take(serverTime, sent, arrived) {
      if (!Number.isFinite(serverTime)) return;
      const at = Date.now();
      samples = samples.filter((s) => at - s.at < SAMPLE_MS).slice(-(SAMPLES - 1));
      samples.push({ rtt: arrived - sent, offset: serverTime - (sent + arrived) / 2, at });
      offset = samples.reduce((best, s) => (s.rtt < best.rtt ? s : best)).offset;
    },
  };
}

// Asks with `ask()` (an api call answering `{server_time, …}`), timing it for the clock.
export async function timed(clock, ask) {
  const sent = Date.now();
  const answer = await ask();
  clock.take(answer?.server_time, sent, Date.now());
  return answer;
}

// How long until a party set playing starts, in ms; 0 once it has.
export const startsIn = (p, clock) => (p.playing ? Math.max(0, p.position_at - clock.now()) : 0);

export function expected(p, clock) {
  const played = p.playing ? Math.max(0, clock.now() - p.position_at) : 0;
  return (p.position_ms + played) / 1000;
}

// Brings `video` to the party. Answers what it did: "seek", "nudge", "wait" (a start ahead,
// played when it comes) or "close".
export function correct(video, p, clock, coarse = false) {
  if (!p.playing) {
    video.playbackRate = 1;
    return "close";
  }
  const ahead = startsIn(p, clock);
  if (ahead > 0) {
    if (!video.paused) video.pause();
    if (Math.abs(video.currentTime - p.position_ms / 1000) > CLOSE) video.currentTime = p.position_ms / 1000;
    return "wait";
  }
  const drift = expected(p, clock) - video.currentTime;
  if (Math.abs(drift) > (coarse ? SEEK_COARSE : SEEK)) {
    video.currentTime = expected(p, clock);
    video.playbackRate = 1;
    return "seek";
  }
  if (Math.abs(drift) > CLOSE) {
    video.playbackRate = drift > 0 ? 1 + NUDGE : 1 - NUDGE;
    return "nudge";
  }
  video.playbackRate = 1;
  return "close";
}
