// The music player's engine: it plays the queue (music-queue.js) through Melete's stream links
// while the pages above change. Each file plays as it is when the browser says it can play it,
// else converted (FLAC where the browser plays FLAC, so lossless stays lossless, else Opus, AAC,
// then MP3). If the browser turns a file down after all, the track is converted instead, once.
// The next track's link is asked for as soon as one starts, so moving on doesn't wait. Media
// keys and the phone's lock screen work through the Media Session API. Every listen is kept in
// the play history when it ends.
//
// For pages: playQueue(tracks, start) plays a list from a track; playNext(tracks) and
// playLast(tracks) add to Next up; nowPlaying() is the track playing (pages mark it, and follow
// the `melete:now-playing` event). A track is { id, title, artist, album, album_id, cover, codec,
// duration_seconds }. The bar (music-bar.js) and the panel (music-panel.js) show it; they follow
// `melete:track` ({ track, loading }) and `melete:status` (a line of words, or "").

import { SignedOut, announce, api } from "/web/lib.js";
import * as queue from "/web/music-queue.js";

// What each codec is, for asking the browser whether it plays it.
const TYPES = {
  flac: "audio/flac",
  mp3: "audio/mpeg",
  aac: 'audio/mp4; codecs="mp4a.40.2"',
  alac: 'audio/mp4; codecs="alac"',
  opus: 'audio/ogg; codecs="opus"',
  vorbis: 'audio/ogg; codecs="vorbis"',
  wav: "audio/wav",
};
// Conversions, best first, and what they are.
const CONVERSIONS = [
  ["flac", "audio/flac"],
  ["opus", 'audio/ogg; codecs="opus"'],
  ["aac", 'audio/mp4; codecs="mp4a.40.2"'],
  ["mp3", "audio/mpeg"],
];
// Seconds into a track that make "previous" restart it rather than go back.
const RESTART_AFTER = 3;

export const audio = new Audio();
audio.preload = "auto";

// The link for each queued track once asked for: { url, format }.
const links = new Map();
let signedOut = () => {};
// The listen under way, kept in the play history when it ends: heard to the end,
// skipped (moved on from, and where), or stopped.
let listen = null;

function endListen({ completed = false, skipped = false } = {}) {
  if (!listen) return;
  const body = {
    ...listen,
    ended_at: Math.floor(Date.now() / 1000),
    position_seconds: Number.isFinite(audio.currentTime) ? audio.currentTime : 0,
    completed,
    skipped,
  };
  listen = null;
  // Keepalive, so a listen ended by closing the page still arrives. Losing one costs little.
  api("/music/plays", { method: "POST", body, keepalive: true }).catch(() => {});
}

function say(text) {
  document.dispatchEvent(new CustomEvent("melete:status", { detail: text }));
}

function playing(track, loading = false) {
  document.dispatchEvent(new CustomEvent("melete:track", { detail: { track, loading } }));
  document.dispatchEvent(new CustomEvent("melete:now-playing", { detail: { id: track?.id ?? null } }));
}

// Starts the player once signed in; `onSignedOut` sends the user to sign in again.
export function startPlayer({ onSignedOut }) {
  signedOut = onSignedOut;
}

export function playQueue(tracks, start = 0) {
  links.clear();
  queue.setQueue(tracks, start);
  load(queue.position());
}

// Adds tracks to Next up; with nothing playing, the first of them starts.
export function playNext(tracks) {
  const idle = !queue.current();
  queue.playNext(tracks);
  if (idle) load(queue.position());
}

export function playLast(tracks) {
  const idle = !queue.current();
  queue.playLast(tracks);
  if (idle) load(queue.position());
}

export function pause() {
  audio.pause();
}

// Stops and empties the player: on signing out, its links stop working anyway.
export function stopPlayer() {
  endListen();
  audio.pause();
  audio.removeAttribute("src");
  audio.load();
  queue.clear();
  links.clear();
  if ("mediaSession" in navigator) navigator.mediaSession.metadata = null;
  playing(null);
}

export function nowPlaying() {
  return queue.current()?.id ?? null;
}

function playable(codec) {
  const type = TYPES[codec];
  return Boolean(type && audio.canPlayType(type));
}

function conversion() {
  return (CONVERSIONS.find(([, type]) => audio.canPlayType(type)) ?? CONVERSIONS[3])[0];
}

// Asks for a track's stream link, converted when `convert` or when the browser can't play it.
async function link(track, convert = false) {
  const known = links.get(track.id);
  if (known && !convert) return known;
  const format = convert || !playable(track.codec) ? conversion() : "original";
  const answer = await api(`/music/tracks/${track.id}/play`, { method: "POST", body: { format } });
  const got = { url: answer.stream_url, format: answer.format };
  links.set(track.id, got);
  return got;
}

