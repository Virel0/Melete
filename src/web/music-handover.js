// How the player moves from one song to the next without waiting for it to load: the next
// song is put on the spare deck (music-decks.js) twelve seconds before it's wanted, then started
// the moment this one ends (Keep albums whole, or Crossfade off), or faded in over its last
// seconds (Crossfade, by music-mix.js's rules). Like the Aoide apps, the decision is taken
// again at every time update, so a reordered queue, a changed setting or a skip can't leave
// a stale one; a skip, a seek or a pause abandons a fade under way, and a song not ready in
// time is a plain cut instead.
//
// The player hands over itself (startHandOver): `link(track)` asks for a track's stream link,
// `handedOver(at, { sounding })` makes entry `at` the playing one once its deck has taken over
// (`sounding` when it was already playing, as after a fade). finish() is for the end of the
// playing song: true when a hand-over took it, so the player needn't load the next itself.

import { active, audio, empty, fade, hold, spare, start } from "/web/music-decks.js";
import { analysis, measure } from "/web/music-measures.js";
import { fadingIn, fadingOut, plan } from "/web/music-mix.js";
import { listening } from "/web/music-settings.js";
import * as queue from "/web/music-queue.js";

// Seconds before it's wanted that the next song goes on the spare deck.
const LEAD = 12;
// Within this many seconds of the moment, a timer is set for it (time updates come four
// times a second at best).
const SOON = 0.6;
// A gapless start comes this much before the end, for the time a deck takes to sound.
const EARLY = 0.03;
// What a deck must have to start at once (HAVE_FUTURE_DATA), or at least (HAVE_CURRENT_DATA).
const READY = 3;
const STARTABLE = 2;

let hooks = null;
// The hand-over prepared: { key, at, track, ready, failed, running, timer }.
let armed = null;

export function startHandOver(given) {
  hooks = given;
}

// Where a deck's song ends, in its own time.
function endOf(deck) {
  return Number.isFinite(deck.el.duration) ? deck.el.duration : (queue.current()?.duration_seconds ?? NaN);
}

function mixTrack(track) {
  const measured = analysis(track.id);
  return {
    album_id: track.album_id,
    disc_number: track.disc_number,
    track_number: track.track_number,
    seconds: track.duration_seconds,
    bpm: measured?.bpm,
    stability: measured?.bpmStability,
  };
}

function planned(entry) {
  return plan(mixTrack(queue.current()), mixTrack(entry.track), {
    crossfade: Boolean(listening("crossfade")),
    keepAlbums: Boolean(listening("keepAlbums")),
  });
}

// Drops whatever was prepared; a fade under way is undone, the song playing back to full.
export function cancelHandOver() {
  if (!armed) return;
  clearTimeout(armed.timer);
  if (armed.running) hold(active(), 1);
  empty(spare());
  armed = null;
}

async function arm(at, entry) {
  const mine = { key: entry.key, at, track: entry.track, ready: false, failed: false, running: false, timer: 0 };
  armed = mine;
  measure([entry.track]);
  try {
    const got = await hooks.link(entry.track);
    if (armed !== mine) return;
    const deck = spare();
    empty(deck);
    deck.converted = got.format !== "original";
    deck.el.src = got.url;
    mine.ready = true;
  } catch {
    if (armed === mine) mine.failed = true;
  }
}

// The entry a hand-over would bring in, and where: none at the end of the queue, nor for the
// same entry again (repeat one, or a queue of one).
function upcoming() {
  const at = queue.nextIndex();
  const entry = queue.all()[at];
  if (!entry || at === queue.position() || queue.repeating() === "one") return null;
  return { at, entry };
}

// Taken again at every time update while a song plays.
function consider() {
  const deck = active();
  if (!hooks || deck.el.paused || !queue.current()) return;
  const next = upcoming();
  if (!next || (armed && armed.key !== next.entry.key)) cancelHandOver();
  const remaining = endOf(deck) - deck.el.currentTime;
  if (!next || !Number.isFinite(remaining)) return;
  if (armed?.running) return soon(remaining, complete);
  const how = planned(next.entry);
  const overlap = how.kind === "blend" ? how.overlap : EARLY;
  if (!armed && remaining <= overlap + LEAD) arm(next.at, next.entry);
  else if (armed?.ready && !armed.failed) soon(remaining - overlap, () => go(how));
}

// Runs `then` in `seconds`, once it's close enough to time well.
function soon(seconds, then) {
  if (armed.timer || seconds > SOON) return;
  const mine = armed;
  const rate = active().el.playbackRate || 1;
  mine.timer = setTimeout(() => {
    mine.timer = 0;
    if (armed === mine) then();
  }, Math.max(0, (seconds * 1000) / rate));
}

// The moment: the next song starts, at full or fading in.
function go(how) {
  const mine = armed;
  if (!mine || mine.running) return;
  const deck = spare();
  const out = active();
  if (how.kind !== "blend") {
    if (deck.el.readyState < STARTABLE) return; // The song ending takes the plain way.
    start(deck).catch(() => {});
    return complete(false);
  }
  // A fade that can't begin from silence doesn't begin: the song ends, and the next follows.
  if (deck.el.readyState < READY) return;
  const length = Math.max(0.1, endOf(out) - out.el.currentTime);
  hold(deck, 0);
  start(deck).catch(() => cancelHandOver());
  fade(deck, fadingIn, length);
  fade(out, fadingOut, length);
  mine.running = true;
}

// The next song takes over: the player makes it the one playing.
function complete(sounding = true) {
  const mine = armed;
  if (!mine) return;
  clearTimeout(mine.timer);
  armed = null;
  const out = active();
  if (sounding) out.el.pause();
  hooks.handedOver(mine.at, { sounding });
}

// The playing song reached its end: true when the next one was ready and took over.
export function finish() {
  if (!armed || armed.failed) return false;
  if (armed.running) {
    complete(true);
    return true;
  }
  if (!armed.ready || spare().el.readyState < STARTABLE) return false;
  start(spare()).catch(() => {});
  complete(false);
  return true;
}

audio.addEventListener("timeupdate", consider);
// A pause or a seek abandons a fade under way, and puts off a start that was timed.
for (const type of ["pause", "seeking"]) {
  audio.addEventListener(type, () => {
    if (armed?.running) return cancelHandOver();
    if (armed) {
      clearTimeout(armed.timer);
      armed.timer = 0;
    }
  });
}
// A song that won't load on the spare deck is left to the player, which converts it.
for (const deck of [active(), spare()]) {
  deck.el.addEventListener("error", () => {
    if (armed && deck === spare() && deck.el.getAttribute("src")) armed.failed = true;
  });
}
document.addEventListener("melete:queue", () => armed && armed.key !== upcoming()?.entry.key && cancelHandOver());
document.addEventListener("melete:track", (event) => {
  const track = event.detail.track;
  if (track) measure([track, queue.all()[queue.nextIndex()]?.track]);
});
