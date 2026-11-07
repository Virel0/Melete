// The Listening group's sound choices (music-settings.js): the equaliser, its own curve, the
// compressor, the visualizer and the output device. Each counts at once and is kept by this
// browser; `said` is the group's status line.

import { h } from "/web/lib.js";
import { FREQUENCIES, GAIN_LIMIT, PRESETS, canChooseOutput, clampGain, outputs } from "/web/music-sound.js";
import { choose, listening } from "/web/music-settings.js";

const hz = (f) => (f >= 1000 ? `${f / 1000} kHz` : `${f} Hz`);

function keep(name, value, said, words) {
  said.textContent = choose(name, value) ? words : "This browser can't keep it: its storage is turned off. It holds until the page is reloaded.";
}

// The equaliser: Off, a preset, or Custom with a slider a band.
function equaliser(said) {
  const select = h(
    "select",
    { id: "listening-eq", "aria-describedby": "listening-eq-hint" },
    [["Off", "Off"], ...PRESETS.map(([name]) => [name, name]), ["Custom", "My own"]].map(([value, label]) =>
      h("option", { value, selected: value === listening("eq") }, label),
    ),
  );
  const sliders = FREQUENCIES.map((f, i) => {
    const input = h("input", { type: "range", min: -GAIN_LIMIT, max: GAIN_LIMIT, step: "1", value: clampGain(listening("eqGains")?.[i]), id: `listening-eq-${i}` });
    const value = h("output", { for: `listening-eq-${i}` });
    const show = () => {
      const db = Number(input.value);
      value.textContent = `${db > 0 ? "+" : ""}${db} dB`;
      input.setAttribute("aria-valuetext", `${db} decibels`);
    };
    show();
    input.addEventListener("input", show);
    input.addEventListener("change", () => {
      const gains = FREQUENCIES.map((_, j) => clampGain(listening("eqGains")?.[j]));
      gains[i] = Number(input.value);
      keep("eqGains", gains, said, `${hz(f)}: ${input.value} dB.`);
    });
    return h("div", { class: "eq-band" }, h("label", { for: `listening-eq-${i}` }, hz(f)), input, value);
  });
  const custom = h("div", { class: "eq-bands", role: "group", "aria-label": "My own equaliser curve", hidden: listening("eq") !== "Custom" }, sliders);
  select.addEventListener("change", () => {
    custom.hidden = select.value !== "Custom";
    keep("eq", select.value, said, `Equaliser: ${select.selectedOptions[0].textContent}.`);
  });
  return h(
    "div",
    { class: "field" },
    h("label", { for: "listening-eq" }, "Equaliser"),
    h("p", { class: "note", id: "listening-eq-hint" }, "Ten bands an octave apart. Each preset is turned down by as much as it boosts, so a loud song never clips."),
    select,
    custom,
  );
}

// The output device, where the browser lets a page choose one and names its devices.
function output(said) {
  const box = h("div", { class: "field" });
  const fallback = (words) => box.replaceChildren(h("p", { class: "note" }, words));
  if (!canChooseOutput()) {
    fallback("This browser plays through your system's sound output; it doesn't let a page choose another.");
    return box;
  }
  outputs().then((list) => {
    if (!list.length) return fallback("This browser plays through your system's sound output; it names its devices only to a page allowed to use the microphone, which Melete doesn't ask for.");
    const chosen = listening("output") ?? "";
    const select = h(
      "select",
      { id: "listening-output" },
      [{ id: "", name: "System default" }, ...list].map((d) => h("option", { value: d.id, selected: d.id === chosen }, d.name)),
    );
    select.addEventListener("change", () => keep("output", select.value, said, `Playing through ${select.selectedOptions[0].textContent}.`));
    box.replaceChildren(h("label", { for: "listening-output" }, "Play through"), select);
  });
  return box;
}

export function soundFields(checkbox, said) {
  return [
    equaliser(said),
    checkbox("compressor", "Compressor", "Bring loud and quiet passages closer together, for a noisy room or low volume.", said),
    checkbox("visualizer", "Visualizer", "Bars that move with the music beside Now playing in the queue panel. Never when your device asks for less motion.", said),
    output(said),
  ];
}