// Asks ahead for the next track's link, so moving on doesn't wait for it.
function prefetch() {
  const following = queue.all()[queue.nextIndex()]?.track;
  if (!following || links.has(following.id)) return;
  link(following).catch(() => links.delete(following.id));
}

// Plays queue entry `at`.
async function load(at, { convert = false } = {}) {
  const entry = queue.all()[at];
  if (!entry) return;
  // Moving on to another entry before this one ended is a skip.
  if (listen && (listen.track_id !== entry.track.id || at !== queue.position())) endListen({ skipped: true });
  const track = queue.jump(at);
  playing(track, true);
  say(convert ? "Converting…" : "Loading…");
  mediaSession(track);
  try {
    const got = await link(track, convert);
    if (queue.current() !== track) return; // Moved on meanwhile.
    audio.dataset.converted = got.format === "original" ? "" : "yes";
    audio.src = got.url;
    await audio.play();
    say("");
    announce(`Playing ${track.title} by ${track.artist}`);
    prefetch();
  } catch (err) {
    if (err instanceof SignedOut) {
      stopPlayer();
      return signedOut();
    }
    // The browser refuses to start without a click (autoplay): wait for one.
    if (err.name === "NotAllowedError") return say("");
    // A file the browser turned down: the audio's error event converts it, once.
    if (err.name === "NotSupportedError") return;
    say(`Couldn't play this track: ${err.message}`);
  } finally {
    if (queue.current() === track) playing(track, false);
  }
}

function mediaSession(track) {
  if (!("mediaSession" in navigator)) return;
  navigator.mediaSession.metadata = new MediaMetadata({
    title: track.title,
    artist: track.artist ?? "",
    album: track.album ?? "",
    artwork: track.cover ? [{ src: new URL(track.cover, location.href).href }] : [],
  });
}

export function toggle() {
  if (!queue.current()) return;
  if (audio.paused) audio.play().catch(() => {});
  else audio.pause();
}

export function goNext() {
  const at = queue.nextIndex();
  if (at >= 0) load(at);
}

export function goPrevious() {
  const at = queue.previousIndex();
  if (audio.currentTime > RESTART_AFTER || at < 0) audio.currentTime = 0;
  else load(at);
}

// Plays queue entry `at` now (a row chosen in the queue).
export function jumpTo(at) {
  load(at);
}

export function seekTo(seconds) {
  if (queue.current() && Number.isFinite(seconds)) audio.currentTime = seconds;
}

// A listen starts when the track first plays, not when it's chosen. It's sent then too, so
// the household sees it playing when sharing is on; sent again with the same id, it
// ends.
audio.addEventListener("playing", () => {
  const track = queue.current();
  if (!track || listen) return;
  const id = `web-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  listen = { id, track_id: track.id, started_at: Math.floor(Date.now() / 1000) };
  api("/music/plays", { method: "POST", body: listen }).catch(() => {});
});
window.addEventListener("pagehide", () => endListen());
audio.addEventListener("play", () => {
  if ("mediaSession" in navigator) navigator.mediaSession.playbackState = "playing";
});
audio.addEventListener("pause", () => {
  if ("mediaSession" in navigator) navigator.mediaSession.playbackState = "paused";
});
audio.addEventListener("ended", () => {
  endListen({ completed: true });
  // Repeating one track plays it again, as a new listen.
  if (queue.repeating() === "one") {
    audio.currentTime = 0;
    audio.play().catch(() => {});
    return;
  }
  const at = queue.nextIndex();
  if (at >= 0) return load(at);
  say("End of the queue");
});
// A file the browser can't decode after all shows here, not as a rejected play().
audio.addEventListener("error", () => {
  if (!queue.current() || !audio.getAttribute("src")) return;
  if (audio.dataset.converted !== "yes") {
    links.delete(queue.current().id);
    load(queue.position(), { convert: true });
  } else {
    say("Couldn't play this track.");
  }
});

if ("mediaSession" in navigator) {
  const actions = {
    play: () => audio.play().catch(() => {}),
    pause: () => audio.pause(),
    previoustrack: goPrevious,
    nexttrack: goNext,
    seekto: (details) => seekTo(details.seekTime),
  };
  for (const [action, handler] of Object.entries(actions)) {
    try {
      navigator.mediaSession.setActionHandler(action, handler);
    } catch {
      // A browser without this action.
    }
  }
}
