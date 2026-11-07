// How large subtitles are drawn: Small, Medium (as the browser or the track has them), Large or
// Extra large, chosen in the player's Subtitles panel and kept by this browser. Text tracks
// follow through `::cue` (watch.css, by the video's `data-subtitle-size`); styled ASS/SSA tracks
// drawn over the video have their styles' font sizes scaled (player-ass.js `scaleStyled`).
//
// sizePicker(video, onScale) → the picker; `onScale(scale)` hears each change, and once at the
// start with the kept size.

import { announce, h } from "/web/lib.js";

const KEY = "melete.subtitle-size";

// Each size, its name, and how much larger than as made.
export const SUBTITLE_SIZES = [
  { size: "small", label: "Small", scale: 0.75 },
  { size: "medium", label: "Medium", scale: 1 },
  { size: "large", label: "Large", scale: 1.3 },
  { size: "extra-large", label: "Extra large", scale: 1.6 },
];
const MEDIUM = SUBTITLE_SIZES[1];

// The size this browser keeps; Medium when none is kept, or storage is turned off.
function keptSize() {
  try {
    const kept = localStorage.getItem(KEY);
    return SUBTITLE_SIZES.find((s) => s.size === kept) ?? MEDIUM;
  } catch {
    return MEDIUM;
  }
}

function keep(size) {
  try {
    localStorage.setItem(KEY, size.size);
  } catch {
    // Storage turned off: the size holds for this video, and is forgotten after.
  }
}

export function sizePicker(video, onScale) {
  const kept = keptSize();
  const select = h(
    "select",
    { id: "subtitle-size" },
    SUBTITLE_SIZES.map((s) => h("option", { value: s.size, selected: s === kept }, s.label)),
  );
  const apply = (size) => {
    video.dataset.subtitleSize = size.size;
    onScale(size.scale);
  };
  select.addEventListener("change", () => {
    const size = SUBTITLE_SIZES.find((s) => s.size === select.value) ?? MEDIUM;
    apply(size);
    keep(size);
    announce(`Subtitle size: ${size.label}`);
  });
  apply(kept);
  return h("div", {}, h("label", { for: "subtitle-size" }, "Size"), select);
}
