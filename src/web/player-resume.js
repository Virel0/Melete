// Picking up where it was left: a few seconds before, as this browser asks (Your account →
// Playback), so a sentence cut off isn't missed; and "Picking up at 12:34" under the video,
// with a button to start from the beginning instead.

import { clock, h } from "/web/lib.js";
import { resumeEarlier } from "/web/playback-prefs.js";

// Where to start: the beginning when asked (start=0), else where the server says it was left,
// less the seconds this browser asks for, never before the start.
export function startAt(play, restart) {
  if (restart || !(play.resume_seconds > 0)) return 0;
  return Math.max(play.resume_seconds - resumeEarlier(), 0);
}

// "Picking up at 12:34", with a button to start from the beginning instead.
export function resumedNote(start, fileId, params) {
  return start > 0
    ? h(
        "p",
        { class: "resumed" },
        `Picking up at ${clock(start)}. `,
        h(
          "button",
          {
            type: "button",
            class: "link",
            // Starting over is its own address, so the server is told with every report.
            onclick: () => {
              const again = new URLSearchParams(params);
              again.set("start", "0");
              location.replace(`#/play/${fileId}?${again}`);
            },
          },
          "Start from the beginning",
        ),
      )
    : null;
}
