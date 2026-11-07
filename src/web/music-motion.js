// An album's motion cover (`motion_cover`: a short looping video in its folder) over its still
// cover, on the album's page and in Now Playing. Always muted and looped, without controls or
// sound; shown only once it plays, so the still cover stays while it loads or if it can't play.
// Never for a device that asks for less motion: none starts then, and any playing stops and goes
// when the setting changes.
//
// motionCover(url) → a video to put in the cover's box (which is position: relative), or null.

import { h } from "/web/lib.js";

const stillness = matchMedia("(prefers-reduced-motion: reduce)");

export function motionCover(url) {
  if (!url || stillness.matches) return null;
  const video = h("video", {
    class: "motion-cover",
    src: url,
    muted: true,
    loop: true,
    playsinline: true,
    preload: "auto",
    disablepictureinpicture: true,
    disableremoteplayback: true,
    "aria-hidden": "true",
  });
  // The property, not only the attribute, is what keeps it silent.
  video.muted = true;
  video.addEventListener("playing", () => video.classList.add("shown"), { once: true });
  video.addEventListener("error", () => video.remove());
  video.play().catch(() => {});
  return video;
}

stillness.addEventListener("change", () => {
  if (!stillness.matches) return;
  for (const video of document.querySelectorAll("video.motion-cover")) {
    video.pause();
    video.remove();
  }
});
