// Listen Together's engine: this device in a session (`/music/sessions*`) played in step with
// the others in it. The server keeps the session (the queue by track id, which track, where and
// whether it plays, at a version); each change here is sent against the version last seen, and
// every device hears of each change from the event stream's `listening` event and follows it.
// A change refused because another device got in first is dropped, and the newer state
// followed instead.
//
// What's sent: a new queue, a skip, a seek, or play and pause, made on this device while not
// following someone else's change. Pressing play while the session plays catches up with it
// rather than sending this device's place. A track ending moves every device on by itself.
//
// - startTogether(user): once signed in; picks up a session this device is still in, and plays
//   along with it.
// - together(): { me, session, note } as it stands; `melete:together` is sent on each change.
// - startSession(), joinSession(code), leaveSession(), endSession(): what the tab does; each
//   throws the server's refusal for the tab to say.

import { SignedOut, announce, api } from "/web/lib.js";
import { audio, jumpTo, pause, playQueue, seekTo } from "/web/music-player.js";
import * as queue from "/web/music-queue.js";
import { looseQueue } from "/web/music-play.js";
import { seesAllMusic } from "/web/music-access.js";

// A queue holds at most this many tracks (the server's limit).
const MOST = 1000;
// Off by more than this, in seconds, a device seeks to where the session is.
const DRIFT = 1.5;
// Changes close together go as one.
const SEND_AFTER_MS = 300;
// How long a followed change's own play, pause and seek events are left unsent.
const SETTLE_MS = 800;
const RETRY_MS = 15_000;

let me = null;
let session = null;
let note = "";
let source = null;
let retry = null;
// The server's clock less this one's, from the last answer, for where a playing session is now.
let skew = 0;
let following = 0;
let sendTimer = null;

export const together = () => ({ me, session, note });

function changed() {
  document.dispatchEvent(new CustomEvent("melete:together"));
}

// An answer with a session and the server's clock.
function took(answer) {
  skew = answer.server_time - Date.now();
  session = answer.session;
  note = "";
  listen();
  changed();
}

function ended(words) {
  session = null;
  note = words;
  clearTimeout(retry);
  clearTimeout(sendTimer);
  source?.close();
  source = null;
  announce(words);
  changed();
}

// Where the session's track is now, in seconds.
function placeOf(s) {
  const ms = s.position_ms + (s.playing ? Math.max(0, Date.now() + skew - s.position_at) : 0);
  return ms / 1000;
}

const ids = () => queue.all().map((e) => e.track.id);

// This device's state, as the server takes it.
function here() {
  let tracks = ids();
  let index = Math.max(queue.position(), 0);
  if (tracks.length > MOST) {
    tracks = tracks.slice(index, index + MOST);
    index = 0;
  }
  return {
    queue: tracks,
    index: tracks.length ? index : 0,
    position_ms: Math.round((Number.isFinite(audio.currentTime) ? audio.currentTime : 0) * 1000),
    playing: !audio.paused,
  };
}

// The session's tracks as the player's queue entries: those queued here already as they are,
// the rest asked for, a hundred at a time.
async function tracksFor(wanted) {
  const known = new Map(queue.all().map((e) => [e.track.id, e.track]));
  const missing = [...new Set(wanted.filter((id) => !known.has(id)))];
  for (let i = 0; i < missing.length; i += 100) {
    const page = await api(`/music/tracks?id=${missing.slice(i, i + 100).join(",")}&limit=100`);
    for (const track of looseQueue(page.items)) known.set(track.id, track);
  }
  return wanted.map((id) => known.get(id)).filter(Boolean);
}

// Resolves once the new track has started playing (a seek before that is lost as it starts), or
// after a few seconds.
function loaded() {
  return new Promise((done) => {
    const finish = () => {
      clearTimeout(timer);
      audio.removeEventListener("playing", finish);
      done();
    };
    const timer = setTimeout(finish, 5000);
    audio.addEventListener("playing", finish);
  });
}

// Whether a state being followed has been overtaken: the session left, or changed since. (The
// same version heard again, say from the event stream, is the same state.)
const stale = (s) => !session || session.id !== s.id || session.version !== s.version;

