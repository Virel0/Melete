// What Melete measured of the tracks about to play, for the player: each track's loudness and
// tempo (/aoide/audio-analysis) and where its sound starts and stops (/aoide/sound-bounds).
// Asked for the playing track and the next as each starts; a track not measured yet is asked
// about again a few minutes later, and asking moves it up the server's list. A `melete:measured`
// event says when new answers are in.
//
// measure(tracks) asks for what isn't known. analysis(id) is what's known now (null for nothing
// measured, or nothing came of it); level(id) the factor Level volume plays it at, and
// bounds(id) the part of it Trim silence plays, as each setting has it.

import { api } from "/web/lib.js";
import { listening } from "/web/music-settings.js";

const AGAIN_AFTER = 5 * 60_000;
// ReplayGain 2.0's reference level, the apps' (LoudnessNormalisation.swift, loudness.rs).
const REFERENCE_LUFS = -18;
const known = { analysis: new Map(), bounds: new Map() };
const asked = new Map();

export const analysis = (id) => known.analysis.get(String(id)) ?? null;

// Decibels to turn a track down by, or null to leave it: attenuation only (a quiet master is
// left alone rather than pushed into clipping), and a master over full scale turned down until
// it isn't, rounded to a hundredth as every app rounds it.
export function gainDb(lufs, peak) {
  if (!Number.isFinite(lufs)) return null;
  let gain = REFERENCE_LUFS - lufs;
  if (gain >= 0) return null;
  if (Number.isFinite(peak) && peak + gain > 0) gain = -peak;
  const rounded = Math.round(gain * 100) / 100;
  return rounded < 0 ? rounded : null;
}

// The factor a track plays at: 1 with Level volume off or nothing measured.
export function level(id) {
  if (!listening("levelVolume")) return 1;
  const measured = analysis(id);
  const db = measured ? gainDb(measured.loudnessLufs, measured.truePeakDbfs) : null;
  return db == null ? 1 : Math.min(1, 10 ** (db / 20));
}

// The part of a track to play, { start, end } in seconds, with Trim silence on and the track
// measured; null to play it whole.
export function bounds(id) {
  if (!listening("trimSilence")) return null;
  const span = known.bounds.get(String(id));
  return span && span.soundEndMs > span.soundStartMs ? { start: span.soundStartMs / 1000, end: span.soundEndMs / 1000 } : null;
}

// Settles once every track asked about is answered, including those already being asked for.
export function measure(tracks) {
  const now = Date.now();
  const wanted = [...new Set(tracks.filter(Boolean).map((t) => String(t.id)))];
  const ids = wanted.filter(
    (id) => !(known.analysis.has(id) && known.bounds.has(id)) && !(now - (asked.get(id)?.at ?? -Infinity) < AGAIN_AFTER),
  );
  if (ids.length) {
    const ask = async (path, field, into) => {
      try {
        const answer = await api(`${path}?ids=${ids.join(",")}`);
        for (const [id, value] of Object.entries(answer[field] ?? {})) into.set(id, value);
      } catch {
        // Unmeasured, a track plays whole, at its own level, with the short blend.
      }
    };
    const answered = Promise.all([ask("/aoide/audio-analysis", "analysis", known.analysis), ask("/aoide/sound-bounds", "bounds", known.bounds)]).then(
      () => document.dispatchEvent(new CustomEvent("melete:measured", { detail: { ids } })),
    );
    for (const id of ids) asked.set(id, { at: now, answered });
  }
  return Promise.all(wanted.map((id) => asked.get(id)?.answered));
}
