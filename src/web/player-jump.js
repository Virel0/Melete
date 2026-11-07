// How far a jump went, on the picture: "+10" or "−10" on the side it went, adding up over a run
// of jumps ("+30"), and gone a moment after the last. Screen readers hear the run's total once
// it settles. It shows under reduced motion too, only without the fade.
//
// A jump is told by a "jumped" event on the video, its detail the seconds asked for (back is
// below zero): `jump(video, seconds)` moves and says so. jumpFlash(video) → { el, stop }: `el`
// goes over the picture, `stop` clears its timer on leaving.

import { announce, h } from "/web/lib.js";

// A jump within this long of the one before adds to it; the flash goes this long after the last.
const RUN_MS = 1000;
const stillness = matchMedia("(prefers-reduced-motion: reduce)");

// Moves `video` by `seconds`, within its length, and tells the flash.
export function jump(video, seconds) {
  const end = Number.isFinite(video.duration) ? video.duration : Infinity;
  video.currentTime = Math.min(Math.max(video.currentTime + seconds, 0), end);
  video.dispatchEvent(new CustomEvent("jumped", { detail: seconds }));
}

export function jumpFlash(video) {
  const el = h("div", { class: "jump-flash", "aria-hidden": "true", hidden: true });
  let total = 0;
  let timer = null;
  const settle = () => {
    timer = null;
    el.hidden = true;
    if (total) announce(`${total > 0 ? "Forward" : "Back"} ${Math.abs(total)} seconds`);
    total = 0;
  };
  video.addEventListener("jumped", ({ detail: seconds }) => {
    // Turning the other way starts a new run.
    if (Math.sign(total) === -Math.sign(seconds)) total = 0;
    total += seconds;
    el.textContent = `${total > 0 ? "+" : "−"}${Math.abs(total)}`;
    el.classList.toggle("back", total < 0);
    el.hidden = false;
    if (!stillness.matches) el.animate([{ opacity: 0.4, scale: 0.85 }, { opacity: 1, scale: 1 }], { duration: 160, easing: "ease-out" });
    clearTimeout(timer);
    timer = setTimeout(settle, RUN_MS);
  });
  return { el, stop: () => clearTimeout(timer) };
}
