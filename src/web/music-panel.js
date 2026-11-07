// The Queue | Lyrics panel, opened from the music bar: beside the page at 1100 px and wider (the
// page makes room for it), over the page when narrower, with focus kept in it until it closes.
// On a phone it also carries the player's controls and Stop, which the bar leaves out there.
// Full screen fills the window with Now playing: the cover large (its motion cover looping muted
// over it), the controls under it, the tabs beside it on a wide screen and under it on a phone,
// all in the album's cover colours (music-now.js). Escape leaves full screen; Close closes.
//
// - Queue: Played (the last few, the rest on request), Now playing, Next up (added with Play
//   next or Play last), then the rest. A row plays when chosen; Move up, Move down and Remove
//   work within its part; "Clear the rest" empties everything after the playing track.
// - Lyrics: music-lyrics.js.
// - Together: listening together with others in the household, music-together-tab.js.
// The tabs are a tablist: arrow keys move between them. Escape or Close closes the panel, and
// focus goes back to what opened it. togglePanel(tab) opens it on a tab, or closes it when it's
// already open there; its state is sent as `melete:panel` ({ open, tab }).

import { clock, h } from "/web/lib.js";
import { audio, jumpTo, seekTo, stopPlayer } from "/web/music-player.js";
import * as queue from "/web/music-queue.js";
import { clip, emptyState } from "/web/page-parts.js";
import { icon, iconButton, transport } from "/web/music-controls.js";
import { lyricsTab } from "/web/music-lyrics.js";
import { nowLook } from "/web/music-now.js";
import { togetherTab } from "/web/music-together-tab.js";

const wide = matchMedia("(min-width: 68.75rem)");
const PLAYED_SHOWN = 3;

const lyrics = lyricsTab(audio, seekTo);
const queueBody = h("div", { class: "queue" });
const together = togetherTab();
const tabs = {
  queue: h("button", { type: "button", role: "tab", id: "panel-tab-queue", "aria-controls": "panel-queue" }, "Queue"),
  lyrics: h("button", { type: "button", role: "tab", id: "panel-tab-lyrics", "aria-controls": "panel-lyrics" }, "Lyrics"),
  together: h("button", { type: "button", role: "tab", id: "panel-tab-together", "aria-controls": "panel-together" }, "Together"),
};
const bodies = {
  queue: h("div", { role: "tabpanel", id: "panel-queue", "aria-labelledby": "panel-tab-queue", tabindex: "0" }, queueBody),
  lyrics: h("div", { role: "tabpanel", id: "panel-lyrics", "aria-labelledby": "panel-tab-lyrics", tabindex: "0" }, lyrics.body),
  together: h("div", { role: "tabpanel", id: "panel-together", "aria-labelledby": "panel-tab-together", tabindex: "0" }, together.body),
};
const cover = h("img", { alt: "" });
const coverBox = h("div", { class: "cover" }, cover);
const title = h("p", { class: "now-title" });
const artist = h("p", { class: "now-artist" });
const fullButton = iconButton("fullscreen", "Full screen", () => setFull(!full));
fullButton.setAttribute("aria-pressed", "false");
const panel = h(
  "dialog",
  { class: "music-panel", "aria-labelledby": "panel-heading" },
  h(
    "div",
    { class: "panel-head" },
    h("h2", { id: "panel-heading" }, "Now playing"),
    h("div", { class: "panel-head-tools" }, fullButton, h("button", { type: "button", class: "quiet", onclick: () => closePanel() }, "Close")),
  ),
  h(
    "div",
    { class: "panel-side" },
    h("div", { class: "panel-now" }, coverBox, h("div", { class: "words" }, title, artist)),
    h("div", { class: "panel-controls" }, transport(), iconButton("stop", "Stop and close the player", stopPlayer)),
  ),
  h("div", { class: "panel-tabs", role: "tablist", "aria-label": "Queue, lyrics and listening together" }, tabs.queue, tabs.lyrics, tabs.together),
  bodies.queue,
  bodies.lyrics,
  bodies.together,
);

