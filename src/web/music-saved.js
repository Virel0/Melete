// The queue kept across a reload, and published for the user's other devices.
//
// - Kept: the queue, its place, shuffle, repeat and how far into the song, in this browser's
//   storage under the signed-in user's id, so it's never shown to anyone else signing in here;
//   restored paused when they sign in again, and forgotten on Stop and on signing out. Like the
//   apps, at most 500 entries, no more than 25 of them already played.
// - Published: the same queue as this session's `queue_state` (PUT /music/queue), so an Aoide
//   device can offer "Pick up from …" with it: a few seconds after it changes, every half
//   minute while it plays, and empty on Stop. Not while listening privately, and not for a
//   user who may see only some of the music (the apps' queues are closed to them).
//
// startSaved(user) restores and starts keeping; forgetSaved() is for signing out.

import { api } from "/web/lib.js";
import { seesAllMusic } from "/web/music-access.js";
import { privately } from "/web/music-extras.js";
import { audio, restoreQueue } from "/web/music-player.js";
import * as queue from "/web/music-queue.js";

const PREFIX = "melete.queue.";
const MAX_ENTRIES = 500;
const MAX_PLAYED = 25;
const SETTLE = 3000;
const EVERY = 30_000;
// How often the place in a playing song is kept.
const KEEP_EVERY = 5000;

let user = null;
let settle = 0;
let lastSent = 0;
let lastKept = 0;

// The queue as kept: the window of entries around the playing one, and where it is.
function snapshot() {
  const all = queue.all();
  const index = queue.position();
  if (!all.length || index < 0) return null;
  const from = Math.max(0, Math.min(index - MAX_PLAYED, all.length - MAX_ENTRIES));
  const entries = all.slice(from, from + MAX_ENTRIES);
  return {
    entries: entries.map(({ track, added, order, infinity }) => ({ track, added, order: Number.isFinite(order) ? order : null, infinity })),
    index: index - from,
    shuffle: queue.shuffled(),
    repeat: queue.repeating(),
    elapsed: Number.isFinite(audio.currentTime) ? audio.currentTime : 0,
  };
}

function keep() {
  if (!user) return;
  lastKept = Date.now();
  const saved = snapshot();
  try {
    if (saved) localStorage.setItem(PREFIX + user.id, JSON.stringify(saved));
    else localStorage.removeItem(PREFIX + user.id);
  } catch {
    // Storage off or full: the queue lasts until the page goes.
  }
}

function publish() {
  clearTimeout(settle);
  settle = 0;
  if (!user || privately() || !seesAllMusic()) return;
  const saved = snapshot();
  lastSent = Date.now();
  const body = saved
    ? { track_ids: saved.entries.map((e) => e.track.id), position: saved.index, elapsed_ms: Math.round(saved.elapsed * 1000) }
    : { track_ids: [], position: 0, elapsed_ms: 0 };
  api("/music/queue", { method: "PUT", body }).catch(() => {});
}

function changed() {
  keep();
  clearTimeout(settle);
  settle = setTimeout(publish, SETTLE);
}

export function startSaved(signedIn) {
  user = signedIn;
  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(PREFIX + user.id) ?? "null");
  } catch {
    saved = null;
  }
  if (saved?.entries?.length && !queue.current()) restoreQueue(saved);
}

// Signing out: every queue kept here goes, whoever's it was.
export function forgetSaved() {
  clearTimeout(settle);
  user = null;
  try {
    for (const key of Object.keys(localStorage)) if (key.startsWith(PREFIX)) localStorage.removeItem(key);
  } catch {
    // Nothing kept.
  }
}

document.addEventListener("melete:queue", changed);
document.addEventListener("melete:track", changed);
audio.addEventListener("pause", changed);
audio.addEventListener("timeupdate", () => {
  if (!user || audio.paused) return;
  if (Date.now() - lastKept > KEEP_EVERY) keep();
  if (Date.now() - lastSent > EVERY) publish();
});
window.addEventListener("pagehide", keep);
