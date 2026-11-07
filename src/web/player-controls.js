// The player's own controls over the picture, in full screen too: the seek bar, then play,
// back and forward ten seconds, the sound, the time, and the panels (Subtitles, Audio,
// Settings), picture in picture and full screen. They fade out after a few seconds of nothing
// while the video plays, and come back on any movement, touch or key; they stay while paused,
// while a panel is open or while focus is on them. A click on the picture plays or pauses; on a
// touch screen the first tap only brings them back. Waiting for the video shows a spinner, and
// each jump back or forward shows how far it went (player-jump.js).

import { clock, h } from "/web/lib.js";
import { icon } from "/web/music-controls.js";
import { jump, jumpFlash } from "/web/player-jump.js";

// Seconds the back and forward buttons move.
const STEP = 10;
// How long the controls stay with nothing happening, while the video plays.
const HIDE_AFTER_MS = 3000;

// The controls for `video` on `stage`. `seek` is the seek bar, `panels` what panelButtons gave,
// `fullscreen()` goes in or out. Returns { el, stop }: the overlay for the stage, and what
// takes its page-wide listeners away on leaving.
export function controlBar(video, stage, { seek, panels, fullscreen }) {
  const control = (name, label, onclick, extra = "") =>
    h("button", { type: "button", class: `icon control ${extra}`.trim(), "aria-label": label, title: label, onclick }, icon(name));
  const play = control("play", "Play", () => toggle(video));
  // Not on a phone's narrow screen, where the row has room for the rest only.
  const back = control("back", `Back ${STEP} seconds`, () => jump(video, -STEP), "control-wide");
  const ahead = control("forward", `Forward ${STEP} seconds`, () => jump(video, STEP), "control-wide");
  const mute = control("volume", "Mute", () => (video.muted = !video.muted));
  const volume = h("input", { type: "range", class: "volume", min: "0", max: "1", step: "0.05", value: String(video.volume), "aria-label": "Volume" });
  volume.addEventListener("input", () => {
    video.volume = Number(volume.value);
    video.muted = video.volume === 0;
  });
  const time = h("span", { class: "control-time" });
  const pip = document.pictureInPictureEnabled ? control("pip", "Picture in picture", () => pictureInPicture(video), "control-wide") : null;
  const screen = control("fullscreen", "Full screen", () => fullscreen()?.catch?.(() => {}));
  const big = h("button", { type: "button", class: "stage-play", "aria-label": "Play", onclick: () => toggle(video) }, icon("play"));
  const spinner = h("div", { class: "stage-waiting", role: "status", "aria-label": "Loading" });
  const flash = jumpFlash(video);
  const el = h(
    "div",
    { class: "player-chrome" },
    big,
    spinner,
    flash.el,
    h(
      "div",
      { class: "control-bar" },
      panels.host,
      seek,
      h("div", { class: "control-row" }, play, back, ahead, mute, volume, time, h("span", { class: "control-gap" }), ...panels.buttons, pip, screen),
    ),
  );

  const drawPlay = () => {
    const playing = !video.paused;
    play.replaceChildren(icon(playing ? "pause" : "play"));
    play.setAttribute("aria-label", playing ? "Pause" : "Play");
    play.title = playing ? "Pause" : "Play";
    big.hidden = playing;
    stage.classList.toggle("paused", !playing);
  };
  const drawSound = () => {
    const silent = video.muted || video.volume === 0;
    mute.replaceChildren(icon(silent ? "muted" : "volume"));
    mute.setAttribute("aria-label", silent ? "Unmute" : "Mute");
    mute.title = silent ? "Unmute" : "Mute";
    volume.value = String(video.muted ? 0 : video.volume);
  };
  const drawTime = () => {
    const end = Number.isFinite(video.duration) ? video.duration : null;
    time.textContent = end ? `${clock(video.currentTime)} / ${clock(end)}` : clock(video.currentTime);
  };
  const drawScreen = () => {
    const full = document.fullscreenElement === stage;
    screen.replaceChildren(icon(full ? "exit-fullscreen" : "fullscreen"));
    screen.setAttribute("aria-label", full ? "Exit full screen" : "Full screen");
    screen.title = full ? "Exit full screen" : "Full screen";
  };
  for (const event of ["play", "pause", "ended", "emptied"]) video.addEventListener(event, () => (drawPlay(), wake()));
  video.addEventListener("volumechange", drawSound);
  for (const event of ["timeupdate", "durationchange", "loadedmetadata"]) video.addEventListener(event, drawTime);
  video.addEventListener("waiting", () => stage.classList.add("waiting"));
  for (const event of ["playing", "canplay", "pause", "error"]) video.addEventListener(event, () => stage.classList.remove("waiting"));
  document.addEventListener("fullscreenchange", drawScreen);

  // Out of the way while it plays and nothing happens; back on anything.
  let timer = null;
  const hide = () => {
    // Keyboard focus on a control keeps them; a mouse click leaves focus there but shouldn't.
    const busy = video.paused || panels.isOpen() || Boolean(el.querySelector(":focus-visible"));
    stage.classList.toggle("idle", !busy);
  };
  const wake = () => {
    stage.classList.remove("idle");
    clearTimeout(timer);
    timer = setTimeout(hide, HIDE_AFTER_MS);
  };
  stage.addEventListener("pointermove", wake);
  stage.addEventListener("keydown", wake);
  el.addEventListener("focusin", wake);
  document.addEventListener("keydown", wake);
  // A click on the picture plays or pauses; a tap on a touch screen first brings the controls.
  video.addEventListener("pointerup", (event) => {
    if (event.pointerType !== "mouse" && stage.classList.contains("idle")) return wake();
    if (event.button === 0) toggle(video);
  });

  drawPlay();
  drawSound();
  drawTime();
  drawScreen();
  wake();
  const stop = () => {
    clearTimeout(timer);
    flash.stop();
    document.removeEventListener("fullscreenchange", drawScreen);
    document.removeEventListener("keydown", wake);
  };
  return { el, stop, wake };
}

function toggle(video) {
  if (video.paused) video.play().catch(() => {});
  else video.pause();
}

function pictureInPicture(video) {
  const going = document.pictureInPictureElement ? document.exitPictureInPicture() : video.requestPictureInPicture();
  going.catch(() => {});
}
