// What Melete measured of the tracks about to play, for the player's hand-overs: each track's
// tempo (/aoide/audio-analysis), which picks a crossfade's length. Asked for the playing track
// and the next as each starts; a track not measured yet is asked about again a few minutes
// later, and asking moves it up the server's list.
//
// measure(tracks) asks for what isn't known; analysis(id) is what's known now (null for
// nothing measured, or nothing came of it).

import { api } from "/web/lib.js";

const AGAIN_AFTER = 5 * 60_000;
const known = new Map();
const asked = new Map();

export const analysis = (id) => known.get(String(id)) ?? null;

export async function measure(tracks) {
  const now = Date.now();
  const ids = [...new Set(tracks.filter(Boolean).map((t) => String(t.id)))].filter(
    (id) => !known.has(id) && !(now - (asked.get(id) ?? -Infinity) < AGAIN_AFTER),
  );
  if (!ids.length) return;
  for (const id of ids) asked.set(id, now);
  try {
    const answer = await api(`/aoide/audio-analysis?ids=${ids.join(",")}`);
    for (const [id, value] of Object.entries(answer.analysis ?? {})) known.set(id, value);
  } catch {
    // Without a measurement a blend is the short one; nothing else depends on it.
  }
}

