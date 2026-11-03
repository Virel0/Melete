// Add new's search: results as you type, every kind the service has at once (artists, albums
// and songs for Music; movies; series), each group best first. A search starts once typing
// pauses; one in flight is cancelled when the words change, so at most one runs. Arrow keys
// move through the results' Add buttons and Enter opens one; Escape goes back to the words.
// While slskd is set up, Music also looks for songs on Soulseek once typing has stopped; they
// join MusicBrainz's songs when they come, without holding them up, and the two are ranked
// together by how close each is to the words (the server's `closeness`). Only the words leave
// the server, to TMDB, MusicBrainz or slskd.

import { group } from "/web/forms.js";
import { SignedOut, api, h, number, plural } from "/web/lib.js";
import { candidateRow } from "/web/manage-add-form.js";

// What each service searches for: [kind, the group's name].
const KINDS = {
  movies: [["movie", "Movies"]],
  series: [["series", "Series"]],
  anime: [["series", "Series"]],
  music: [
    ["artist", "Artists"],
    ["album", "Albums"],
    ["song", "Songs"],
  ],
};
// How long typing pauses before a search starts, and the fewest letters searched for.
const PAUSE_MS = 400;
const FEWEST = 2;
// Soulseek's search takes ten seconds or more and goes outside: it starts only once typing has
// stopped for longer, never for words half typed, and for at least this many letters.
const SEEK_PAUSE_MS = 1200;
const SEEK_FEWEST = 3;

export function findPart(page) {
  const music = page.key === "music";
  const source = music ? "MusicBrainz" : "TMDB";
  const q = h("input", { id: "add-q", type: "search", autocomplete: "off", spellcheck: "false", maxlength: 200, "aria-describedby": "add-said", "aria-controls": "add-results" });
  const said = h("p", { class: "status find-said", id: "add-said", role: "status" });
  const results = h("div", { class: "add-results", id: "add-results" });
  const search = h(
    "form",
    { class: "search-row", role: "search", novalidate: true },
    h("div", { class: "find-words" }, h("label", { for: "add-q" }, music ? "Artist, album or song" : "Title"), q),
  );
  // Whether slskd is set up, asked once: Soulseek is searched only then.
  const slskd = music ? api("/settings/services").then((all) => all.some((s) => s.key === "slskd" && s.on), () => false) : Promise.resolve(false);
  const v = { page, music, source, q, said, results, running: null, timer: null, words: "", shown: null, slskd, seek: null, seeking: false, seekTimer: null, rows: new WeakMap() };
  idle(v);
  q.addEventListener("input", () => {
    clearTimeout(v.timer);
    v.timer = setTimeout(() => find(v), PAUSE_MS);
    if (!music) return;
    clearTimeout(v.seekTimer);
    v.seekTimer = setTimeout(() => {
      v.seekTimer = null;
      seek(v);
    }, SEEK_PAUSE_MS);
  });
  search.addEventListener("submit", (event) => {
    event.preventDefault();
    clearTimeout(v.timer);
    find(v, { again: true });
    if (!music) return;
    clearTimeout(v.seekTimer);
    v.seekTimer = null;
    seek(v);
  });
  q.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      const first = results.querySelector(".find-add");
      if (first) {
        event.preventDefault();
        first.focus();
      }
    }
  });
  results.addEventListener("keydown", (event) => move(v, event));
  return group(
    "Add new",
    h("p", { class: "note" }, `Results appear as you type. Only the words you search for go to ${source}${music ? ", and to Soulseek once you stop typing when slskd is set up: its songs join MusicBrainz's when they come, ranked together by how close each is to the words" : ""}${music ? "" : "; posters of titles not here come from TMDB only when that is on under Outside services"}. Adding one puts it on the wanted list at once, as an approved request.`),
    search,
    said,
    results,
  );
}

