// Search: the library's movies and shows that match what's typed, with their posters, then,
// while discovery is on, TMDB's titles that aren't in the library, each to ask for. The words
// go to TMDB only then; with discovery off, only the library is searched. What's typed is kept
// in the page's address (#/search?q=…), so Back, a reload and the palette's "See all results"
// open the same search. Before anything is typed, while discovery is on, what's trending this
// week (trending.js).

import { SignedOut, api, h, plural, show } from "/web/lib.js";
import { generation, signIn } from "/web/page-state.js";
import { titleCard } from "/web/cards.js";
import { failed, placeholders } from "/web/page-parts.js";
import { candidateRow } from "/web/requests.js";
import { trendingSection } from "/web/trending.js";

// How many of each kind the library shows, and TMDB's per kind.
const LIBRARY_LIMIT = 24;
const OUTSIDE_LIMIT = 10;

const signedOut = () => signIn("Your session ended. Sign in again.");

export async function searchPage(params, gen) {
  const v = {
    gen,
    q: (params.get("q") ?? "").trim(),
    asking: 0,
    discovery: false,
    input: h("input", { id: "search-words", type: "search", autocomplete: "off", spellcheck: "false", maxlength: 200 }),
    said: h("p", { class: "status", role: "status" }),
    // What's shown before anything is typed.
    start: h("div", { class: "search-start" }),
    library: h("section", { class: "search-library", "aria-labelledby": "search-library-head", hidden: true }),
    outside: h("section", { class: "search-outside", "aria-labelledby": "search-outside-head", hidden: true }),
  };
  v.input.value = v.q;
  const box = h(
    "form",
    { class: "toolbar", role: "search", novalidate: true },
    h("div", {}, h("label", { for: "search-words" }, "Search movies and shows"), v.input),
  );
  show(
    v.q ? `Search: ${v.q}` : "Search",
    h("h1", {}, "Search"),
    box,
    v.said,
    v.start,
    v.library,
    v.outside,
    h("p", { class: "note" }, "Not here, or looking for music? ", h("a", { href: "#/requests/ask" }, "Ask for something"), "."),
  );
  try {
    v.discovery = (await api("/discover/status")).on;
  } catch (err) {
    if (err instanceof SignedOut) return signedOut();
    // Unknown is off: nothing goes to TMDB.
  }
  if (gen !== generation) return;
  if (v.discovery) v.start.append(trendingSection({ signedOut }, { id: "search-trending", on: true }));
  listen(v, box);
  find(v);
}

// Typing searches once it pauses; Enter at once.
function listen(v, box) {
  let typing;
  const again = () => {
    clearTimeout(typing);
    const q = v.input.value.trim();
    if (v.gen !== generation || q === v.q) return;
    v.q = q;
    // The address says what's searched without adding history entries, so Back leaves the page.
    history.replaceState(history.state, "", q ? `#/search?${new URLSearchParams({ q })}` : "#/search");
    document.title = q ? `Search: ${q} – Melete` : "Search – Melete";
    find(v);
  };
  v.input.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(again, 400);
  });
  box.addEventListener("submit", (event) => {
    event.preventDefault();
    again();
  });
}

// Searches the library, and TMDB while discovery is on, for what's typed, replacing what's shown.
async function find(v) {
  const mine = ++v.asking;
  const stale = () => v.gen !== generation || mine !== v.asking;
  const { q } = v;
  v.start.hidden = Boolean(q);
  if (!q) {
    v.library.hidden = v.outside.hidden = true;
    v.said.textContent = v.discovery
      ? "Type a title to find it in the library, or to ask for one that isn't."
      : "Type a title to find it in the library.";
    return;
  }
  v.said.textContent = "Searching…";
  const library = findInLibrary(v, q, stale);
  const outside = v.discovery && q.length >= 2 ? findOutside(v, q, stale) : Promise.resolve(null);
  if (!(v.discovery && q.length >= 2)) v.outside.hidden = true;
  const [here, there] = await Promise.all([library, outside]);
  if (stale()) return;
  const counts = [
    here === null ? null : here ? `${plural(here, "title", "titles")} in the library` : "Nothing in the library",
    there === null ? null : `${there} not in the library`,
  ].filter(Boolean);
  v.said.textContent = counts.length ? `${counts.join(", ")}.` : "";
}

// The library's movies and shows called that, as posters. Answers how many, or null on failure.
async function findInLibrary(v, q, stale) {
  v.library.hidden = false;
  v.library.replaceChildren(
    h("h2", { id: "search-library-head" }, "In the library"),
    placeholders("the library's titles", { kind: "cards", count: 6 }),
  );
  const query = new URLSearchParams({ q, limit: String(LIBRARY_LIMIT) });
  let movies, series;
  try {
    [movies, series] = await Promise.all([api(`/movies?${query}`), api(`/series?${query}`)]);
  } catch (err) {
    if (err instanceof SignedOut) return signedOut();
    if (stale()) return null;
    v.library.replaceChildren(h("h2", { id: "search-library-head" }, "In the library"), failed("the library's titles", err, () => find(v)));
    return null;
  }
  if (stale()) return null;
  const cards = [...movies.items.map((m) => titleCard("movies", m)), ...series.items.map((s) => titleCard("series", s))];
  v.library.replaceChildren(
    h("h2", { id: "search-library-head" }, "In the library"),
    cards.length ? h("ul", { class: "grid" }, cards) : h("p", { class: "status" }, `Nothing in the library is called “${q}”.`),
  );
  return movies.total + series.total;
}

// TMDB's movies and series called that which aren't in the library, each to ask for. Answers how
// many, or null on failure.
async function findOutside(v, q, stale) {
  v.outside.hidden = false;
  const head = h("h2", { id: "search-outside-head" }, "Not in the library");
  v.outside.replaceChildren(head, placeholders("titles not in the library", { kind: "rows", count: 3 }));
  let movies, series;
  try {
    [movies, series] = await Promise.all(
      ["movie", "series"].map((kind) => api(`/discover/search?${new URLSearchParams({ q, kind })}`)),
    );
  } catch (err) {
    if (err instanceof SignedOut) return signedOut();
    if (stale()) return null;
    v.outside.replaceChildren(head, failed("titles not in the library", err, () => find(v)));
    return null;
  }
  if (stale()) return null;
  const elsewhere = (items) => items.filter((d) => d.state !== "playable").slice(0, OUTSIDE_LIMIT);
  const found = [...elsewhere(movies), ...elsewhere(series)];
  const ctx = { signedOut };
  v.outside.replaceChildren(
    head,
    found.length
      ? h("ul", { class: "rows" }, found.map((d) => candidateRow(ctx, candidate(d))))
      : h("p", { class: "status" }, `TMDB has nothing else called “${q}”.`),
  );
  return found.length;
}

// A title TMDB found, as asking for it takes it (a request candidate).
function candidate(d) {
  return {
    kind: d.kind,
    tmdb_id: d.tmdb_id,
    title: d.title,
    year: d.year,
    artist: null,
    detail: null,
    in_library: false,
    // Asked for by you, or wanted by the household already.
    requested: d.request_id,
    wanted: d.state === "requested" && !d.request_id,
    poster: d.poster,
  };
}
