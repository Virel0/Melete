// Keeping a watch party in step, for the household's player and guests' alike. The rules live
// here, and only here.
//
// - partyClock(): the server's clock as this browser sees it, NTP-style. Each answer timed from
//   ask to arrival gives a sample, offset = server time − the request's midpoint; the sample
//   with the shortest round trip among the latest few wins, so a slow answer can't skew it.
// - expected(p, clock): where the party's file is now, in seconds; held at its start until a
//   start set a moment ahead (so everyone starts together) comes. This is the reference: the
//   party's own clock, never the slowest player's place.
// - correct(video, p, clock, note): the player brought to the party by speed, rarely by a seek.
//   A seek empties a conversion's buffer, and on a stream barely faster than real time each one
//   puts the player further behind, so while the party plays:
//   - drift under 0.1 s is left (a speed change can't be heard, pitch kept, so it starts soon);
//   - from 0.1 s to 4 s the player plays at 1.05× or 0.95× (1.1× or 0.9× past 1 s) until it's
//     within 0.04 s, then at 1×; it plays faster only with 2 s or more buffered ahead, so
//     catching up doesn't make it stall;
//   - past 4 s it seeks, at most once in 10 s: forward, a little past the party by what its
//     last catch-up seek took to play again, so it lands in step; backward only to a place it
//     still holds, or when it's over 15 s ahead; otherwise it plays faster or slower meanwhile;
//   - while it's buffering (from `waiting` or `seeking` until it can play again, or without
//     enough data to play on), nothing is done: drift then only says how slow the stream is;
//   - a move someone made (their seek: a new version with the party elsewhere) is followed at
//     once, with a seek past 0.1 s: it's not drift, and no limit applies. A move is a version
//     more than 0.3 s from where the one before had the party, past a pause's or a play's
//     rounding, so pressing play isn't taken for one.
//   A paused party has every player where it paused, except that while it's held for someone,
//   and as it plays on after, a player up to 4 s behind (the one it waited for) stays and
//   catches up by speed once it plays, rather than loading afresh. A player not going yet (the
//   party's play reached it after the start, or it's still loading) is put where the party is:
//   a jump nobody sees, where a nudge would leave it behind for half a minute.
//   `note(event, detail)` (the playback report's, when there is one) is told each change in what
//   it does: drift, the action (none, rate or seek), the speed, the reference and whether the
//   player was buffering; every seek is told.
// - placeToSend(video, p, clock, sought): the place sent with this player's play or pause: the
//   party's own while it plays (the player's, and the drift it last had past 0.1 s), so one
//   player behind pausing doesn't pull everyone back; the player's when the viewer sought.
// - partySeek(video): whether the seek that just ended was one of these, or the player's own
//   step of under a second over a gap, rather than the viewer's: neither is sent to the party.
// - catchingUp(video): from a catch-up seek made while the party plays until the player can play
//   again; a stall then is this player's alone, and the party isn't held for it.

// Samples kept, and how long each counts.
const SAMPLES = 8;
const SAMPLE_MS = 5 * 60 * 1000;
// Drift (seconds) left alone; where a speed change ends; past which the speed is firmer.
const LEFT = 0.1;
const SETTLED = 0.04;
const FIRM_PAST = 1;
const GENTLE = 0.05;
const FIRM = 0.1;
// How far (seconds) a new version must put the party from where the one before had it to be
// someone's move.
const MOVED = 0.3;
// Past this a player seeks, at most once in SEEK_EVERY_MS; when it's ahead by more than
// FAR_AHEAD, even to a place it no longer holds.
const SEEK_PAST = 4;
const SEEK_EVERY_MS = 10_000;
const FAR_AHEAD = 15;
// Buffered ahead (seconds) needed to play faster; the most a forward seek leads by.
const AHEAD_NEEDED = 2;
const LEAD_MOST = 8;
// How long a player's last drift counts for the place its pause is sent at.
const LAG_MS = 5000;
// How long after a catch-up seek a stall is still its own.
const CATCH_UP_MOST_MS = 30_000;
// A paused party, or a player not going yet: put where the party is past this.
const STILL_CLOSE = 0.08;
// A seek this short is the player stepping over a gap, not the viewer's.
const STEP = 1;
// HAVE_FUTURE_DATA: enough to play on.
const CAN_PLAY = 3;

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

