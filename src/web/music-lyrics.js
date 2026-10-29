// The Lyrics tab of the queue and lyrics panel: the playing track's lyrics from
// `GET /music/tracks/{id}/lyrics`. Synced lines follow playback, the current one lit and kept in
// view, and each line is a button that seeks there; a blank line shows ♪. Unsynced lyrics are
// plain text. Translate (into the browser's language) and Pronunciation are offered when the
// server has them set up: they're asked for only when pressed, since a translation sends the
// song's lines to the translator the household set up, and a server without one says so and
// the button goes. States: nothing playing, loading, no lyrics, failed with Try again.
//
// lyricsTab(audio, seek) makes the tab's body; show(track) draws it for a track (null when
// nothing plays), and follow() lights the line for the audio's time.

import { SignedOut, api, h } from "/web/lib.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";

const SOURCES = { file: "a lyrics file beside the song", tags: "the song's own tags", lrclib: "LRCLIB" };

// The browser's language as the translator names it: "en", "pt-BR", "zh-Hans".
function language() {
  const [main, region] = (navigator.language || "en").split("-");
  if (main === "zh") return ["TW", "HK", "MO", "Hant"].includes(region) ? "zh-Hant" : "zh-Hans";
  if (main === "pt" && region === "BR") return "pt-BR";
  return main;
}

// What this server turned out not to offer, for the rest of the session.
const missing = { translate: false, pronounce: false };

// One line: a button that seeks there when the lyrics are synced, else plain text. A blank
// line shows a note; the translation and pronunciation sit under the words when asked for.
// A blank line: a note for the eye, words for a screen reader.
function pause() {
  return [h("span", { "aria-hidden": "true" }, "♪"), h("span", { class: "visually-hidden" }, "A pause")]; // check-ui: ignore a music note marks an instrumental pause, as lyrics apps show it; not an icon
}

function lyricLine(line, i, synced, seek) {
  const words = [
    h("span", { class: "words" }, line.text || pause()),
    line.translation && line.translation !== line.text ? h("span", { class: "extra" }, line.translation) : null,
    line.pronunciation ? h("span", { class: "extra" }, line.pronunciation) : null,
  ];
  if (!synced) return h("p", { class: "line" }, words);
  return h("button", { type: "button", class: "line", "data-line": i, onclick: () => seek(line.start_seconds) }, words);
}

// The line sung at `now` seconds: the last one started (a moment early, so it lights as it's
// sung), or -1 before the first.
function lineAt(lines, now) {
  let at = -1;
  for (let i = 0; i < lines.length && lines[i].start_seconds <= now + 0.2; i++) at = i;
  return at;
}

// Marks line `at` as the one sung, and scrolls it into the middle of the panel when `scroll`.
function light(box, at, scroll) {
  const before = box.querySelector(".line.now");
  before?.classList.remove("now");
  before?.removeAttribute("aria-current");
  const line = box.querySelector(`[data-line="${at}"]`);
  if (!line) return;
  line.classList.add("now");
  line.setAttribute("aria-current", "true");
  if (!scroll) return;
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  line.scrollIntoView({ block: "center", behavior: still ? "auto" : "smooth" });
}

export function lyricsTab(audio, seek) {
  const box = h("div", { class: "lyrics" });
  const said = h("p", { class: "note", role: "status" });
  const options = { translate: false, pronounce: false };
  const translate = h("button", { type: "button", class: "quiet small", "aria-pressed": "false" }, "Translate");
  const pronounce = h("button", { type: "button", class: "quiet small", "aria-pressed": "false" }, "Pronunciation");
  const tools = h("div", { class: "lyrics-tools" }, translate, pronounce);
  const foot = h("p", { class: "note lyrics-source" });
  const body = h("div", { class: "lyrics-body" }, tools, said, box, foot);
  let track = null;
  let lines = [];
  let asked = 0;
  let lit = -1;

  const draw = (answer) => {
    lit = -1;
    lines = answer.synced ? answer.lines : [];
    box.classList.toggle("synced", answer.synced);
    box.replaceChildren(...answer.lines.map((line, i) => lyricLine(line, i, answer.synced, seek)));
    foot.textContent = `Lyrics from ${SOURCES[answer.source] ?? answer.source}.`;
    follow();
  };

  const load = async () => {
    const mine = ++asked;
    tools.hidden = !track;
    foot.textContent = "";
    if (!track) return box.replaceChildren(emptyState("Nothing is playing."));
    box.replaceChildren(placeholders("the lyrics", { kind: "lines", count: 6 }));
    const query = new URLSearchParams();
    if (options.translate) query.set("translate", language());
    if (options.pronounce) query.set("pronounce", "true");
    try {
      const answer = await api(`/music/tracks/${track.id}/lyrics${query.size ? `?${query}` : ""}`);
      if (mine === asked) draw(answer);
    } catch (err) {
      if (mine !== asked || err instanceof SignedOut) return;
      if (err.status === 404) {
        tools.hidden = true;
        return box.replaceChildren(emptyState("No lyrics for this song."));
      }
      if (err.status === 409) return turnedOff(options.translate && !missing.translate ? "translate" : "pronounce");
      box.replaceChildren(failed("the lyrics", err, load));
    }
  };

  // The server doesn't offer this: say so, take the button away, and show the lyrics without it.
  const turnedOff = (what) => {
    missing[what] = true;
    options[what] = false;
    (what === "translate" ? translate : pronounce).hidden = true;
    said.textContent = what === "translate" ? "Translating lyrics isn't set up on this server." : "Pronunciation isn't turned on on this server.";
    load();
  };

  for (const [what, button] of [["translate", translate], ["pronounce", pronounce]]) {
    button.hidden = missing[what];
    button.addEventListener("click", () => {
      options[what] = !options[what];
      button.setAttribute("aria-pressed", String(options[what]));
      said.textContent = "";
      load();
    });
  }

  // Lights the line being sung, and keeps it in view while the tab shows.
  const follow = () => {
    const at = lineAt(lines, audio.currentTime);
    if (at === lit) return;
    lit = at;
    light(box, at, body.isConnected && !body.closest("[hidden]"));
  };

  const show = (next) => {
    if (next?.id === track?.id && track) return;
    track = next;
    said.textContent = "";
    load();
  };

  return { body, show, follow };
}
