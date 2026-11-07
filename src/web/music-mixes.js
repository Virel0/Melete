// Mixes: the two the Aoide apps make for everyone, Rediscover Mix and Heavy Rotation, worked out
// by the server from the apps' own rules and the listener's listens (`POST /smart-playlists/songs`),
// each to play or shuffle; and, only when an admin has set AI up, a mix from a mood
// (`POST /ai/music/mix`), whose rules can be tried and then kept as a smart playlist. The mood
// alone goes to the AI service, as that route says; the songs are picked here, from the rules.

import { SignedOut, api, h, plural, show } from "/web/lib.js";
import { seesAllMusic, SYNC_CLOSED } from "/web/music-access.js";
import { songRow } from "/web/music-parts.js";
import { trackActions } from "/web/music-actions.js";
import { looseQueue, play, shufflePlay } from "/web/music-play.js";
import { playlistTrouble, playlistsChanged } from "/web/music-playlist-pick.js";
import { playlistHref } from "/web/music-playlists.js";
import { emptyState, hold, placeholders, release } from "/web/page-parts.js";

// The apps' built-in smart playlists, with their rules exactly as every client makes them.
export const BUILT_IN = [
  {
    key: "rediscover",
    name: "Rediscover Mix",
    notes: "Songs you played a lot and haven't heard in six months. Changes as you listen.",
    rules: {
      match: "all",
      rules: [
        { field: "last_played", op: "notInTheLast", value: "-6m" },
        { field: "play_count", op: "greaterThan", value: 2 },
      ],
      sort: { field: "play_count", dir: "desc" },
      limit: 40,
    },
  },
  {
    key: "heavy-rotation",
    name: "Heavy Rotation",
    notes: "What you've played most this month.",
    rules: {
      match: "all",
      rules: [
        { field: "last_played", op: "inTheLast", value: "-1m" },
        { field: "play_count", op: "greaterThan", value: 1 },
      ],
      sort: { field: "play_count", dir: "desc" },
      limit: 25,
    },
  },
];

// What rules give the listener now, on their calendar.
const worked = (rules) => api("/smart-playlists/songs", { method: "POST", body: { rules, offset_minutes: -new Date().getTimezoneOffset() } });

// A mix's songs with Play and Shuffle, or a sentence when none fit.
function songsOf(found, empty) {
  if (!found.tracks.length) return [h("p", { class: "status" }, empty)];
  const queue = looseQueue(found.tracks);
  return [
    h(
      "div",
      { class: "play-actions" },
      h("button", { type: "button", onclick: () => play(queue) }, "Play"),
      h("button", { type: "button", class: "quiet", onclick: () => shufflePlay(queue) }, "Shuffle"),
      h("span", { class: "note" }, plural(found.count, "song", "songs")),
    ),
    h("ul", { class: "tracks songs" }, found.tracks.map((t, i) => songRow(t, () => play(queue, i), trackActions(t, queue[i])))),
  ];
}

async function builtIn(mix) {
  const id = `mix-${mix.key}`;
  const box = h("section", { class: "listen-row mix", "aria-labelledby": id }, h("h2", { id }, mix.name), h("p", { class: "note" }, mix.notes));
  const body = h("div", {}, placeholders(mix.name, { kind: "rows", count: 3 }));
  box.append(body);
  worked(mix.rules)
    .then((found) => body.replaceChildren(...songsOf(found, "Nothing yet: it fills as you listen.")))
    .catch((err) => {
      if (err instanceof SignedOut) throw err;
      body.replaceChildren(h("p", { class: "error" }, err.message));
    });
  return box;
}

// A mix from a mood: the words, what the AI made of them, the songs that gives, and Keep it.
function fromMood() {
  const said = h("p", { class: "note", role: "status" });
  const input = h("input", { id: "mood", maxlength: "300", autocomplete: "off", placeholder: "rainy Sunday morning" });
  const go = h("button", { type: "submit" }, "Make a mix");
  const result = h("div", { "aria-live": "polite" });
  const form = h(
    "form",
    { class: "picker-new mood-form" },
    h("div", {}, h("label", { for: "mood" }, "A mood, in your own words"), input),
    go,
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const mood = input.value.trim();
    if (!mood) {
      said.textContent = "Say what it should sound like.";
      input.focus();
      return;
    }
    if (!hold(go)) return;
    said.textContent = "Asking for rules…";
    result.replaceChildren();
    try {
      const mix = await api("/ai/music/mix", { method: "POST", body: { description: mood } });
      if (!mix.rules) {
        said.textContent = `No rules came of it${mix.rejected.length ? `: ${mix.rejected.join("; ")}` : ""}. Try other words.`;
        return;
      }
      const found = await worked(mix.rules);
      said.textContent = mix.rejected.length ? `Some rules were left out: ${mix.rejected.join("; ")}` : "";
      const keep = h("button", { type: "button", class: "quiet" }, "Keep it as a smart playlist");
      keep.addEventListener("click", async () => {
        if (!hold(keep)) return;
        try {
          const made = await api("/playlists", { method: "POST", body: { name: mood.slice(0, 200), rules: found.rules } });
          playlistsChanged();
          location.hash = playlistHref(made.id);
        } catch (err) {
          if (err instanceof SignedOut) throw err;
          said.textContent = `Couldn't keep it: ${playlistTrouble(err)}`;
          release(keep);
        }
      });
      result.replaceChildren(...songsOf(found, "No song fits its rules now."), h("div", { class: "form-actions" }, keep));
    } catch (err) {
      if (err instanceof SignedOut) throw err;
      said.textContent = err.message;
    } finally {
      release(go);
    }
  });
  return h(
    "section",
    { class: "listen-row mix", "aria-labelledby": "mix-mood" },
    h("h2", { id: "mix-mood" }, "From a mood"),
    h("p", { class: "note" }, "Only the words you type go to the AI service an admin set up, which turns them into rules; the songs are picked here, from your library and listens."),
    form,
    said,
    result,
  );
}

export async function mixesPage(ctx) {
  const title = h("h1", {}, "Mixes");
  if (!seesAllMusic()) return show("Mixes", title, emptyState(SYNC_CLOSED));
  show("Mixes", title, placeholders("mixes", { kind: "rows", count: 6 }));
  const ai = await api("/ai").catch((err) => {
    if (err instanceof SignedOut) throw err;
    return { enabled: false };
  });
  if (!ctx.current()) return;
  const mixes = await Promise.all(BUILT_IN.map(builtIn));
  show("Mixes", title, ...mixes, ai.enabled ? fromMood() : null);
}
