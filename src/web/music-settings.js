// How this browser plays music, kept by the browser only (as each Aoide device keeps its own):
// Crossfade, off by default, and Keep albums whole, on, the apps' defaults. A part of Your
// account; the player reads them at each hand-over, so a change counts from the next song.
//
// listening(name) is a choice as kept; choose(name, value) keeps one and says so with a
// `melete:listening` event.

import { group } from "/web/forms.js";
import { h } from "/web/lib.js";

const KEY = "melete.listening";
const DEFAULTS = { crossfade: false, keepAlbums: true };

function read() {
  try {
    const kept = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return { ...DEFAULTS, ...(kept && typeof kept === "object" ? kept : {}) };
  } catch {
    return { ...DEFAULTS };
  }
}

let choices = read();

export const listening = (name) => choices[name];

// Keeps a choice; false when this browser's storage is turned off (it holds until a reload).
export function choose(name, value) {
  choices = { ...choices, [name]: value };
  document.dispatchEvent(new CustomEvent("melete:listening", { detail: { name, value } }));
  try {
    localStorage.setItem(KEY, JSON.stringify(choices));
    return true;
  } catch {
    return false;
  }
}

// A box that keeps its choice the moment it changes, and says so in `said`.
function checkbox(name, label, hint, said) {
  const id = `listening-${name}`;
  const input = h("input", { type: "checkbox", id, checked: listening(name), "aria-describedby": `${id}-hint` });
  input.addEventListener("change", () => {
    const kept = choose(name, input.checked);
    said.textContent = kept ? `${label}: ${input.checked ? "on" : "off"}.` : "This browser can't keep it: its storage is turned off. It holds until the page is reloaded.";
  });
  return h(
    "div",
    {},
    h("div", { class: "check" }, input, h("label", { for: id }, label)),
    h("p", { class: "note check-hint", id: `${id}-hint` }, hint),
  );
}

export function listeningGroup() {
  const said = h("p", { class: "form-result", role: "status" });
  return group(
    "Listening",
    h("p", { class: "note" }, "Kept by this browser only, as each Aoide device keeps its own. A change counts from the next song."),
    checkbox(
      "crossfade",
      "Crossfade",
      "Fade each song into the next: 8 seconds when their tempos match, 4 otherwise, never more than a quarter of a short song. Off, the next song starts the moment one ends.",
      said,
    ),
    checkbox(
      "keepAlbums",
      "Keep albums whole",
      "Songs of one album played in order run into each other without a fade or a gap, as the record was made, even with Crossfade on.",
      said,
    ),
    said,
  );
}