// Up and down through the results' Add buttons; Escape back to the words.
function move(v, event) {
  if (event.key === "Escape" && event.target.classList.contains("find-add")) {
    event.preventDefault();
    v.q.focus();
    return;
  }
  if (!["ArrowDown", "ArrowUp"].includes(event.key) || !event.target.classList.contains("find-add")) return;
  event.preventDefault();
  const all = [...v.results.querySelectorAll(".find-add")];
  const at = all.indexOf(event.target) + (event.key === "ArrowDown" ? 1 : -1);
  if (at < 0) return v.q.focus();
  all[Math.min(at, all.length - 1)].focus();
}

function idle(v) {
  v.shown = null;
  v.said.textContent = `Type at least ${FEWEST} letters of a ${v.music ? "name or title" : "title"}.`;
  v.results.replaceChildren();
}

// Searches for the words unless they're what was searched last (or `again`), cancelling a
// search still running.
async function find(v, { again = false } = {}) {
  const words = v.q.value.trim();
  if (words === v.words && !again) return;
  v.running?.abort();
  v.running = null;
  v.words = words;
  if ([...words].length < FEWEST) return idle(v);
  const running = new AbortController();
  v.running = running;
  v.said.textContent = `Searching ${v.source} for “${words}”…`;
  v.results.setAttribute("aria-busy", "true");
  const kinds = KINDS[v.page.key];
  const answers = await Promise.allSettled(
    kinds.map(([kind]) => api(`/requests/search?${new URLSearchParams({ kind, q: words })}`, { signal: running.signal })),
  );
  // With Soulseek searched too, each song says where it's from.
  const sourced = await v.slskd;
  if (running.signal.aborted || !v.page.ctx.current()) return;
  v.running = null;
  v.results.removeAttribute("aria-busy");
  const failed = answers.find((a) => a.status === "rejected")?.reason;
  if (failed instanceof SignedOut) return v.page.ctx.signedOut();
  const groups = kinds.map(([kind, name], i) => {
    const found = answers[i].status === "fulfilled" ? answers[i].value : null;
    return { kind, name, found: kind === "song" && sourced && found ? found.map((c) => ({ ...c, source: "MusicBrainz" })) : found };
  });
  v.shown = { words, groups, failed };
  show(v);
}

// Looks on Soulseek for the words in the box, unless that's done or being done for them already:
// one search at a time, and an answer for words no longer in the box is dropped.
async function seek(v) {
  const words = v.q.value.trim();
  if ([...words.replace(/\s/g, "")].length < SEEK_FEWEST || v.seeking) return;
  if (v.seek?.words === words && !v.seek.error) return;
  if (!(await v.slskd) || v.seeking || v.q.value.trim() !== words) return;
  const asked = { words, songs: null, error: null };
  v.seek = asked;
  v.seeking = true;
  if (v.shown?.words === words) show(v);
  try {
    const heard = await api(`/requests/soulseek/candidates?${new URLSearchParams({ q: words })}`);
    asked.songs = heard.map((c) => ({ ...c, source: "Soulseek" }));
  } catch (err) {
    asked.error = err;
  }
  v.seeking = false;
  if (asked.error instanceof SignedOut) return v.page.ctx.signedOut();
  if (!v.page.ctx.current()) return;
  if (v.q.value.trim() !== words) {
    // The words changed meanwhile: the answer goes, and the new words are looked for unless
    // typing hasn't stopped yet (then its own pause does it).
    v.seek = null;
    if (v.seekTimer === null) seek(v);
    return;
  }
  if (v.shown?.words === words) show(v);
}

// Closest to the words first, by the server's one measure for both sources; MusicBrainz's
// first when they're as close.
function ranked(found, heard) {
  return [...found, ...heard].sort((a, b) => b.closeness - a.closeness);
}