// What's known of each player: whether it's stalled, where it was before a seek, the seek of
// ours under way, its last catch-up seek and what that took, and what was told last. Listened
// for from the first look at it.
const players = new WeakMap();

function known(video) {
  let s = players.get(video);
  if (s) return s;
  s = { stalled: false, at: null, from: null, target: null, seekAt: -Infinity, catchUp: null, lead: 0, said: null, seen: null, kept: false, lag: null };
  players.set(video, s);
  const on = (type, fn) => video.addEventListener?.(type, fn);
  const going = () => {
    s.stalled = false;
    // What the catch-up seek cost, until it could play again: the next one's lead.
    if (s.catchUp !== null) s.lead = Math.min(LEAD_MOST, (Date.now() - s.catchUp) / 1000);
    s.catchUp = null;
  };
  on("waiting", () => (s.stalled = true));
  on("seeking", () => {
    s.stalled = true;
    s.from = s.at;
  });
  on("playing", going);
  on("canplay", going);
  on("timeupdate", () => {
    if (!video.seeking) s.at = video.currentTime;
  });
  return s;
}

export const buffering = (video) => known(video).stalled || video.seeking || video.readyState < CAN_PLAY;

// Seconds buffered from the player's place on, and whether `at` is buffered.
function held(video, at) {
  const ranges = video.buffered;
  let ahead = 0;
  let has = false;
  for (let i = 0; i < (ranges?.length ?? 0); i++) {
    const start = ranges.start(i);
    const end = ranges.end(i);
    if (start <= video.currentTime + 0.1 && end >= video.currentTime) ahead = end - video.currentTime;
    if (start <= at && end >= at + 0.5) has = true;
  }
  return { ahead, has };
}

function seekTo(video, s, at) {
  s.target = at;
  video.currentTime = at;
}

// The rule itself, from what's measured: `drift` (party − player, seconds), whether the player
// is buffering, its speed now, ms since its last catch-up seek, seconds buffered ahead, whether
// the party's place is buffered. Answers { action: "none" | "rate" | "seek", rate }.
export function decide({ drift, buffering, rate, sinceSeekMs, ahead, targetHeld }) {
  if (buffering) return { action: "none", rate: 1 };
  const size = Math.abs(drift);
  const behind = drift > 0;
  // Played faster only with room in the buffer; slower is always safe.
  const speed = (step) => (behind && ahead < AHEAD_NEEDED ? { action: "none", rate: 1 } : { action: "rate", rate: behind ? 1 + step : 1 - step });
  if (size > SEEK_PAST) {
    const jump = sinceSeekMs >= SEEK_EVERY_MS && (behind || targetHeld || size > FAR_AHEAD);
    return jump ? { action: "seek", rate: 1 } : speed(FIRM);
  }
  const correcting = rate !== 1 && rate > 1 === behind && size > SETTLED;
  if (size > LEFT || correcting) return speed(size > FIRM_PAST ? FIRM : GENTLE);
  return { action: "none", rate: 1 };
}

// Tells `note` of a change in what's done; every seek is told.
function tell(note, s, what) {
  if (!note) return;
  const key = `${what.action} ${what.rate} ${what.buffering}`;
  if (key === s.said && what.action !== "seek") return;
  s.said = key;
  note("sync", what);
}

