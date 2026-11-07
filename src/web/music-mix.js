// How one song becomes the next, the rules the Aoide apps share (MixTransition.swift, mix.rs):
// pure arithmetic over numbers the library already holds, so nothing here touches a player.
//
// - Keep albums whole (on by default): two tracks of one album in order hand over gaplessly,
//   whatever Crossfade says, so a record sequenced to run on still does.
// - Crossfade off (the default): a cut, the next track starting the moment this one ends.
// - Crossfade on: an 8-second blend when both tempi agree (within 6 %, in any octave, and both
//   holding steady), 4 seconds otherwise or when nothing was measured. No blend takes more than
//   a quarter of the shorter track, and one shorter than 2 seconds is a cut instead. A track of
//   unknown length is cut, never faded over a guess.
//
// It isn't beat-matching: tempo only decides between the long blend and the short one.
//
// A track here is { album_id, disc_number, track_number, seconds, bpm, stability }: `seconds`
// how much of it sounds (its length, less any silence trimmed), `bpm` and `stability` as
// /aoide/audio-analysis measured them.

export const MATCHED_BLEND = 8;
export const UNMATCHED_BLEND = 4;
const SHARE = 0.25;
const SHORTEST = 2;
const TOLERANCE = 0.06;
const STEADY = 0.5;

// { kind: "gapless" | "cut" | "blend", overlap } for handing `from` over to `to`.
export function plan(from, to, { crossfade = false, keepAlbums = true } = {}) {
  if (keepAlbums && albumRun(from, to)) return { kind: "gapless", overlap: 0 };
  if (!crossfade) return { kind: "cut", overlap: 0 };
  const wanted = tempiAgree(from, to) ? MATCHED_BLEND : UNMATCHED_BLEND;
  if (!(from.seconds > 0) || !(to.seconds > 0)) return { kind: "cut", overlap: 0 };
  const overlap = Math.min(wanted, Math.min(from.seconds, to.seconds) * SHARE);
  return overlap >= SHORTEST ? { kind: "blend", overlap } : { kind: "cut", overlap: 0 };
}

// The next track of the same album: the following number on one disc, or the first of the
// next disc. An album on shuffle isn't a run, nor track 1 after track 9 of the same record.
export function albumRun(from, to) {
  if (from.album_id == null || from.album_id !== to.album_id) return false;
  const [a, b] = [from.track_number, to.track_number];
  if (!Number.isInteger(a) || !Number.isInteger(b)) return false;
  const [discA, discB] = [from.disc_number ?? 1, to.disc_number ?? 1];
  return (discA === discB && b === a + 1) || (discB === discA + 1 && b === 1);
}

// A tempo describes its track when it holds for at least half of it; an unknown stability
// (an older measurement) isn't held against it.
const holds = (stability) => stability == null || stability >= STEADY;

// Near enough for the long blend: the ratio folded into one octave, as 85 and 170 are one
// groove counted two ways.
export function tempiAgree(from, to) {
  if (!holds(from.stability) || !holds(to.stability)) return false;
  const [a, b] = [from.bpm, to.bpm];
  if (!(a > 0) || !(b > 0) || !Number.isFinite(a) || !Number.isFinite(b)) return false;
  let ratio = a / b;
  while (ratio > 1.5) ratio /= 2;
  while (ratio < 1 / 1.5) ratio *= 2;
  return Math.abs(ratio - 1) <= TOLERANCE;
}

// The equal-power curves, `progress` running 0 to 1: two songs at half volume each aren't as
// loud as one, so a straight line would dip in the middle of every blend.
const clamp = (p) => (Number.isFinite(p) ? Math.min(1, Math.max(0, p)) : 0);
export const fadingOut = (progress) => Math.cos((clamp(progress) * Math.PI) / 2);
export const fadingIn = (progress) => Math.sin((clamp(progress) * Math.PI) / 2);
