// What the player does to the sound past the song itself, as the Aoide desktop app has it
// (eq.rs, compressor.rs): a ten-band equaliser with its presets, a compressor, and the output
// device where the browser lets a page choose one. Each is off unless chosen, and kept by this
// browser (music-settings.js). music-decks.js builds the nodes; this says how they're set.
//
// The presets are ten bells an octave apart, each with a preamp that takes the curve's highest
// point back to 0 dB, so a preset never makes a full-scale song clip.

export const FREQUENCIES = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
export const Q = 1.41;
export const GAIN_LIMIT = 12;
export const PRESETS = [
  ["Flat", [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
  ["Bass Boost", [6, 5, 4, 2, 0, 0, 0, 0, 0, 0]],
  ["Bass Reducer", [-6, -5, -4, -2, 0, 0, 0, 0, 0, 0]],
  ["Treble Boost", [0, 0, 0, 0, 0, 1, 2, 4, 5, 6]],
  ["Treble Reducer", [0, 0, 0, 0, 0, -1, -2, -4, -5, -6]],
  ["Vocal", [-2, -2, -1, 0, 2, 4, 4, 2, 0, -1]],
  ["Spoken Word", [-4, -3, -1, 1, 3, 4, 4, 3, 1, -2]],
  ["Loudness", [5, 4, 2, 0, -1, 0, 0, 1, 3, 4]],
  ["Acoustic", [4, 4, 3, 1, 2, 2, 3, 3, 3, 2]],
  ["Electronic", [4, 4, 1, 0, -2, 2, 1, 1, 4, 5]],
  ["Rock", [5, 4, 3, 1, -1, -1, 1, 3, 4, 5]],
  ["Pop", [-1, -1, 0, 2, 4, 4, 2, 0, -1, -1]],
  ["Jazz", [3, 2, 1, 2, -1, -1, 0, 1, 2, 3]],
  ["Classical", [4, 3, 2, 1, -1, -1, 0, 2, 3, 4]],
];
// The desktop's compressor defaults: −20 dB, 3:1, a 6 dB knee, 10 ms attack, 150 ms release.
export const COMPRESSOR = { threshold: -20, ratio: 3, knee: 6, attack: 0.01, release: 0.15 };

// One peaking bell's gain in dB at `f` (the Audio EQ Cookbook's, as Web Audio's peaking
// filter is), at a 48 kHz rate.
function bellDb(f, f0, gainDb, rate = 48000) {
  if (!gainDb) return 0;
  const a = 10 ** (gainDb / 40);
  const w0 = (2 * Math.PI * f0) / rate;
  const alpha = Math.sin(w0) / (2 * Q);
  const [b0, b1, b2] = [1 + alpha * a, -2 * Math.cos(w0), 1 - alpha * a];
  const [a0, a1, a2] = [1 + alpha / a, -2 * Math.cos(w0), 1 - alpha / a];
  const w = (2 * Math.PI * f) / rate;
  const magnitude = (c0, c1, c2) => {
    const re = c0 + c1 * Math.cos(w) + c2 * Math.cos(2 * w);
    const im = -(c1 * Math.sin(w) + c2 * Math.sin(2 * w));
    return Math.hypot(re, im);
  };
  return 20 * Math.log10(magnitude(b0, b1, b2) / magnitude(a0, a1, a2));
}

// The preamp a curve gets: its highest point, over 20 Hz to 20 kHz, brought back to 0 dB
// (rounded up to a tenth), or none for a curve that never rises above it.
export function preampFor(gains) {
  let peak = -Infinity;
  for (let i = 0; i <= 240; i++) {
    const f = 20 * 1000 ** (i / 240);
    peak = Math.max(peak, gains.reduce((sum, g, band) => sum + bellDb(f, FREQUENCIES[band], g), 0));
  }
  return peak > 0 ? -Math.ceil(peak * 10) / 10 : 0;
}

// The gains a choice plays with: a preset's, the listener's own ("Custom"), or none ("Off").
export function gainsFor(choice, custom) {
  if (choice === "Custom") return FREQUENCIES.map((_, i) => clampGain(custom?.[i]));
  return PRESETS.find(([name]) => name === choice)?.[1] ?? null;
}

export const clampGain = (g) => Math.min(GAIN_LIMIT, Math.max(-GAIN_LIMIT, Number.isFinite(Number(g)) ? Number(g) : 0));

// Whether this browser lets a page choose where its Web Audio plays.
export const canChooseOutput = () => typeof window.AudioContext === "function" && typeof AudioContext.prototype.setSinkId === "function";

// The outputs to offer: only when the browser names them (it may not, to a page it hasn't
// let use the microphone), else none.
export async function outputs() {
  if (!canChooseOutput() || !navigator.mediaDevices?.enumerateDevices) return [];
  try {
    const all = (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === "audiooutput" && d.deviceId !== "default");
    return all.every((d) => d.label) ? all.map((d) => ({ id: d.deviceId, name: d.label })) : [];
  } catch {
    return [];
  }
}
