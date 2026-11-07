// A visualizer for the Now playing panel: the music's spectrum as bars, from the player's
// analyser (music-decks.js), taken after the equaliser and before the volume, so it shows the
// music, not where the volume slider is. Decorative, so hidden from screen readers. It draws
// only while it's on the page, shown and the music plays, never when the device asks for less
// motion, and not at all when Visualizer is off under Your account → Listening.

import { h } from "/web/lib.js";
import { analyser, audio } from "/web/music-decks.js";
import { listening } from "/web/music-settings.js";

const BARS = 24;
const calm = matchMedia("(prefers-reduced-motion: reduce)");

export function visualizer() {
  const canvas = h("canvas", { class: "visualizer", width: "240", height: "48", "aria-hidden": "true" });
  let frame = 0;
  let levels = null;

  const draw = () => {
    frame = 0;
    const tap = analyser();
    const pen = canvas.getContext("2d");
    if (!tap || !pen || !wanted()) return stop();
    if (!levels || levels.length !== tap.frequencyBinCount) levels = new Uint8Array(tap.frequencyBinCount);
    tap.getByteFrequencyData(levels);
    const { width, height } = canvas;
    pen.clearRect(0, 0, width, height);
    pen.fillStyle = getComputedStyle(canvas).color;
    const gap = 2;
    const bar = (width - gap * (BARS - 1)) / BARS;
    // Log-spaced bands from about 40 Hz up, as the ear hears them.
    const nyquist = tap.context.sampleRate / 2;
    for (let i = 0; i < BARS; i++) {
      const from = Math.floor(((40 * (nyquist / 40) ** (i / BARS)) / nyquist) * levels.length);
      const to = Math.max(from + 1, Math.floor(((40 * (nyquist / 40) ** ((i + 1) / BARS)) / nyquist) * levels.length));
      let peak = 0;
      for (let j = from; j < Math.min(to, levels.length); j++) peak = Math.max(peak, levels[j]);
      const tall = Math.max(2, (peak / 255) * height);
      pen.fillRect(i * (bar + gap), height - tall, bar, tall);
    }
    frame = requestAnimationFrame(draw);
  };
  const shown = () => listening("visualizer") && !calm.matches;
  const wanted = () => canvas.isConnected && !canvas.closest("[hidden]") && shown() && !audio.paused && !document.hidden;
  const stop = () => {
    cancelAnimationFrame(frame);
    frame = 0;
    canvas.hidden = !shown();
    canvas.getContext("2d")?.clearRect(0, 0, canvas.width, canvas.height);
  };
  const go = () => {
    canvas.hidden = !shown();
    if (!frame && wanted()) frame = requestAnimationFrame(draw);
  };
  for (const type of ["play", "playing"]) audio.addEventListener(type, go);
  document.addEventListener("melete:panel", go);
  document.addEventListener("melete:listening", go);
  document.addEventListener("visibilitychange", go);
  calm.addEventListener("change", go);
  go();
  return canvas;
}
