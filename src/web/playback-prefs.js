// Your playback preferences, which every app follows: whether the next episode starts by itself,
// how long the countdown before it lasts, and where the Watch home's hero plays trailers. And one
// this browser keeps: how far before where you stopped the player picks up. A part of Your
// account.

import { field, form, group } from "/web/forms.js";
import { api, h } from "/web/lib.js";

const MIN = 5;
const MAX = 30;

const HERO_TRAILERS = [
  ["large_screens", "On screens larger than a phone"],
  ["everywhere", "On every screen"],
  ["off", "Never"],
];

// How many seconds before where you stopped the player picks up, kept by this browser.
const RESUME_KEY = "melete.resume-earlier";
const RESUME_EARLIER = [
  [0, "Where I stopped"],
  [5, "5 seconds earlier"],
  [10, "10 seconds earlier"],
  [15, "15 seconds earlier"],
  [30, "30 seconds earlier"],
];

// The seconds this browser keeps; none when it keeps nothing, or storage is turned off.
export function resumeEarlier() {
  try {
    const kept = Number(localStorage.getItem(RESUME_KEY));
    return RESUME_EARLIER.some(([seconds]) => seconds === kept) ? kept : 0;
  } catch {
    return 0;
  }
}

// Keeps `seconds` in this browser; false when its storage is turned off.
function keepResumeEarlier(seconds) {
  try {
    localStorage.setItem(RESUME_KEY, String(seconds));
    return true;
  } catch {
    return false;
  }
}

// A checkbox shaped like a form field, so `form` can clear it with the rest.
function checkField(id, label, checked) {
  const input = h("input", { type: "checkbox", id, checked });
  return { box: h("div", { class: "check" }, input, h("label", { for: id }, label)), input, clear() {} };
}

export async function playbackGroup() {
  const saved = await api("/me/playback");
  const autoplay = checkField("autoplay-next", "Start the next episode by itself", saved.autoplay_next);
  const countdown = field(
    "countdown-seconds",
    "Countdown before the next episode (seconds)",
    h("input", { type: "number", inputmode: "numeric", min: MIN, max: MAX, step: 1, value: saved.countdown_seconds }),
    `From ${MIN} to ${MAX}. When the next episode doesn't start by itself, the player offers it without counting down.`,
  );
  const trailers = field(
    "hero-trailers",
    "Play previews on the Watch home",
    h(
      "select",
      {},
      HERO_TRAILERS.map(([value, label]) => h("option", { value, selected: value === saved.hero_trailers }, label)),
    ),
    "A pick's trailer when it has one in its folder, else a clip of the title, muted, after a few seconds on it, with buttons to unmute and stop. Never when your device asks for less motion.",
  );
  const earlier = resumeEarlier();
  const resume = field(
    "resume-earlier",
    "Pick up",
    h(
      "select",
      {},
      RESUME_EARLIER.map(([seconds, label]) => h("option", { value: seconds, selected: seconds === earlier }, label)),
    ),
    "Where a video you stopped starts again, so a sentence cut off isn't missed. Kept by this browser only.",
  );
  const saving = form([autoplay, countdown, trailers, resume], "Save playback", async () => {
    const seconds = Number(countdown.input.value);
    if (!Number.isInteger(seconds) || seconds < MIN || seconds > MAX) {
      countdown.fail(`Enter a whole number of seconds from ${MIN} to ${MAX}.`);
      return null;
    }
    const kept = await api("/me/playback", {
      method: "PUT",
      body: { autoplay_next: autoplay.input.checked, countdown_seconds: seconds, hero_trailers: trailers.input.value },
    });
    countdown.input.value = kept.countdown_seconds;
    const said = kept.autoplay_next
      ? `Saved. The next episode starts after ${kept.countdown_seconds} seconds.`
      : "Saved. The next episode waits for you.";
    return keepResumeEarlier(Number(resume.input.value)) ? said : `${said} This browser can't keep where to pick up: its storage is turned off.`;
  });
  return group("Playback", h("p", { class: "note" }, "Every app follows these, apart from what's kept by this browser only."), saving);
}
