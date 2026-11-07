// What both decks' sound goes through on its way to the volume (music-decks.js builds it with the
// rest of the Web Audio graph): a short fade on pause and resume, the equaliser (a preamp and ten
// bells), the compressor when it's on, and a tap for the visualizer, after all of those and
// before the volume, so the bars show the music, not where the volume slider is. It plays
// through the output device chosen, where the browser lets a page choose (music-sound.js). Each
// change in Your account → Listening counts at once, glided to so it doesn't click.

import { COMPRESSOR, FREQUENCIES, Q, canChooseOutput, gainsFor, preampFor } from "/web/music-sound.js";
import { listening } from "/web/music-settings.js";

// Seconds a pause fades out and a resume fades in, as on the desktop app.
export const PAUSE_FADE = 0.15;
// The time constant changes glide with.
const GLIDE = 0.015;

let nodes = null;

// The chain on `context`: { input, output } to put between the decks and the volume.
export function buildChain(context) {
  const bus = context.createGain();
  const preamp = context.createGain();
  const bands = FREQUENCIES.map((frequency) => {
    const band = context.createBiquadFilter();
    band.type = "peaking";
    band.frequency.value = frequency;
    band.Q.value = Q;
    band.gain.value = 0;
    return band;
  });
  const squeeze = context.createDynamicsCompressor();
  for (const [name, value] of Object.entries(COMPRESSOR)) squeeze[name].value = value;
  const out = context.createGain();
  const tap = context.createAnalyser();
  tap.fftSize = 2048;
  tap.smoothingTimeConstant = 0.8;
  [bus, preamp, ...bands].reduce((from, to) => (from.connect(to), to));
  out.connect(tap);
  nodes = { context, bus, preamp, bands, squeeze, out, tap, squeezing: null, sink: "" };
  tune();
  route();
  return { input: bus, output: out };
}

export const analyser = () => nodes?.tap ?? null;
export const chained = () => Boolean(nodes);

// The equaliser and compressor as Listening has them.
function tune() {
  if (!nodes) return;
  const choice = listening("eq");
  const gains = choice && choice !== "Off" ? gainsFor(choice, listening("eqGains")) : null;
  const now = nodes.context.currentTime;
  nodes.preamp.gain.setTargetAtTime(10 ** ((gains ? preampFor(gains) : 0) / 20), now, GLIDE);
  nodes.bands.forEach((band, i) => band.gain.setTargetAtTime(gains?.[i] ?? 0, now, GLIDE));
  const squeezing = Boolean(listening("compressor"));
  if (squeezing === nodes.squeezing) return;
  const last = nodes.bands.at(-1);
  last.disconnect();
  nodes.squeeze.disconnect();
  if (squeezing) {
    last.connect(nodes.squeeze);
    nodes.squeeze.connect(nodes.out);
  } else {
    last.connect(nodes.out);
  }
  nodes.squeezing = squeezing;
}

// The output device chosen; the system's again when it's gone.
function route() {
  if (!nodes || !canChooseOutput()) return;
  const id = listening("output") ?? "";
  if (id === nodes.sink) return;
  nodes.sink = id;
  nodes.context.setSinkId(id).catch(() => {
    nodes.sink = "";
    nodes.context.setSinkId("").catch(() => {});
  });
}

// Fades everything to `value` (0 or 1) over PAUSE_FADE.
export function fadeTo(value) {
  if (!nodes) return;
  const gain = nodes.bus.gain;
  const now = nodes.context.currentTime;
  gain.cancelScheduledValues(now);
  gain.setValueAtTime(gain.value, now);
  gain.linearRampToValueAtTime(value, now + PAUSE_FADE);
}

document.addEventListener("melete:listening", () => {
  tune();
  route();
});