// What MusicBrainz found for the words, with Soulseek's songs among the songs once they came.
function show(v) {
  const { words, groups, failed } = v.shown;
  const asked = v.seek?.words === words ? v.seek : null;
  const heard = asked?.songs ?? [];
  const missed = groups.filter((g) => g.found === null).map((g) => g.name.toLowerCase());
  if (missed.length === groups.length && !heard.length) {
    trouble(v, failed);
    if (asked) v.said.append(` ${soulseekSaid(asked, 0)}`);
    return;
  }
  const mixed = groups.map((g) => (g.kind === "song" && heard.length ? { ...g, found: ranked(g.found ?? [], heard) } : g));
  const partly = missed.length ? ` ${missed.join(" and ")} couldn't be searched: ${failed?.message ?? ""}` : "";
  draw(v, words, mixed, partly, asked);
}

// How Soulseek's search for the words stands, after `total` results.
function soulseekSaid(asked, total) {
  if (asked.error) return `Soulseek couldn't be searched: ${asked.error.message}`;
  if (!asked.songs) return "Searching Soulseek for songs too, which takes ten seconds or more…";
  if (!asked.songs.length) return `Nothing on Soulseek${total ? "" : " either"}.`;
  return "";
}

// The results, a group per kind with what it found. A row stays the same element from one
// drawing to the next, so an open Add form and the keyboard's focus stay where they were.
function draw(v, words, groups, partly, asked) {
  const total = groups.reduce((n, g) => n + (g.found?.length ?? 0), 0);
  const fromSoulseek = asked?.songs?.length ?? 0;
  const status = asked ? ` ${soulseekSaid(asked, total)}`.trimEnd() : "";
  const focused = document.activeElement;
  const parts = groups
    .filter((g) => g.found?.length)
    .map((g) =>
      h(
        "section",
        { class: "find-group", "aria-label": g.name },
        groups.length > 1 ? h("h3", {}, `${g.name} (${g.found.length})`) : null,
        g.kind === "song" && fromSoulseek ? h("p", { class: "note" }, "Ranked by how close each is to the words. Soulseek's are named as their files are; adding one wants it as a song by its title and artist, fetched from Soulseek.") : null,
        h("ul", { class: "rows" }, g.found.map((c) => rowOf(v, c))),
      ),
    );
  if (!total) {
    v.said.textContent = `${v.source} found nothing for “${words}”.${partly}${status}`;
    v.results.replaceChildren(h("p", { class: "note" }, "Try another spelling, or fewer words."));
    return;
  }
  const some = fromSoulseek ? `, ${number.format(fromSoulseek)} of them on Soulseek` : "";
  v.said.textContent = `${plural(total, "result", "results")} for “${words}”${some}. Arrow down to go through them.${partly}${status}`;
  v.results.replaceChildren(...parts);
  if (focused !== document.activeElement && v.results.contains(focused)) focused.focus({ preventScroll: true });
}

// A result's row, made once.
function rowOf(v, c) {
  let row = v.rows.get(c);
  if (!row) {
    row = candidateRow(v.page, c);
    v.rows.set(c, row);
  }
  return row;
}

// Why nothing could be searched: a service that's busy or off, or Melete out of reach; each
// with a way on.
function trouble(v, err) {
  const retry = h("button", { type: "button", class: "quiet", onclick: () => find(v, { again: true }) }, "Try again");
  if (err?.status === undefined) {
    v.said.textContent = "Melete can't be reached right now, so nothing was searched.";
    v.results.replaceChildren(h("p", { class: "note" }, "Check the connection; the search goes on as you type. ", retry));
  } else if (err.status === 502) {
    v.said.textContent = `${v.source} is busy or didn't answer.`;
    v.results.replaceChildren(h("p", { class: "note" }, "Keep typing to search again, or ", retry));
  } else {
    v.said.textContent = err.message;
    v.results.replaceChildren(h("p", { class: "note" }, "How TMDB and MusicBrainz stand is under ", h("a", { href: "#/admin/system/services" }, "System, Outside services"), "."));
  }
}
