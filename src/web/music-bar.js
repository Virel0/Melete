// The music bar at the foot of every page while something is queued; the page leaves room for
// it (`with-player` on the body, and --player-height for what sits above it). Now playing on the
// left (cover, title linked to its album, artist, and what the player is doing), the controls
// in the middle (music-controls.js), and Lyrics, Queue, volume and Stop on the right. On a
// phone it shrinks to the cover, the title and play; the cover then opens the panel, which
// carries the rest of the controls there.
//
// startBar() puts it on the page once signed in; it shows and hides itself as tracks start and
// the player stops.

import { h } from "/web/lib.js";
import { audio, stopPlayer, toggle } from "/web/music-player.js";
import { icon, iconButton, transport } from "/web/music-controls.js";
import { openPanel, togglePanel } from "/web/music-panel.js";

const cover = h("img", { alt: "" });
// The cover opens the panel: on a phone, it's the way to the queue, lyrics and other controls.
const coverButton = h(
  "button",
  { type: "button", class: "cover", "aria-label": "Open the queue and lyrics", title: "Queue and lyrics", onclick: () => openPanel() },
  cover,
);
const titleLink = h("a", { class: "track-title clip" });
const artistLine = h("span", { class: "track-artist clip" });
const status = h("span", { class: "track-status", role: "status" });
const phonePlay = iconButton("play", "Play", toggle, "play phone-only");
const lyricsButton = iconButton("lyrics", "Lyrics", () => togglePanel("lyrics"));
const queueButton = iconButton("queue", "Queue", () => togglePanel("queue"));
const mute = iconButton("volume", "Mute", () => (audio.muted = !audio.muted));
const volume = h("input", { type: "range", min: "0", max: "1", step: "0.05", value: "1", "aria-label": "Volume" });
const stop = iconButton("stop", "Stop and close the player", stopPlayer);

const bar = h(
  "section",
  { class: "music-bar", "aria-label": "Music player", hidden: true },
  h("div", { class: "now" }, coverButton, h("div", { class: "words" }, titleLink, artistLine, status), phonePlay),
  transport(),
  h("div", { class: "extras" }, lyricsButton, queueButton, h("div", { class: "volume" }, mute, volume), stop),
);

export function startBar() {
  if (!bar.isConnected) document.body.append(bar);
}

function show(track) {
  bar.hidden = !track;
  document.body.classList.toggle("with-player", Boolean(track));
  if (!track) return;
  titleLink.textContent = track.title;
  titleLink.title = track.title;
  titleLink.href = track.album_id ? `#/music/albums/${track.album_id}` : "#/music";
  artistLine.textContent = [track.artist, track.album].filter(Boolean).join(", ");
  artistLine.title = artistLine.textContent;
  if (track.cover) cover.src = track.cover;
  else cover.removeAttribute("src");
  coverButton.classList.toggle("blank", !track.cover);
}

document.addEventListener("melete:track", (event) => show(event.detail.track));
document.addEventListener("melete:status", (event) => (status.textContent = event.detail));
document.addEventListener("melete:panel", (event) => {
  const { open, tab } = event.detail;
  lyricsButton.setAttribute("aria-pressed", String(open && tab === "lyrics"));
  queueButton.setAttribute("aria-pressed", String(open && tab === "queue"));
});
lyricsButton.setAttribute("aria-pressed", "false");
queueButton.setAttribute("aria-pressed", "false");

const playing = (on) => {
  phonePlay.replaceChildren(icon(on ? "pause" : "play"));
  phonePlay.setAttribute("aria-label", on ? "Pause" : "Play");
};
audio.addEventListener("play", () => playing(true));
audio.addEventListener("pause", () => playing(false));
audio.addEventListener("emptied", () => playing(false));

volume.addEventListener("input", () => {
  audio.volume = Number(volume.value);
  audio.muted = false;
});
audio.addEventListener("volumechange", () => {
  const silent = audio.muted || audio.volume === 0;
  mute.replaceChildren(icon(silent ? "muted" : "volume"));
  mute.setAttribute("aria-label", silent ? "Unmute" : "Mute");
  mute.title = silent ? "Unmute" : "Mute";
  volume.value = String(audio.muted ? 0 : audio.volume);
});

// The bar's height, for what sits above it (the panel, the select bar).
new ResizeObserver(() => {
  document.documentElement.style.setProperty("--player-height", `${bar.hidden ? 0 : bar.offsetHeight}px`);
}).observe(bar);
