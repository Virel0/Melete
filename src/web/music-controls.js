// The music player's controls, for the bar and for the panel's phone layout: shuffle,
// previous, play or pause, next, repeat, and the scrubber with the time played and the time
// left. transport() makes one set, kept in step with what plays by the audio's and the queue's
// events, so two sets can show at once. icon(name) draws one of the player's icons (Watch's
// pages use them too).
//
// Icons are SVG, which look the same everywhere (glyphs turn into emoji on some phones), and
// say nothing to screen readers: their buttons have labels.

import { clock, h } from "/web/lib.js";
import { audio, goNext, goPrevious, seekTo, toggle } from "/web/music-player.js";
import { cycleRepeat, nextIndex, repeating, shuffled, toggleShuffle } from "/web/music-queue.js";

// Filled shapes, then line drawings (drawn with the stroke).
const FILLED = {
  previous: "M6 5h2v14H6zM9.5 12 18 5v14z",
  play: "M7 4.5v15L19.5 12z",
  pause: "M6.5 5h4v14h-4zM13.5 5h4v14h-4z",
  next: "M16 5h2v14h-2zM14.5 12 6 19V5z",
  stop: "M6.5 6.5h11v11h-11z",
  more: "M5 10.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3zm7 0a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3zm7 0a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3z",
  liked: "M12 20s-7.5-4.6-7.5-10.2A4.3 4.3 0 0 1 12 7a4.3 4.3 0 0 1 7.5 2.8C19.5 15.4 12 20 12 20z",
};
const LINES = {
  shuffle: "M3 7h4l10 10h4M3 17h4l3-3m4-4 3-3h4M18 4l3 3-3 3M18 14l3 3-3 3",
  repeat: "M4 11V9a2 2 0 0 1 2-2h13M16 4l3 3-3 3M20 13v2a2 2 0 0 1-2 2H5M8 20l-3-3 3-3",
  "repeat-one": "M4 11V9a2 2 0 0 1 2-2h13M16 4l3 3-3 3M20 13v2a2 2 0 0 1-2 2H5M8 20l-3-3 3-3M11 11l1.5-1v4.5",
  queue: "M4 6h12M4 12h12M4 18h7M16 15l5 3-5 3z",
  lyrics: "M5 4h14a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1h-9l-4 4v-4H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM8 8h8M8 12h5",
  volume: "M4 9v6h4l5 4V5L8 9zM16.5 8.5a5 5 0 0 1 0 7M19 6a8.5 8.5 0 0 1 0 12",
  muted: "M4 9v6h4l5 4V5L8 9zM17 9l5 6M22 9l-5 6",
  up: "M6 15l6-6 6 6",
  down: "M6 9l6 6 6-6",
  close: "M6 6l12 12M18 6 6 18",
  like: "M12 20s-7.5-4.6-7.5-10.2A4.3 4.3 0 0 1 12 7a4.3 4.3 0 0 1 7.5 2.8C19.5 15.4 12 20 12 20z",
  // For Watch: stepping between picks and rows, and My List.
  left: "M15 6l-6 6 6 6",
  right: "M9 6l6 6-6 6",
  plus: "M12 5v14M5 12h14",
  check: "M5 12.5l4.5 4.5L19 7",
};

export function icon(name) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  if (LINES[name]) svg.setAttribute("class", "lines");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", FILLED[name] ?? LINES[name]);
  svg.append(path);
  return svg;
}

export function iconButton(name, label, onclick, extra = "") {
  return h("button", { type: "button", class: `icon ${extra}`.trim(), "aria-label": label, title: label, onclick }, icon(name));
}

const REPEAT_WORDS = { off: "Repeat: off", all: "Repeat: all", one: "Repeat: this track" };

export function transport() {
  const shuffle = iconButton("shuffle", "Shuffle", toggleShuffle);
  const prev = iconButton("previous", "Previous track", goPrevious);
  const play = iconButton("play", "Play", toggle, "play");
  const next = iconButton("next", "Next track", goNext);
  const repeat = iconButton("repeat", REPEAT_WORDS.off, cycleRepeat);
  const elapsed = h("span", { class: "time", "aria-hidden": "true" }, "0:00");
  const left = h("span", { class: "time", "aria-hidden": "true" }, "0:00");
  const seek = h("input", { type: "range", min: "0", max: "0", step: "1", value: "0", "aria-label": "Position" });
  const box = h(
    "div",
    { class: "controls" },
    h("div", { class: "buttons" }, shuffle, prev, play, next, repeat),
    h("div", { class: "position" }, elapsed, seek, left),
  );

  // Dragging the slider moves the playhead when it's let go; it follows the music otherwise.
  let dragging = false;
  const times = (at) => {
    const length = Number.isFinite(audio.duration) ? audio.duration : Number(seek.max);
    elapsed.textContent = clock(at);
    left.textContent = `-${clock(Math.max(0, length - at))}`;
    seek.setAttribute("aria-valuetext", `${clock(at)} of ${clock(length)}`);
  };
  seek.addEventListener("pointerdown", () => (dragging = true));
  seek.addEventListener("input", () => times(Number(seek.value)));
  seek.addEventListener("change", () => {
    seekTo(Number(seek.value));
    dragging = false;
  });
  audio.addEventListener("timeupdate", () => {
    if (dragging) return;
    seek.value = String(Math.floor(audio.currentTime));
    times(Math.floor(audio.currentTime));
  });
  audio.addEventListener("durationchange", () => {
    if (Number.isFinite(audio.duration)) seek.max = String(Math.round(audio.duration));
    times(Math.floor(audio.currentTime));
  });
  const playing = (on) => {
    play.replaceChildren(icon(on ? "pause" : "play"));
    play.setAttribute("aria-label", on ? "Pause" : "Play");
    play.title = on ? "Pause" : "Play";
  };
  audio.addEventListener("play", () => playing(true));
  audio.addEventListener("pause", () => playing(false));
  audio.addEventListener("emptied", () => playing(false));
  const queued = () => {
    shuffle.setAttribute("aria-pressed", String(shuffled()));
    const mode = repeating();
    repeat.replaceChildren(icon(mode === "one" ? "repeat-one" : "repeat"));
    repeat.setAttribute("aria-label", REPEAT_WORDS[mode]);
    repeat.title = REPEAT_WORDS[mode];
    repeat.setAttribute("aria-pressed", String(mode !== "off"));
    // Previous always works: it restarts the track when there's nothing before it.
    next.disabled = nextIndex() < 0;
  };
  document.addEventListener("melete:queue", queued);
  document.addEventListener("melete:track", (event) => {
    const track = event.detail.track;
    seek.max = String(Math.round(track?.duration_seconds ?? 0));
    seek.value = "0";
    times(0);
    play.disabled = event.detail.loading;
    queued();
  });
  queued();
  return box;
}
