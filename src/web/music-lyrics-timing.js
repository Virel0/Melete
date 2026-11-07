// Lyrics timing: synced lyrics nudged earlier or later, for a file whose times run a little
// ahead of the recording or behind it. A quarter of a second a press, up to five seconds either
// way, kept by this browser for every song (in localStorage, so it holds across reloads; with
// storage turned off it holds until the page is left). The Lyrics tab lights each line and seeks
// to it by its start plus the offset (music-lyrics.js).
//
// lyricsTiming(onChange) makes the control: { box, seconds() }. seconds() is how much later
// than the file says each line starts (below 0: earlier); onChange() hears each change.

import { announce, h } from "/web/lib.js";

const KEY = "melete.lyrics-offset";
const STEP = 0.25;
const MOST = 5;

function kept() {
  try {
    const value = Number(localStorage.getItem(KEY));
    return Number.isFinite(value) ? Math.min(Math.max(value, -MOST), MOST) : 0;
  } catch {
    return 0;
  }
}

function keep(seconds) {
  try {
    if (seconds) localStorage.setItem(KEY, String(seconds));
    else localStorage.removeItem(KEY);
  } catch {
    // Storage turned off: the offset holds until the page is left.
  }
}

// "On time", "0.5 s later", "1.25 s earlier".
function words(seconds) {
  if (!seconds) return "On time";
  return `${Math.abs(seconds)} s ${seconds > 0 ? "later" : "earlier"}`;
}

export function lyricsTiming(onChange) {
  let offset = kept();
  const said = h("span", { class: "timing-now" });
  const earlier = h("button", { type: "button", class: "quiet small", "aria-label": "Show lyrics earlier" }, "Earlier");
  const later = h("button", { type: "button", class: "quiet small", "aria-label": "Show lyrics later" }, "Later");
  const reset = h("button", { type: "button", class: "quiet small" }, "Reset timing");
  const name = h("span", { class: "timing-name", id: "lyrics-timing-name" }, "Timing");
  const box = h("div", { class: "lyrics-timing", role: "group", "aria-labelledby": "lyrics-timing-name" }, name, earlier, said, later, reset);

  const draw = () => {
    said.textContent = words(offset);
    // Marked rather than disabled at the end of the range, so the keyboard keeps its place.
    earlier.setAttribute("aria-disabled", String(offset <= -MOST));
    later.setAttribute("aria-disabled", String(offset >= MOST));
    reset.hidden = !offset;
  };
  const set = (seconds) => {
    offset = Math.round(Math.min(Math.max(seconds, -MOST), MOST) / STEP) * STEP;
    keep(offset);
    draw();
    announce(`Lyrics timing: ${words(offset)}`);
    onChange();
  };
  earlier.addEventListener("click", () => set(offset - STEP));
  later.addEventListener("click", () => set(offset + STEP));
  reset.addEventListener("click", () => {
    set(0);
    // The button goes once the timing is back on time: keep the keyboard in the group.
    later.focus();
  });
  draw();
  return { box, seconds: () => offset };
}
