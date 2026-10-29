// The seek bar under the video: a tick where each of the file's chapters starts and the credits
// marked on its track; pointing at it shows the time and chapter there, and the picture from
// the sheets the server made (`GET /files/{id}/trickplay`) for a file that has them. Using it
// seeks there. Shown for a file with previews, chapters or credits.

import { api, clock, h } from "/web/lib.js";

// The widest a preview is shown, in pixels; narrower on a narrow screen.
const SHOWN_WIDTH = 240;

// The bar for `video`; sheets come from `streamBase`, the stream link. `chapters` are the
// file's, `credits` its credits part ({ start_seconds, end_seconds }) or null. Null when
// there's nothing to show on it.
export function previewBar(video, fileId, streamBase, { trickplay, chapters = [], credits = null }) {
  if (!trickplay && chapters.length < 2 && !credits) return null;
  const picture = h("div", { class: "scrub-picture", hidden: true });
  const time = h("span", { class: "scrub-time" });
  const tip = h("div", { class: "scrub-preview", hidden: true, "aria-hidden": "true" }, picture, time);
  const bar = h("input", { type: "range", class: "scrub", min: "0", max: "1", step: "1", value: "0", "aria-label": "Seek" });
  const marks = h("div", { class: "scrub-marks", "aria-hidden": "true" });
  const wrap = h("div", { class: "scrub-wrap" }, tip, h("div", { class: "scrub-track" }, marks, bar));
  const p = { video, streamBase, index: null, wrap, bar, tip, picture, time, marks, chapters, credits };
  attach(p);
  if (trickplay) {
    api(`/files/${fileId}/trickplay`)
      .then((index) => (p.index = index))
      .catch(() => {});
  }
  return wrap;
}

// The last of `times` at or before `t`.
function lastAt(times, t) {
  let low = 0;
  let high = times.length - 1;
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (times[mid] <= t) low = mid;
    else high = mid - 1;
  }
  return low;
}

// The chapter playing at `t`, if the file has chapters.
function chapterAt(chapters, t) {
  if (chapters.length < 2) return null;
  const c = chapters[lastAt(chapters.map((x) => x.start_seconds), t)];
  return c?.title ?? `Chapter ${chapters.indexOf(c) + 1}`;
}

// Ticks at the chapters' starts (not the first, at nothing), and the credits as a band.
function drawMarks(p, length) {
  const at = (t) => `${Math.min(100, (100 * t) / length)}%`;
  const ticks = p.chapters.slice(1).map((c) => {
    const tick = h("span", { class: "scrub-tick" });
    tick.style.setProperty("left", at(c.start_seconds));
    return tick;
  });
  const band = p.credits ? h("span", { class: "scrub-credits" }) : null;
  if (band) {
    band.style.setProperty("left", at(p.credits.start_seconds));
    band.style.setProperty("width", `calc(${at(p.credits.end_seconds)} - ${at(p.credits.start_seconds)})`);
  }
  p.marks.replaceChildren(...ticks, ...(band ? [band] : []));
}

function attach(p) {
  const { video, bar, tip } = p;
  const length = () => (Number.isFinite(video.duration) && video.duration > 0 ? video.duration : null);
  let dragging = false;
  let drawn = null;
  const sync = () => {
    const end = length();
    if (!end) return;
    if (drawn !== end) {
      drawn = end;
      bar.max = String(Math.floor(end));
      drawMarks(p, end);
    }
    if (!dragging) bar.value = String(Math.floor(video.currentTime));
    const chapter = chapterAt(p.chapters, video.currentTime);
    bar.setAttribute("aria-valuetext", `${clock(video.currentTime)} of ${clock(end)}${chapter ? `, ${chapter}` : ""}`);
  };
  video.addEventListener("loadedmetadata", sync);
  video.addEventListener("timeupdate", sync);
  const pointedAt = (event) => {
    const box = bar.getBoundingClientRect();
    const share = Math.min(Math.max((event.clientX - box.left) / box.width, 0), 1);
    return { t: share * Number(bar.max), x: event.clientX - box.left };
  };
  bar.addEventListener("pointermove", (event) => {
    if (!length()) return;
    const { t, x } = pointedAt(event);
    show(p, t, x);
  });
  bar.addEventListener("pointerdown", () => (dragging = true));
  bar.addEventListener("pointerup", () => (dragging = false));
  bar.addEventListener("pointerleave", () => {
    if (document.activeElement !== bar) tip.hidden = true;
  });
  // Keys move the bar: show where it's at, and go there.
  bar.addEventListener("input", () => {
    const t = Number(bar.value);
    show(p, t, (t / Number(bar.max)) * bar.clientWidth);
    video.currentTime = t;
  });
  bar.addEventListener("blur", () => (tip.hidden = true));
}

// Shows the time, chapter and (with previews) the picture for time `t` above the bar, `x`
// pixels along it.
function show(p, t, x) {
  const { index, streamBase, wrap, tip, picture, time } = p;
  let width = 0;
  picture.hidden = !index;
  if (index) {
    const i = lastAt(index.times, t);
    const perSheet = index.columns * index.rows;
    const sheet = Math.floor(i / perSheet);
    const column = i % index.columns;
    const row = Math.floor(i / index.columns) % index.rows;
    width = Math.min(SHOWN_WIDTH, index.width, wrap.clientWidth * 0.45);
    const scale = width / index.width;
    Object.assign(picture.style, {
      width: `${width}px`,
      height: `${index.height * scale}px`,
      backgroundImage: `url("${streamBase}/trickplay/${sheet}.jpg")`,
      backgroundSize: `${index.columns * width}px ${index.rows * index.height * scale}px`,
      backgroundPosition: `${-column * width}px ${-row * index.height * scale}px`,
    });
  }
  const chapter = chapterAt(p.chapters, t);
  time.textContent = chapter ? `${clock(t)} · ${chapter}` : clock(t);
  tip.hidden = false;
  const half = Math.max(width, tip.offsetWidth) / 2 + 4;
  tip.style.left = `${Math.min(Math.max(x, half), wrap.clientWidth - half)}px`;
}