let tab = "queue";
let showAllPlayed = false;
let returnTo = null;
let full = false;
const look = nowLook(coverBox, cover, panel);

function said() {
  document.dispatchEvent(new CustomEvent("melete:panel", { detail: { open: panel.open, tab } }));
}

function choose(next) {
  tab = next;
  for (const [key, button] of Object.entries(tabs)) {
    button.setAttribute("aria-selected", String(key === tab));
    button.tabIndex = key === tab ? 0 : -1;
    bodies[key].hidden = key !== tab;
  }
  if (tab === "lyrics") lyrics.show(queue.current());
  said();
}

function openIn(mode) {
  document.body.classList.toggle("with-panel", mode === "beside");
  if (mode === "beside") panel.show();
  else panel.showModal();
}

// Beside the page on a wide screen, over it otherwise or in full screen.
const mode = () => (!full && wide.matches ? "beside" : "over");

// Full screen on or off: the panel opens again in its new place, focus staying on the switch
// (unless the panel is closing).
function setFull(on, keepFocus = true) {
  full = on;
  panel.classList.toggle("full", on);
  fullButton.replaceChildren(icon(on ? "exit-fullscreen" : "fullscreen"));
  fullButton.setAttribute("aria-pressed", String(on));
  if (panel.open) {
    panel.close();
    openIn(mode());
  }
  if (keepFocus) fullButton.focus();
}

export function openPanel(next = tab) {
  if (!panel.isConnected) document.body.append(panel);
  choose(next);
  draw();
  if (!panel.open) {
    returnTo = document.activeElement;
    openIn(mode());
  }
  tabs[tab].focus();
  said();
}

export function closePanel() {
  if (!panel.open) return;
  panel.close();
}

export function togglePanel(next) {
  if (panel.open && tab === next) closePanel();
  else openPanel(next);
}

panel.addEventListener("close", () => {
  // Closed only to open again elsewhere (full screen, a wider window): nothing more to do.
  if (panel.open) return;
  look.pause();
  if (full) setFull(false, false);
  document.body.classList.remove("with-panel");
  if (returnTo?.isConnected && (panel.contains(document.activeElement) || document.activeElement === document.body)) returnTo.focus();
  said();
});
// Escape closes the panel beside the page too, unless something opened since (the palette, a
// page's menu) is what it's meant for.
// In full screen, Escape leaves full screen rather than closing the panel.
panel.addEventListener("cancel", (event) => {
  if (!full) return;
  event.preventDefault();
  setFull(false);
});
document.addEventListener("keydown", (event) => {
  const other = document.querySelector("dialog:modal, #fold-menu[open]");
  if (event.key === "Escape" && panel.open && !panel.matches(":modal") && !other) closePanel();
});
// Arrow keys, Home and End move between the tabs.
panel.querySelector('[role="tablist"]').addEventListener("keydown", (event) => {
  const order = ["queue", "lyrics", "together"];
  const moves = { ArrowRight: 1, ArrowLeft: -1, Home: -9, End: 9 };
  if (!(event.key in moves)) return;
  event.preventDefault();
  const at = Math.min(Math.max(order.indexOf(tab) + moves[event.key], 0), order.length - 1);
  choose(order[at]);
  tabs[tab].focus();
});
for (const [key, button] of Object.entries(tabs)) button.addEventListener("click", () => choose(key));
// Crossing 1100 px while open moves the panel beside the page or over it.
wide.addEventListener("change", () => {
  if (!panel.open || full) return;
  panel.close();
  openIn(mode());
});

// ---------- Queue ----------

const PARTS = [
  ["played", "Played"],
  ["now", "Now playing"],
  ["next", "Next up"],
  ["rest", "Then"],
];

