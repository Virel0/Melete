// Infinity: keeps the queue going, as the Aoide apps do. When it's on and fewer than three songs
// are left after the one playing, the server is asked what follows it (`POST /aoide/next`,
// seeded by the song playing, with what's queued and what this tab played lately), and its
// choices go at the end of the queue in its order, each marked as Infinity's with why it was
// chosen. Songs already queued aren't added twice.
//
// Like the apps: off unless switched on, kept by this browser; one question at a time, asked
// once for each song and length of the queue, so an answer with nothing new isn't asked again
// until the queue moves on; not while the queue repeats (a loop never runs out), nor in a Listen
// Together session (every device would add songs of its own), nor for someone who may see only
// some music libraries (the server refuses them). Private listening doesn't stop it: asking what
// comes next keeps nothing and records no listen.
//
// infinitySwitch() is the switch, for the bar and the panel; each change is sent as
// `melete:infinity`. infinityReason(entry) is what a queue row says about a song Infinity added.

import { api } from "/web/lib.js";
import { seesAllMusic } from "/web/music-access.js";
import { iconButton } from "/web/music-controls.js";
import { goOn } from "/web/music-player.js";
import { looseQueue } from "/web/music-play.js";
import * as queue from "/web/music-queue.js";
import { recentlyPlayed } from "/web/music-radio.js";
import { together } from "/web/music-together.js";

const KEY = "melete.infinity";
// Top up when fewer than this many songs are left after the one playing.
export const MINIMUM_AHEAD = 3;
// How many to ask for each time.
const BATCH = 20;

let on = (() => {
  try {
    return localStorage.getItem(KEY) === "on";
  } catch {
    return false;
  }
})();
// A question under way; and the song and queue length last asked for: { key, length }.
let asking = false;
let askedFor = null;
// The server refused (403): not asked again until the page loads again.
let closed = false;

export const infinityOn = () => on;

export function setInfinity(value) {
  on = Boolean(value);
  try {
    if (on) localStorage.setItem(KEY, "on");
    else localStorage.removeItem(KEY);
  } catch {
    // Held for this page only.
  }
  // Switched on: look at the queue now, not at the next song.
  askedFor = null;
  document.dispatchEvent(new CustomEvent("melete:infinity"));
  topUp();
}

// One line on why the server chose a song, from its factors (each 0 to 1).
function summary(f) {
  if (!f) return "";
  const pct = (value) => `${Math.round(value * 100)}%`;
  const parts = [`Taste ${pct(f.taste)}`];
  if (typeof f.kinship === "number") parts.push(`Kin ${pct(f.kinship)}`);
  parts.push(`Fits ${pct(f.similarity)}`, `Arc ${pct(f.arc)}`);
  if (f.freshness < 0.25) parts.push("heard lately");
  else if (f.freshness < 0.75) parts.push("same artist lately");
  return parts.join(" · ");
}

export function infinityReason(entry) {
  if (typeof entry.infinity !== "string") return null;
  return entry.infinity ? `Infinity · ${entry.infinity}` : "Infinity";
}

// Whether to ask now, and if so, for which entry.
function due() {
  if (!on || asking || closed || !seesAllMusic() || together().session) return null;
  if (queue.repeating() !== "off") return null;
  const all = queue.all();
  const seed = all[queue.position()];
  if (!seed || all.length - queue.position() - 1 >= MINIMUM_AHEAD) return null;
  if (askedFor?.key === seed.key && askedFor.length === all.length) return null;
  return seed;
}

async function topUp() {
  const seed = due();
  if (!seed) return;
  asking = true;
  askedFor = { key: seed.key, length: queue.all().length };
  try {
    const known = new Set(queue.all().map((e) => String(e.track.id)));
    const after = queue
      .all()
      .slice(queue.position() + 1)
      .map((e) => String(e.track.id));
    const next = await api("/aoide/next", {
      method: "POST",
      body: { seed: String(seed.track.id), queue: after, recent: recentlyPlayed(), mode: "infinity", limit: BATCH },
    });
    const chosen = next.candidates.filter((c) => !known.has(String(c.id)));
    if (!chosen.length) return;
    const ids = chosen.map((c) => String(c.id));
    const found = await api(`/music/tracks?${new URLSearchParams({ id: ids.join(","), limit: String(ids.length) })}`);
    // The queue was replaced or stopped meanwhile, or Infinity switched off: not for this one.
    if (!on || !queue.all().includes(seed) || together().session) return;
    const byId = new Map(found.items.map((t) => [String(t.id), t]));
    const why = new Map(chosen.map((c) => [String(c.id), summary(c.factors)]));
    const still = new Set(queue.all().map((e) => String(e.track.id)));
    const tracks = ids.filter((id) => byId.has(id) && !still.has(id)).map((id) => byId.get(id));
    if (!tracks.length) return;
    queue.extend(looseQueue(tracks), tracks.map((t) => why.get(String(t.id))));
    goOn();
  } catch (err) {
    // Closed to this listener; anything else ends with nothing, as the queue would have.
    if (err.status === 403) closed = true;
  } finally {
    asking = false;
    // The queue may have moved on while the question was out.
    topUp();
  }
}

// The switch: pressed while Infinity is on. Hidden from someone to whom it's closed.
export function infinitySwitch() {
  const button = iconButton("infinity", "Infinity", () => setInfinity(!on));
  button.title = "Infinity: keep the queue going with songs chosen for you";
  const follow = () => {
    button.setAttribute("aria-pressed", String(on));
    button.hidden = !seesAllMusic();
  };
  document.addEventListener("melete:infinity", follow);
  document.addEventListener("melete:track", follow);
  follow();
  return button;
}

document.addEventListener("melete:queue", () => topUp());
document.addEventListener("melete:track", () => topUp());
document.addEventListener("melete:together", () => topUp());
