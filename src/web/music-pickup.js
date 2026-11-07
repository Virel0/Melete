// "Pick up from …" on Listen home: the queue another of the user's devices was playing in the
// last day (GET /aoide/queue), as the Aoide apps offer it (handoff.rs, ResumeGrid). Picking up
// keeps that device's whole list, earlier songs too, and starts at the song it was on, found by
// id, where it was in it; a song this library no longer has is left out, and if it was the one
// playing, the next one starts from its beginning. Not offered to a user who may see only some
// of the music, nor when nothing else was playing.
//
// pickUpCard(ctx) is the card, or null.

import { SignedOut, api, h } from "/web/lib.js";
import { seesAllMusic } from "/web/music-access.js";
import { looseQueue } from "/web/music-play.js";
import { restoreQueue } from "/web/music-player.js";
import { clip, hold, release } from "/web/page-parts.js";

const DAY = 86_400;
const PER_ASK = 100;

// A queue row's track ids, which the apps write as JSON text inside the payload.
function trackIds(payload) {
  const ids = payload?.trackIds;
  if (Array.isArray(ids)) return ids.map(String);
  try {
    const parsed = JSON.parse(ids ?? "[]");
    return Array.isArray(parsed) ? parsed.map(String) : [];
  } catch {
    return [];
  }
}

// Every track of `ids` the library has, by id, asked for a hundred at a time.
async function tracksOf(ids) {
  const found = new Map();
  const wanted = [...new Set(ids)].filter((id) => /^\d+$/.test(id));
  for (let i = 0; i < wanted.length; i += PER_ASK) {
    const chunk = wanted.slice(i, i + PER_ASK);
    const page = await api(`/music/tracks?id=${chunk.join(",")}&limit=${PER_ASK}`);
    for (const t of page.items) found.set(String(t.id), t);
  }
  return found;
}

// The other device's queue as one to play here: the songs found, where to start, and how far in.
function pickUp(ids, position, elapsedMs, found) {
  const tracks = [];
  let start = -1;
  let named = false;
  ids.forEach((id, i) => {
    if (!found.has(id)) return;
    if (start < 0 && i >= position) {
      start = tracks.length;
      named = i === position;
    }
    tracks.push(found.get(id));
  });
  if (!tracks.length) return null;
  return { tracks: looseQueue(tracks), start: start < 0 ? tracks.length - 1 : start, seconds: named ? Math.max(0, elapsedMs) / 1000 : 0 };
}

export async function pickUpCard(ctx) {
  if (!seesAllMusic()) return null;
  try {
    const queues = await api("/aoide/queue");
    const other = queues.find((q) => !q.isCurrentDevice && q.ageSeconds <= DAY && trackIds(q.payload).length);
    if (!other) return null;
    const ids = trackIds(other.payload);
    const position = Math.min(Math.max(Number(other.payload.position) || 0, 0), ids.length - 1);
    const plan = pickUp(ids, position, Number(other.payload.elapsedMs) || 0, await tracksOf(ids));
    if (!plan || !ctx.current()) return null;
    const device = String(other.payload.deviceName ?? "").trim() || "another device";
    const now = plan.tracks[plan.start];
    const button = h("button", { type: "button" }, "Pick up");
    button.addEventListener("click", async () => {
      if (!hold(button)) return;
      const entries = plan.tracks.map((track, i) => ({ track, added: false, order: i }));
      await restoreQueue({ entries, index: plan.start, shuffle: false, repeat: "off", elapsed: plan.seconds }, { play: true });
      release(button);
    });
    return h(
      "section",
      { class: "pick-up", "aria-labelledby": "pick-up-title" },
      h("h2", { id: "pick-up-title" }, `Pick up from ${device}`),
      h("p", {}, clip(now.title, "strong"), now.artist ? [" by ", clip(now.artist)] : null),
      button,
    );
  } catch (err) {
    if (err instanceof SignedOut) ctx.signedOut?.();
    return null;
  }
}
