// The tab's title says the song while music plays ("Title – Artist"), and goes back to the
// page's own title once it's paused or stopped. Pages set their titles as they always have; this
// notices each one and keeps it for when the music stops.

import { audio } from "/web/music-player.js";

const titleEl = document.querySelector("title");
let pageTitle = document.title;
let track = null;
// Between one track and the next the audio is paused for a moment; the song stays meanwhile.
let loading = false;
// The title this module last set, so its own change isn't taken for the page's.
let mine = null;

function apply() {
  const want = track && (loading || !audio.paused) ? [track.title, track.artist].filter(Boolean).join(" – ") : pageTitle;
  if (document.title === want) return;
  mine = want;
  document.title = want;
}

new MutationObserver(() => {
  if (document.title === mine) return;
  pageTitle = document.title;
  apply();
}).observe(titleEl, { childList: true, characterData: true, subtree: true });

document.addEventListener("melete:track", (event) => {
  track = event.detail.track;
  loading = event.detail.loading;
  apply();
});
audio.addEventListener("play", apply);
audio.addEventListener("pause", apply);
audio.addEventListener("emptied", apply);
