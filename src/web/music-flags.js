// Track flags, as the Aoide apps keep them in the listener's sync log (`/music/flags`): "Not
// interested" keeps a song out of mixes, radio, shuffle and smart playlists; "Don't count
// plays" keeps its new listens out of the history. Read once and kept here; a change made here
// is written through the server, and other views hear of it ("melete:flags").
//
// flagButtons(track, said) gives the two toggle buttons for a song's actions strip. Setting
// "Not interested" on the song playing now moves on to the next one, as the apps do.

import { SignedOut, api, h } from "/web/lib.js";
import { goNext } from "/web/music-player.js";
import * as queue from "/web/music-queue.js";
import { hold, release } from "/web/page-parts.js";
import { SYNC_CLOSED } from "/web/music-access.js";

const FLAGS = {
  not_interested: { label: "Not interested", on: (t) => `${t} is kept out of mixes, radio and shuffle.`, off: (t) => `${t} can come up in mixes again.` },
  dont_count: { label: "Don't count plays", on: (t) => `Plays of ${t} aren't counted from now on.`, off: (t) => `Plays of ${t} count again.` },
};

// What a change to `key` says, for a song titled `title`.
export function saidOf(key, on, title) {
  return (on ? FLAGS[key].on : FLAGS[key].off)(title);
}

// Track id → { not_interested, dont_count }, once read.
let known = null;
let reading = null;

export async function flagsOf(trackId) {
  if (!known) {
    reading ??= api("/music/flags").then((answer) => {
      known = new Map(answer.items.filter((i) => i.track).map((i) => [i.track.id, { not_interested: i.not_interested, dont_count: i.dont_count }]));
    });
    try {
      await reading;
    } catch (err) {
      reading = null;
      throw err;
    }
  }
  return known.get(trackId) ?? { not_interested: false, dont_count: false };
}

// Sets one flag; says what it changed for other views.
export async function setFlag(track, key, on) {
  const now = await api(`/music/tracks/${track.id}/flags`, { method: "PUT", body: { [key]: on } });
  known?.set(track.id, now);
  document.dispatchEvent(new CustomEvent("melete:flags", { detail: { id: track.id, flags: now } }));
  if (key === "not_interested" && on && queue.current()?.id === track.id) goNext();
  return now;
}

// The two toggles for a song's strip, pressed while set; disabled until the flags are read.
export function flagButtons(track, said) {
  const buttons = Object.entries(FLAGS).map(([key, words]) => {
    const button = h("button", { type: "button", class: "quiet small", "aria-pressed": "false", disabled: true }, words.label);
    button.addEventListener("click", async () => {
      if (!hold(button)) return;
      said.textContent = "";
      const want = button.getAttribute("aria-pressed") !== "true";
      try {
        const now = await setFlag(track, key, want);
        button.setAttribute("aria-pressed", String(now[key]));
        said.textContent = (now[key] ? words.on : words.off)(track.title);
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        said.textContent = err.status === 403 ? SYNC_CLOSED : `Couldn't change it: ${err.message}`;
      } finally {
        release(button);
      }
    });
    return [key, button];
  });
  flagsOf(track.id)
    .then((flags) => {
      for (const [key, button] of buttons) {
        button.setAttribute("aria-pressed", String(Boolean(flags[key])));
        button.disabled = false;
      }
    })
    .catch((err) => {
      if (err instanceof SignedOut) return;
      for (const [, button] of buttons) button.title = "Flags couldn't be read";
    });
  return buttons.map(([, button]) => button);
}
