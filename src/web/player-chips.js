// Subtitle chips under the seek bar: Off and the first few subtitle tracks, the one showing
// pressed, and More for the full list in the Subtitles picker. They and C (the next choice,
// then off) work the picker itself, so everything a choice does (converting, remembering it
// for the series) happens once, in one place.
//
// subtitleChips(select, video) → the row (null without subtitles); nextSubtitle(select) for C.

import { announce, h } from "/web/lib.js";

// How many tracks get a chip of their own; the rest are under More.
const SHOWN = 4;

// Chooses `value` in the picker as if someone had.
function pick(select, value) {
  if (select.value === value) return;
  select.value = value;
  select.dispatchEvent(new Event("change"));
}

export function subtitleChips(select, video) {
  if (!select || select.options.length < 2) return null;
  const row = h("div", { class: "subtitle-chips", role: "group", "aria-label": "Subtitles" });
  const draw = () => {
    const options = [...select.options];
    const chips = options.slice(0, SHOWN + 1).map((o) => {
      const chip = h("button", { type: "button", class: "chip", "aria-pressed": String(o.value === select.value) }, o.value ? shortName(o.textContent) : "Off");
      chip.title = o.textContent;
      chip.addEventListener("click", () => {
        pick(select, o.value);
        announce(`Subtitles: ${o.value ? o.textContent : "off"}`);
      });
      return chip;
    });
    const more =
      options.length > SHOWN + 1
        ? h("button", { type: "button", class: "chip", onclick: () => select.focus() }, `More (${options.length - SHOWN - 1})`)
        : null;
    // Keep focus on the chip pressed through the redraw.
    const at = [...row.children].indexOf(document.activeElement);
    row.replaceChildren(...chips, ...(more ? [more] : []));
    if (at >= 0) row.children[at]?.focus();
  };
  select.addEventListener("change", draw);
  // The video's own captions menu changes the picker without a change event.
  video.textTracks.addEventListener("change", () => setTimeout(draw));
  // Tracks found while playing join the picker; the chips follow.
  new MutationObserver(draw).observe(select, { childList: true });
  draw();
  return row;
}

// "English, SDH" → "English SDH": the picker's name without its commas, for a chip.
function shortName(text) {
  return text.replace(/, /g, " ");
}

// C: the next subtitle choice in the picker, then off again.
export function nextSubtitle(select) {
  if (!select || select.options.length < 2) return announce("No subtitles for this video.");
  const next = (select.selectedIndex + 1) % select.options.length;
  pick(select, select.options[next].value);
  announce(`Subtitles: ${next ? select.options[next].textContent : "off"}`);
}