// Brings `video` to the party. Answers what it did: "seek", "rate", "wait" (a start ahead,
// played when it comes) or "none". A paused party has everyone where it paused, so it plays on
// together; the caller pauses the player.
export function correct(video, p, clock, note) {
  const s = known(video);
  const moved = movedBy(s, p);
  const told = (action, reference, more = {}) => {
    const drift = reference - video.currentTime;
    tell(note, s, { drift_ms: Math.round(drift * 1000), action, rate: video.playbackRate, reference: Math.round(reference * 10) / 10, buffering: buffering(video), ...more });
  };
  // Put where the party is, when it's still or not going here yet; while a party's held, and
  // as it plays on after, a player up to 4 s behind it (the one it waited for) stays, and
  // catches up by speed once it plays, rather than loading afresh.
  const put = (at, close) => {
    video.playbackRate = 1;
    const behind = at - video.currentTime;
    if (Math.abs(behind) <= close) return "none";
    if (s.kept && behind > 0 && behind <= SEEK_PAST) return "none";
    told("seek", at);
    seekTo(video, s, at);
    return "seek";
  };
  if (!p.playing) {
    s.kept = p.held;
    return put(p.position_ms / 1000, STILL_CLOSE);
  }
  if (startsIn(p, clock) > 0) {
    if (!video.paused) video.pause();
    put(p.position_ms / 1000, STILL_CLOSE);
    return "wait";
  }
  const now = clock.now();
  const reference = expected(p, clock);
  const drift = reference - video.currentTime;
  const stuck = buffering(video);
  // Not going yet: looser while it's still loading there.
  if (video.paused) return put(reference, stuck ? SEEK_PAST : STILL_CLOSE);
  s.kept = false;
  if (moved && Math.abs(drift) > LEFT) {
    // Someone moved the party: followed at once, whatever the limits.
    video.playbackRate = 1;
    told("seek", reference, { moved: true });
    seekTo(video, s, reference);
    return "seek";
  }
  s.lag = { drift, at: now };
  const { ahead, has } = held(video, reference);
  const what = decide({ drift, buffering: stuck, rate: video.playbackRate, sinceSeekMs: now - s.seekAt, ahead, targetHeld: has });
  if (video.playbackRate !== what.rate) video.playbackRate = what.rate;
  told(what.action, reference);
  if (what.action === "seek") {
    s.seekAt = now;
    s.catchUp = Date.now();
    seekTo(video, s, reference + (drift > 0 ? s.lead : 0));
  }
  return what.action;
}

// Whether a newer version put the party somewhere the one seen before wouldn't have been when
// it took effect: someone's seek, not a pause, a wait or playing on.
function movedBy(s, p) {
  const before = s.seen;
  s.seen = p;
  if (!before || before.version === p.version) return false;
  const then = before.position_ms + (before.playing ? Math.max(0, p.position_at - before.position_at) : 0);
  return Math.abs(p.position_ms - then) / 1000 > MOVED;
}

export function placeToSend(video, p, clock, sought) {
  const own = Number.isFinite(video.currentTime) ? video.currentTime : 0;
  const { lag } = known(video);
  // The party's place when this player stopped: its own, and the drift it last had, when that
  // was more than is left alone and recent.
  const behind = !sought && p?.playing && lag && clock.now() - lag.at < LAG_MS && Math.abs(lag.drift) > LEFT;
  const at = behind ? Math.max(0, Math.min(own + lag.drift, Number.isFinite(video.duration) ? video.duration : Infinity)) : own;
  return Math.round(at * 1000);
}

// Whether the seek that just ended was one of ours, or the player's own step under a second
// over a gap; called from `seeked`, it answers once for each.
export function partySeek(video) {
  const s = known(video);
  const target = s.target;
  s.target = null;
  if (target !== null && Math.abs(video.currentTime - target) < STEP) return true;
  return s.from !== null && Math.abs(video.currentTime - s.from) < STEP;
}

export function catchingUp(video) {
  const { catchUp } = known(video);
  return catchUp !== null && Date.now() - catchUp < CATCH_UP_MOST_MS;
}

// Starts watching `video` (its stalls, its place before each seek) before any correction.
export const watchPlayer = (video) => void known(video);
