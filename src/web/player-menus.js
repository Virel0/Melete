// The control bar's panels: Subtitles, Audio and Settings. Each is a button that shows or hides
// its panel above the bar (a disclosure: aria-expanded and aria-controls), one open at a time.
// Escape, a click elsewhere or another panel's button closes it, and Escape puts focus back on
// its button. What's in a panel is the page's own pickers, so a choice does what it always did.

import { announce, clock, h } from "/web/lib.js";
import { icon } from "/web/music-controls.js";

let made = 0;

// The buttons for `panels` ({ name, label, body }) and the host their panels show in. `onChange`
// hears whether any is open, so the bar stays up meanwhile. Returns { buttons, host, close,
// isOpen, stop }.
export function panelButtons(panels, onChange = () => {}) {
  const host = h("div", { class: "control-panels" });
  let open = null;
  const set = (entry) => {
    if (open === entry) return;
    if (open) {
      open.button.setAttribute("aria-expanded", "false");
      open.panel.hidden = true;
    }
    open = entry;
    if (open) {
      open.button.setAttribute("aria-expanded", "true");
      open.panel.hidden = false;
      // Into the panel, on what's chosen when there is something, else its first control.
      const into = open.panel.querySelector('[aria-pressed="true"], select, button, input');
      into?.focus();
    }
    onChange(Boolean(open));
  };
  const entries = panels.map(({ name, label, body }) => {
    const id = `control-panel-${++made}`;
    const panel = h("div", { class: "control-panel", id, role: "group", "aria-label": label, hidden: true }, h("h2", { class: "control-panel-title" }, label), body);
    const button = h("button", { type: "button", class: "icon control", "aria-label": label, title: label, "aria-expanded": "false", "aria-controls": id }, icon(name));
    const entry = { button, panel };
    button.addEventListener("click", () => set(open === entry ? null : entry));
    panel.addEventListener("keydown", (event) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      set(null);
      button.focus();
    });
    host.append(panel);
    return entry;
  });
  // A click outside the open panel and its button closes it; `stop()` takes that listener away
  // on leaving the player.
  const outside = (event) => {
    if (open && !open.panel.contains(event.target) && !open.button.contains(event.target)) set(null);
  };
  document.addEventListener("pointerdown", outside);
  const stop = () => document.removeEventListener("pointerdown", outside);
  return { buttons: entries.map((e) => e.button), host, close: () => set(null), isOpen: () => Boolean(open), stop };
}

// The speeds a video can play at, as the Settings panel offers them.
const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];

// Playback speed, for the Settings panel.
export function speedPicker(video) {
  const select = h(
    "select",
    { id: "playback-speed" },
    SPEEDS.map((speed) => h("option", { value: speed, selected: speed === video.playbackRate }, speed === 1 ? "Normal" : `${speed}×`)),
  );
  select.addEventListener("change", () => {
    video.playbackRate = Number(select.value);
    announce(`Speed: ${select.selectedOptions[0].textContent}`);
  });
  return h("div", {}, h("label", { for: "playback-speed" }, "Speed"), select);
}

// The file's own chapters to jump to, when it has more than one.
export function chapterPicker(video, chapters) {
  if (chapters.length < 2) return null;
  const select = h(
    "select",
    { id: "chapter" },
    h("option", { value: "" }, "Jump to…"),
    chapters.map((c, i) => h("option", { value: i }, `${clock(c.start_seconds)} ${c.title ?? `Chapter ${i + 1}`}`)),
  );
  select.addEventListener("change", () => {
    if (select.value === "") return;
    const chapter = chapters[Number(select.value)];
    video.currentTime = chapter.start_seconds;
    announce(`Chapter: ${select.selectedOptions[0].textContent}`);
    select.value = "";
  });
  return h("div", {}, h("label", { for: "chapter" }, "Chapters"), select);
}
