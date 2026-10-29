// The music queue: what's been played, what's playing, what was added with Play next or Play
// last ("Next up"), and the rest of the album or list it came from ("Then"). The player
// (music-player.js) plays from it, the bar shows its shuffle and repeat, and the Queue panel
// (music-panel.js) lists, reorders and trims it. Every change is sent as a `melete:queue`
// event, so whatever shows the queue can follow.
//
// Each entry is { track, added, order, key }: `added` for Next up, `order` its place before
// shuffling, `key` a number unique to the entry (one track can be queued twice). Shuffle mixes
// the rest after the playing track and puts it back in order when turned off; Next up keeps
// its order either way. Repeat is "off", "all" (back to the start after the last track) or
// "one" (the playing track again).

let entries = [];
let index = -1;
let shuffle = false;
let repeat = "off";
let keys = 0;

const entry = (track, added, order) => ({ track, added, order, key: ++keys });

function changed() {
  document.dispatchEvent(new CustomEvent("melete:queue"));
}

// Mixes the entries after the playing one that weren't added with Play next or Play last.
function mixRest() {
  const after = entries.slice(index + 1);
  const added = after.filter((e) => e.added);
  const rest = after.filter((e) => !e.added);
  for (let i = rest.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [rest[i], rest[j]] = [rest[j], rest[i]];
  }
  entries = [...entries.slice(0, index + 1), ...added, ...rest];
}

// A new queue of `tracks`, playing from `start`. With shuffle on, the playing track stays and
// the rest is mixed.
export function setQueue(tracks, start = 0) {
  entries = tracks.map((t, i) => entry(t, false, i));
  index = Math.min(Math.max(start, 0), entries.length - 1);
  if (shuffle) mixRest();
  changed();
}

export function clear() {
  entries = [];
  index = -1;
  changed();
}

export const current = () => entries[index]?.track ?? null;
export const position = () => index;
export const all = () => entries;
export const shuffled = () => shuffle;
export const repeating = () => repeat;

// Where Next goes from the playing track: the following one, the first again when repeating
// everything, or -1 at the end. Repeat one is the player's to do, when a track ends by itself.
export function nextIndex() {
  if (index + 1 < entries.length) return index + 1;
  return repeat === "all" && entries.length ? 0 : -1;
}

export function previousIndex() {
  if (index > 0) return index - 1;
  return repeat === "all" && entries.length ? entries.length - 1 : -1;
}

// Moves to entry `i`; an added entry played leaves Next up.
export function jump(i) {
  if (i < 0 || i >= entries.length) return null;
  index = i;
  entries[i].added = false;
  changed();
  return entries[i].track;
}

// Puts `tracks` first in Next up.
export function playNext(tracks) {
  const added = tracks.map((t) => entry(t, true, Infinity));
  entries.splice(index + 1, 0, ...added);
  if (index < 0) index = 0;
  changed();
}

// Puts `tracks` at the end of Next up, before the rest.
export function playLast(tracks) {
  let at = index + 1;
  while (at < entries.length && entries[at].added) at++;
  entries.splice(at, 0, ...tracks.map((t) => entry(t, true, Infinity)));
  if (index < 0) index = 0;
  changed();
}

// Which part of the queue entry `i` is in: "played", "now", "next" or "rest".
export function partOf(i) {
  if (i < index) return "played";
  if (i === index) return "now";
  return entries[i].added ? "next" : "rest";
}

// Moves entry `i` one place up (-1) or down (1), within its own part only.
export function move(i, by) {
  const j = i + by;
  if (j < 0 || j >= entries.length || partOf(i) !== partOf(j) || partOf(i) === "now") return false;
  [entries[i], entries[j]] = [entries[j], entries[i]];
  changed();
  return true;
}

// Takes entry `i` out; the playing one stays (stop or skip it instead).
export function remove(i) {
  if (i === index || i < 0 || i >= entries.length) return false;
  entries.splice(i, 1);
  if (i < index) index -= 1;
  changed();
  return true;
}

// Empties everything after the playing track: Next up and the rest.
export function clearAfter() {
  entries = entries.slice(0, index + 1);
  changed();
}

export function toggleShuffle() {
  shuffle = !shuffle;
  if (shuffle) mixRest();
  else {
    const after = entries.slice(index + 1);
    const rest = after.filter((e) => !e.added).sort((a, b) => a.order - b.order);
    entries = [...entries.slice(0, index + 1), ...after.filter((e) => e.added), ...rest];
  }
  changed();
}

const REPEATS = ["off", "all", "one"];
export function cycleRepeat() {
  repeat = REPEATS[(REPEATS.indexOf(repeat) + 1) % REPEATS.length];
  changed();
}
