// Add new's search: results as you type, every kind the service has at once (artists, albums
// and songs for Music; movies; series), each group best first. A search starts once typing
// pauses; one in flight is cancelled when the words change, so at most one runs. Arrow keys
// move through the results' Add buttons and Enter opens one; Escape goes back to the words.
// When MusicBrainz finds nothing for music, Soulseek can be searched for the same words. Only
// the words leave the server, to TMDB, MusicBrainz or slskd.

import { group } from "/web/forms.js";
import { SignedOut, api, h, plural } from "/web/lib.js";
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
  const v = { page, music, source, q, said, results, running: null, timer: null, words: "" };
  idle(v);
  q.addEventListener("input", () => {
    clearTimeout(v.timer);
    v.timer = setTimeout(() => find(v), PAUSE_MS);
  });
  search.addEventListener("submit", (event) => {
    event.preventDefault();
    clearTimeout(v.timer);
    find(v, { again: true });
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
    h("p", { class: "note" }, `Results appear as you type. Only the words you search for leave the server, to ${source}${music ? ", or to Soulseek when you ask" : ""}. Adding one puts it on the wanted list at once, as an approved request.`),
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
  if (running.signal.aborted || !v.page.ctx.current()) return;
  v.running = null;
  v.results.removeAttribute("aria-busy");
  const failed = answers.find((a) => a.status === "rejected")?.reason;
  if (failed instanceof SignedOut) return v.page.ctx.signedOut();
  const groups = kinds.map(([kind, name], i) => ({ kind, name, found: answers[i].status === "fulfilled" ? answers[i].value : null }));
  if (groups.every((g) => g.found === null)) return trouble(v, failed);
  draw(v, words, groups, failed);
}

// The results, a group per kind with what it found.
function draw(v, words, groups, failed) {
  const total = groups.reduce((n, g) => n + (g.found?.length ?? 0), 0);
  const parts = groups
    .filter((g) => g.found?.length)
    .map((g) =>
      h(
        "section",
        { class: "find-group", "aria-label": g.name },
        groups.length > 1 ? h("h3", {}, `${g.name} (${g.found.length})`) : null,
        h("ul", { class: "rows" }, g.found.map((c) => candidateRow(v.page, c))),
      ),
    );
  const missed = groups.filter((g) => g.found === null).map((g) => g.name.toLowerCase());
  const partly = missed.length ? ` ${missed.join(" and ")} couldn't be searched: ${failed?.message ?? ""}` : "";
  if (!total) {
    v.said.textContent = `${v.source} found nothing for “${words}”.${partly}`;
    v.results.replaceChildren(
      h("p", { class: "note" }, "Try another spelling, or fewer words."),
      v.music ? soulseekOffer(v, words) : null,
    );
    return;
  }
  v.said.textContent = `${plural(total, "result", "results")} for “${words}”. Arrow down to go through them.${partly}`;
  v.results.replaceChildren(...parts);
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

// MusicBrainz found nothing: look on Soulseek for the same words, and want a song from there by
// its title and artist.
function soulseekOffer(v, words) {
  const box = h("div", { class: "soulseek-offer" });
  const go = h("button", { type: "button" }, `Search Soulseek for “${words}”`);
  go.addEventListener("click", async () => {
    go.disabled = true;
    v.said.textContent = `Searching Soulseek for “${words}”. This takes ten seconds or more…`;
    try {
      const heard = await api(`/requests/soulseek?${new URLSearchParams({ q: words })}`);
      if (!v.page.ctx.current() || v.words !== words) return;
      v.said.textContent = heard.length ? `Soulseek's users share ${plural(heard.length, "song", "songs")} for “${words}”.` : `Nothing on Soulseek for “${words}” either.`;
      box.replaceChildren(
        heard.length
          ? h(
              "section",
              { class: "find-group", "aria-label": "On Soulseek" },
              h("h3", {}, "On Soulseek"),
              h("p", { class: "note" }, "Named as their files are. Adding one wants it as a song by its title and artist, fetched from Soulseek."),
              h("ul", { class: "rows" }, heard.map((s) => candidateRow(v.page, { kind: "song", title: s.title, artist: s.artist, detail: `Shared by ${plural(s.users, "user", "users")}, ${s.formats.join(", ")}`, poster: null }))),
            )
          : h("p", { class: "note" }, "Try other words."),
      );
      box.querySelector(".find-add")?.focus();
    } catch (err) {
      if (err instanceof SignedOut) return v.page.ctx.signedOut();
      v.said.textContent = `Soulseek couldn't be searched: ${err.message}`;
      go.disabled = false;
    }
  });
  box.append(h("p", { class: "note" }, "Music is fetched from Soulseek, which may have what MusicBrainz doesn't list. Only these words go to slskd."), go);
  return box;
}