function row(entry, at, part) {
  const t = entry.track;
  const words = `${t.title}${t.artist ? ` by ${t.artist}` : ""}`;
  const can = (by) => at + by >= 0 && at + by < queue.all().length && queue.partOf(at + by) === part;
  return h(
    "li",
    { class: `queue-row ${part}`, "data-key": entry.key, "aria-current": part === "now" ? "true" : null },
    h("div", { class: "cover" }, t.cover ? h("img", { alt: "", src: t.cover, loading: "lazy" }) : null),
    h(
      "button",
      { type: "button", class: "queue-play", "aria-label": part === "now" ? `${words}, playing` : `Play ${words}`, onclick: () => jumpTo(at) },
      clip(t.title),
      clip(t.artist ?? "", "small"),
    ),
    h("span", { class: "length" }, t.duration_seconds ? clock(t.duration_seconds) : ""),
    part === "now"
      ? null
      : h(
          "div",
          { class: "row-tools" },
          part === "played" ? null : iconButton("up", `Move ${t.title} up`, () => act(entry.key, "up", () => queue.move(at, -1)), can(-1) ? "" : "unavailable"),
          part === "played" ? null : iconButton("down", `Move ${t.title} down`, () => act(entry.key, "down", () => queue.move(at, 1)), can(1) ? "" : "unavailable"),
          iconButton("close", `Remove ${t.title} from the queue`, () => act(entry.key, "remove", () => queue.remove(at))),
        ),
  );
}

// Does a row's action, then puts focus back on the same button of the same row (or the row
// that took its place), so the keyboard doesn't lose its place.
function act(key, what, change) {
  const rows = [...queueBody.querySelectorAll("[data-key]")];
  const was = rows.findIndex((r) => r.dataset.key === String(key));
  if (!change()) return;
  const again = queueBody.querySelector(`[data-key="${key}"]`) ?? queueBody.querySelectorAll("[data-key]")[Math.min(was, rows.length - 2)];
  const buttons = { up: 'button[aria-label^="Move"][aria-label$=" up"]', down: 'button[aria-label$=" down"]', remove: ".queue-play" };
  (again?.querySelector(buttons[what]) ?? again?.querySelector(".queue-play"))?.focus();
}

function draw() {
  const track = queue.current();
  look.show(track);
  title.textContent = track?.title ?? "Nothing is playing";
  artist.textContent = track?.artist ?? "";
  const entries = queue.all();
  if (!track || !entries.length) {
    return queueBody.replaceChildren(emptyState("Nothing queued. Play an album or a song to start.", { href: "#/music", label: "Go to Listen" }));
  }
  const groups = PARTS.map(([part, name]) => {
    let rows = entries.map((e, i) => [e, i]).filter(([, i]) => queue.partOf(i) === part);
    const hidden = part === "played" && !showAllPlayed ? Math.max(0, rows.length - PLAYED_SHOWN) : 0;
    rows = rows.slice(hidden);
    if (!rows.length) return null;
    return h(
      "section",
      { class: `queue-part ${part}`, "aria-labelledby": `queue-${part}` },
      h("h3", { id: `queue-${part}` }, name),
      hidden
        ? h("button", { type: "button", class: "link", onclick: () => ((showAllPlayed = true), draw()) }, `Show the ${hidden} played before`)
        : null,
      h("ol", {}, rows.map(([e, i]) => row(e, i, part))),
    );
  });
  const after = entries.length - queue.position() - 1;
  const clear = after ? h("button", { type: "button", class: "quiet small", onclick: () => queue.clearAfter() }, "Clear the rest") : null;
  queueBody.replaceChildren(...[...groups, clear].filter(Boolean));
}

document.addEventListener("melete:queue", () => panel.open && draw());
document.addEventListener("melete:track", (event) => {
  if (!event.detail.track) return closePanel();
  if (panel.open) draw();
  if (panel.open && tab === "lyrics") lyrics.show(event.detail.track);
});
audio.addEventListener("timeupdate", () => panel.open && tab === "lyrics" && lyrics.follow());
