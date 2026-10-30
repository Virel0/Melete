// The player's keyboard shortcuts, the list of them under the video, and full screen.

import { announce, h } from "/web/lib.js";

// The height of the video's own controls along its bottom, roughly, in CSS pixels.
const CONTROLS_BAND = 56;

// Full screen is the stage's: the picture with Skip and the next episode over it. Returns
// `fullscreen()`, which goes in or out, and `onFullscreen`, the fullscreenchange listener for
// the page to add and remove.
export function stageFullscreen(video, stage) {
  const fullscreen = () => (document.fullscreenElement ? document.exitFullscreen() : stage.requestFullscreen?.());
  // Double-clicking the picture goes full screen as players do, but not on the controls along
  // its bottom, where two quick clicks on Play mean something else.
  video.addEventListener("dblclick", (event) => {
    if (event.offsetY > video.clientHeight - CONTROLS_BAND) return;
    event.preventDefault();
    fullscreen()?.catch?.(() => {});
  });
  // The video's own request (its button, where a browser shows one in spite of `controlslist`)
  // would leave Skip and the next episode behind, so the stage takes over. A browser that wants
  // a fresh click for that keeps the video full screen, as asked. Leaving the stage can land on
  // the video under it; that is left too.
  let last = null;
  const onFullscreen = () => {
    const now = document.fullscreenElement;
    if (now === video) (last === stage ? document.exitFullscreen() : stage.requestFullscreen?.())?.catch?.(() => {});
    last = now;
  };
  return { fullscreen, onFullscreen };
}

// Page-wide, as YouTube's and Jellyfin's players have them; keys typed into a field, or
// pressed on a button or link, are theirs. The video's own controls take Space and the
// arrows while it has focus. `more` gives what the page does for the rest: `subtitles()` for
// C (the next subtitle choice), `next()` for N (the next episode), `fullscreen()` for F.
// Returns the keydown listener, for the page to add and remove.
export function playerKeys(video, more = {}) {
  return (event) => {
    if (event.ctrlKey || event.metaKey || event.altKey || event.defaultPrevented) return;
    const target = event.target;
    if (target.closest?.("input, select, textarea, button, a, summary, dialog, [contenteditable]")) return;
    const onVideo = target === video;
    const by = (seconds) => {
      const end = Number.isFinite(video.duration) ? video.duration : Infinity;
      video.currentTime = Math.min(Math.max(video.currentTime + seconds, 0), end);
    };
    const actions = {
      k: () => (video.paused ? video.play().catch(() => {}) : video.pause()),
      " ": onVideo ? null : () => actions.k(),
      j: () => by(-10),
      l: () => by(10),
      ArrowLeft: onVideo ? null : () => by(-5),
      ArrowRight: onVideo ? null : () => by(5),
      f: () => {
        const asked = document.fullscreenElement ? document.exitFullscreen() : (more.fullscreen?.() ?? video.requestFullscreen?.());
        // Refused (an iframe without permission, say): the player's own button is there.
        asked?.catch?.(() => {});
      },
      m: () => {
        video.muted = !video.muted;
        announce(video.muted ? "Muted" : "Sound on");
      },
      c: more.subtitles ?? null,
      n: more.next ?? null,
    };
    const run = actions[event.key.length === 1 ? event.key.toLowerCase() : event.key];
    if (!run) return;
    event.preventDefault();
    run();
  };
}

// "Keyboard shortcuts", folded, under the video.
export function shortcutsHelp() {
  return h(
    "details",
    { class: "shortcuts" },
    h("summary", {}, "Keyboard shortcuts"),
    h(
      "dl",
      {},
      [
        [["K", "Space"], "Play or pause"],
        [["J", "L"], "Back or forward 10 seconds"],
        [["←", "→"], "Back or forward 5 seconds"],
        [["C"], "Next subtitles, then off"],
        [["N"], "Next episode"],
        [["F"], "Full screen (or double-click the picture)"],
        [["M"], "Sound off or on"],
      ].map(([keys, what]) => h("div", {}, h("dt", {}, keys.map((k, i) => [i ? " or " : "", h("kbd", {}, k)])), h("dd", {}, what))),
    ),
  );
}
