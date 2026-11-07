// Asking for music by describing it ("something mellow for a rainy evening"), above the Songs
// list and from the command palette, only while AI features are on (`GET /ai`). The phrase goes
// to the AI service an admin set up, through Melete (`POST /ai/music/search`), as the Aoide apps
// send it; what comes back is filters (genres the library has, years, liked only, an order), which
// this page runs against the library itself, as the apps do: the AI never picks a song.
//
// askSection(ctx, params) → the section for the Songs page, empty until AI is known to be on;
// `?ask=` on the page's address runs that phrase at once (the palette's way in).
// musicAskHref(phrase) is the Songs page asking it.

import { SignedOut, api, h, number, plural } from "/web/lib.js";
import { playLast, playNext } from "/web/music-player.js";
import { looseQueue, play, shufflePlay } from "/web/music-play.js";
import { hold, release } from "/web/page-parts.js";

// Songs asked for per genre, and kept at most: a long queue is still a queue.
const PER_GENRE = 500;
const KEEP = 200;
// Whether AI is on, asked once per page load (it changes only when an admin sets a key).
let aiOn = null;
export const musicAi = () => (aiOn ??= api("/ai").then((a) => Boolean(a.enabled), () => false));

export const musicAskHref = (phrase) => `#/music/songs?${new URLSearchParams({ ask: phrase })}`;

// The library's songs matching `filters` (as `POST /ai/music/search` answers), in its order.
async function songsFor(filters) {
  const query = (more) => api(`/music/tracks?${new URLSearchParams({ limit: String(PER_GENRE), sort: "title", ...more })}`).then((p) => p.items);
  const term = filters.search_term ? { q: filters.search_term } : {};
  let found;
  if (filters.genres.length) {
    const pages = await Promise.all(filters.genres.slice(0, 5).map((genre) => query({ ...term, genre })));
    found = [...new Map(pages.flat().map((t) => [t.id, t])).values()];
  } else if (filters.search_term) {
    found = await query(term);
  } else if (filters.liked_only) {
    found = ((await api("/music/likes")).tracks ?? []).map((l) => l.track).filter(Boolean);
  } else {
    found = await query({ sort: "recent" });
  }
  const from = filters.from_year ?? -Infinity;
  const to = filters.to_year ?? Infinity;
  const years = filters.from_year !== null || filters.to_year !== null;
  found = found.filter((t) => (!filters.liked_only || t.liked) && (!years || (t.year !== null && t.year >= from && t.year <= to)));
  return ordered(found, filters.ordering);
}

// The order the description asked for: by name, newest first, at random, or by your listening.
async function ordered(tracks, ordering) {
  if (ordering === "newest_first") return tracks.sort((a, b) => (b.year ?? 0) - (a.year ?? 0)).slice(0, KEEP);
  if (ordering === "random") {
    for (let i = tracks.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      [tracks[i], tracks[j]] = [tracks[j], tracks[i]];
    }
    return tracks.slice(0, KEEP);
  }
  if (ordering === "most_played" || ordering === "recently_played") {
    // Your listens, newest first: how often, and how lately, each song was played.
    const history = await api("/music/plays?limit=500").catch((err) => {
      if (err instanceof SignedOut) throw err;
      return { items: [] };
    });
    const count = new Map();
    const last = new Map();
    (history.items ?? []).forEach((listen, i) => {
      count.set(listen.track_id, (count.get(listen.track_id) ?? 0) + 1);
      if (!last.has(listen.track_id)) last.set(listen.track_id, i);
    });
    const by = ordering === "most_played" ? (t) => -(count.get(t.id) ?? 0) : (t) => last.get(t.id) ?? Infinity;
    return tracks.sort((a, b) => by(a) - by(b)).slice(0, KEEP);
  }
  return tracks.slice(0, KEEP);
}

// What the filters came to, in words: "Jazz, Soul · 1990–1999 · liked only".
function described(filters) {
  const years =
    filters.from_year !== null || filters.to_year !== null
      ? filters.from_year === filters.to_year
        ? String(filters.from_year)
        : `${filters.from_year ?? "up"}–${filters.to_year ?? "now"}`
      : null;
  return [filters.search_term ? `“${filters.search_term}”` : null, filters.genres.join(", ") || null, years, filters.liked_only ? "liked only" : null]
    .filter(Boolean)
    .join(" · ");
}

export function askSection(ctx, params) {
  const input = h("input", { id: "music-ask", type: "text", maxlength: "300", autocomplete: "off", placeholder: "Something mellow for a rainy evening" });
  const button = h("button", { type: "submit" }, "Ask");
  const said = h("div", { class: "music-ask-said", role: "status" });
  const results = h("div", { class: "music-ask-results" });
  const form = h(
    "form",
    { class: "search-row music-ask-form", novalidate: true },
    h("div", {}, h("label", { for: "music-ask" }, "Ask by description"), input),
    button,
  );
  const section = h(
    "section",
    { class: "music-ask", "aria-label": "Ask by description", hidden: true },
    form,
    h("p", { class: "note" }, "What you type goes to the AI service set up for Melete, which answers with genres and years from this library; the songs come from the library itself."),
    said,
    results,
  );
  const ask = async (phrase) => {
    if (!hold(button)) return;
    said.replaceChildren(h("p", { class: "status" }, "Asking…"));
    results.replaceChildren();
    try {
      const filters = await api("/ai/music/search", { method: "POST", body: { query: phrase } });
      const tracks = await songsFor(filters);
      if (!ctx.current()) return;
      const about = described(filters);
      said.replaceChildren(
        h("p", { class: "status" }, tracks.length ? `${plural(tracks.length, "song", "songs")}${about ? ` for ${about}` : ""}.` : `No songs in the library for ${about || "that"}. Try other words.`),
      );
      if (tracks.length) results.replaceChildren(...actions(tracks));
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      said.replaceChildren(h("p", { class: "error" }, err.status === 429 ? "That's 30 AI searches this hour. Try again later." : err.message));
    } finally {
      release(button);
    }
  };
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const phrase = input.value.trim();
    if (!phrase) return input.focus();
    ask(phrase);
  });
  musicAi().then((on) => {
    if (!on || !ctx.current()) return;
    section.hidden = false;
    const asked = params.get("ask")?.trim();
    if (asked) {
      input.value = asked.slice(0, 300);
      ask(input.value);
    }
  });
  return section;
}

// Play, Shuffle, Play next and Play last for what was found, and the first few by name.
function actions(tracks) {
  const queue = looseQueue(tracks);
  const shown = tracks.slice(0, 8).map((t) => h("li", {}, h("span", { class: "clip" }, `${t.title} – ${t.artist}`)));
  return [
    h(
      "div",
      { class: "play-actions" },
      h("button", { type: "button", onclick: () => play(queue) }, "Play"),
      h("button", { type: "button", class: "quiet", onclick: () => shufflePlay(queue) }, "Shuffle"),
      h("button", { type: "button", class: "quiet", onclick: () => playNext(queue) }, "Play next"),
      h("button", { type: "button", class: "quiet", onclick: () => playLast(queue) }, "Play last"),
    ),
    h("ul", { class: "music-ask-list" }, shown),
    h("p", { class: "note", hidden: tracks.length <= shown.length }, `And ${number.format(tracks.length - shown.length)} more.`),
  ];
}