// Plays what the session plays, where it plays it.
async function follow(s) {
  following++;
  try {
    const same = ids().length === s.queue.length && ids().every((id, i) => id === s.queue[i]);
    if (!s.queue.length) {
      if (!audio.paused) pause();
      return;
    }
    if (!same) {
      // The session's order is everyone's: shuffle here would mix it.
      if (queue.shuffled()) queue.toggleShuffle();
      const tracks = await tracksFor(s.queue);
      if (stale(s) || tracks.length !== s.queue.length) return;
      const ready = loaded();
      playQueue(tracks, s.index);
      await ready;
    } else if (queue.position() !== s.index) {
      const ready = loaded();
      jumpTo(s.index);
      await ready;
    }
    if (stale(s)) return;
    const at = placeOf(s);
    if (Math.abs(audio.currentTime - at) > DRIFT) seekTo(at);
    if (s.playing && audio.paused) await audio.play().catch(() => {});
    if (!s.playing && !audio.paused) pause();
  } catch (err) {
    if (!(err instanceof SignedOut)) note = `Couldn't follow the session: ${err.message}`;
    changed();
  } finally {
    setTimeout(() => following--, SETTLE_MS);
  }
}

// Takes a newer state of the session from the server and follows it.
async function refresh() {
  if (!session) return;
  try {
    took(await api(`/music/sessions/${session.id}`));
    follow(session);
  } catch (err) {
    if (err.status === 404) ended("The session has ended.");
  }
}

async function send() {
  if (!session) return;
  try {
    took(await api(`/music/sessions/${session.id}`, { method: "PUT", body: { version: session.version, ...here() } }));
  } catch (err) {
    if (err.status === 409) return refresh();
    if (err.status === 404) return ended("The session has ended.");
    if (err instanceof SignedOut) return;
    note = `Couldn't change the session: ${err.message}`;
    changed();
  }
}

// Whether this device plays the session's queue at the session's track.
function inStep() {
  const now = here();
  return now.index === session.index && now.queue.length === session.queue.length && now.queue.every((id, i) => id === session.queue[i]);
}

// Something changed here: sent unless it's this device following, or nothing new. Play while
// the session plays, on its track, catches up with it; on another track (this one moved on),
// the new track is what gets sent.
function local(kind) {
  if (!session || following) return;
  if (kind === "play" && session.playing) return inStep() ? follow(session) : undefined;
  if (kind === "pause" && (audio.ended || !session.playing)) return;
  if (kind === "queue" && inStep()) return;
  clearTimeout(sendTimer);
  sendTimer = setTimeout(send, SEND_AFTER_MS);
}

audio.addEventListener("play", () => local("play"));
audio.addEventListener("pause", () => local("pause"));
audio.addEventListener("seeked", () => local("seek"));
document.addEventListener("melete:queue", () => local("queue"));

// The event stream, while in a session: each `listening` event lists the sessions this device
// is in; a newer version of this one is followed, and its absence means it ended.
function listen() {
  if (source || !session) return;
  source = new EventSource("/api/v1/events");
  source.addEventListener("listening", (message) => {
    let sessions;
    try {
      sessions = JSON.parse(message.data);
    } catch {
      return;
    }
    if (!session) return;
    const now = sessions.find((s) => s.id === session.id);
    if (!now) return ended("The session has ended.");
    const newer = now.version > session.version;
    if (now.version >= session.version) session = now;
    changed();
    if (newer) follow(now);
  });
  // The browser reconnects a dropped stream by itself; one it gives up on is opened again a
  // little later, unless the sign-in has ended (which leaves the session too).
  source.addEventListener("error", async () => {
    if (source?.readyState !== EventSource.CLOSED) return;
    source = null;
    try {
      await api("/me");
    } catch (err) {
      if (err instanceof SignedOut) {
        session = null;
        return changed();
      }
    }
    retry = setTimeout(listen, RETRY_MS);
  });
}

export async function startTogether(user) {
  me = user;
  session = null;
  note = "";
  clearTimeout(retry);
  source?.close();
  source = null;
  if (!seesAllMusic()) return changed();
  try {
    const mine = await api("/music/sessions");
    if (mine[0]) {
      session = mine[0];
      // Back in step, asked afresh for the server's clock; where the browser waits for a click
      // to play, Play catches up.
      await refresh();
    }
  } catch {
    // Picked up when the tab is opened instead.
  }
  changed();
}

export async function startSession() {
  took(await api("/music/sessions", { method: "POST", body: here() }));
  announce(`Session started. Its code is ${session.code.split("").join(" ")}.`);
}

export async function joinSession(code) {
  took(await api("/music/sessions/join", { method: "POST", body: { code } }));
  announce(`Joined ${session.host}'s session`);
  follow(session);
}

export async function leaveSession() {
  if (!session) return;
  await api(`/music/sessions/${session.id}/leave`, { method: "POST" });
  ended("You left the session. The music goes on here.");
}

export async function endSession() {
  if (!session) return;
  await api(`/music/sessions/${session.id}`, { method: "DELETE" });
  ended("You ended the session for everyone.");
}

// Whether this user may end the session for everyone: whoever started it, or an admin.
export const mayEnd = () => Boolean(session && me && (session.host === me.username || me.is_admin));
