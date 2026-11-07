// The player's extras, as the Aoide apps have them: Private listening, what you're doing (an
// activity tag on each listen) and the sleep timer.
//
// - Private listening: while it's on, no listen leaves the browser; the one under way when it's
//   switched on ends there, and recording starts again with the next song after it's switched
//   off, so a song half heard in private isn't counted from its middle. Kept for this tab until
//   it's closed or someone signs out.
// - The activity (gaming, focus, chores or commute), sent as `activity` on each listen until
//   it's taken off; kept by this browser.
// - The sleep timer: at the end of this song, at the end of this album, or after 15 to 60
//   minutes, the music pauses.
//
// The player asks privately(), activity(), and sleepsAfter(track, next) and stopsAfter(track,
// next) at a song's end. The menu (music-extras-menu.js) sets them; each change is sent as
// `melete:extras`.

import { choose, listening } from "/web/music-settings.js";

const PRIVATE_KEY = "melete.private-listening";
export const ACTIVITIES = [
  ["gaming", "Gaming"],
  ["focus", "Focus"],
  ["chores", "Chores"],
  ["commute", "Commute"],
];
export const SLEEPS = [
  ["off", "Off"],
  ["song", "End of this song"],
  ["album", "End of this album"],
  ["15", "15 minutes"],
  ["30", "30 minutes"],
  ["45", "45 minutes"],
  ["60", "60 minutes"],
];

function changed() {
  document.dispatchEvent(new CustomEvent("melete:extras"));
}

// ---------- Private listening ----------

let secret = (() => {
  try {
    return sessionStorage.getItem(PRIVATE_KEY) === "on";
  } catch {
    return false;
  }
})();

export const privately = () => secret;

export function setPrivate(on) {
  if (secret === Boolean(on)) return;
  secret = Boolean(on);
  try {
    if (secret) sessionStorage.setItem(PRIVATE_KEY, "on");
    else sessionStorage.removeItem(PRIVATE_KEY);
  } catch {
    // Held for this page only.
  }
  changed();
}

// ---------- What you're doing ----------

export function activity() {
  const kept = listening("activity");
  return ACTIVITIES.some(([value]) => value === kept) ? kept : null;
}

export function setActivity(value) {
  choose("activity", ACTIVITIES.some(([known]) => known === value) ? value : null);
  changed();
}

// ---------- The sleep timer ----------

// { kind, until (for minutes) } while one is set.
let sleep = null;
let sleepTimer = 0;
let pauseNow = () => {};

// What the player does when the time is up: pause.
export function onSleep(pause) {
  pauseNow = pause;
}

export const sleeping = () => sleep;

export function setSleep(kind) {
  clearTimeout(sleepTimer);
  sleep = null;
  if (kind === "song" || kind === "album") sleep = { kind };
  const minutes = Number(kind);
  if (minutes > 0) {
    sleep = { kind, until: Date.now() + minutes * 60_000 };
    sleepTimer = setTimeout(() => {
      sleep = null;
      pauseNow();
      changed();
    }, minutes * 60_000);
  }
  changed();
}

// Whether the music stops when `track` ends rather than going on to `next`: the song timer
// at any song's end, the album timer when the next song is another album's or there's none.
export function sleepsAfter(track, next) {
  if (!sleep || !track) return false;
  return sleep.kind === "song" || (sleep.kind === "album" && (!next || next.album_id !== track.album_id));
}

// As sleepsAfter, at the song's end: a timer that stops the music is spent.
export function stopsAfter(track, next) {
  const stops = sleepsAfter(track, next);
  if (stops) {
    sleep = null;
    changed();
  }
  return stops;
}

// Signing out: no timer or private listening carries over to whoever signs in next.
export function forgetExtras() {
  clearTimeout(sleepTimer);
  sleep = null;
  setPrivate(false);
}
