// "All shows", "All anime", "All movies": the full grid at the foot of Watch's Shows, Anime and
// Movies pages, with search, sort, genre (and a category, where libraries have one besides
// anime), a page at a time, and finding a title by describing it when AI features are on. What's
// shown is kept in the page's address, so Back and a reload return to it.
//
// everything(page, params, gen, { genres, libraries, ai }) → the section, its grid loading.
// `page` is "shows", "anime" or "movies": shows are `/series` with anime left out, anime is
// `/series` with only anime, movies `/movies`.

import { SignedOut, announce, api, append, failure, h, number, plural } from "/web/lib.js";
import { generation, signIn } from "/web/page-state.js";
import { titleCard } from "/web/cards.js";

const PAGE_SIZE = 100;
const SORTS = [
  ["title", "Title"],
  ["year", "Newest"],
  ["recent", "Recently added"],
];

const validSort = (key) => (SORTS.some(([k]) => k === key) ? key : "title");
const CATEGORY = { anime: "Anime", kids: "Kids" };
// Each page's list: the kind it lists, its anime filter and its nouns.
const PAGES = {
  shows: { kind: "series", anime: "exclude", noun: ["show", "shows"] },
  anime: { kind: "series", anime: "only", noun: ["anime series", "anime series"] },
  movies: { kind: "movies", anime: null, noun: ["movie", "movies"] },
};

export function everything(page, params, gen, { genres, libraries, ai }) {
  const about = PAGES[page];
  // What the grid's parts share: what's shown, and the boxes it's shown in.
  const v = {
    page,
    kind: about.kind,
    anime: about.anime,
    gen,
    noun: about.noun,
    q: params.get("q") ?? "",
    sort: validSort(params.get("sort")),
    // Genre and category narrow the list; both are kept in the address too.
    genre: params.get("genre") ?? "",
    category: params.get("category") ?? "",
  };
  const theirs = libraries.filter((l) => l.kind === (v.kind === "movies" ? "movies" : "shows"));
  // Anime has a page of its own, so the series pages offer only the other categories.
  v.categories =
    page === "anime" ? [] : [...new Set(theirs.map((l) => l.category).filter((c) => c && (v.kind === "movies" || c !== "anime")))];
  if (v.category !== "none" && !v.categories.includes(v.category)) v.category = "";
  // How many were shown before leaving, so Back returns to the same place in a long list.
  v.shown = Math.min(Math.max(Number(params.get("shown")) || PAGE_SIZE, PAGE_SIZE), 500);
  v.reloads = 0;
  const { search, sorter } = filterControls(v, genres);
  v.count = h("p", { class: "status", "aria-live": "polite" });
  v.grid = h("ul", { class: "grid" });
  v.more = h("div", { class: "more" });
  listen(v, sorter);
  reload(v).catch((err) => failed(v, err));
  return h(
    "section",
    { class: "everything", "aria-labelledby": "everything-head" },
    h("h2", { id: "everything-head" }, `All ${v.noun[1]}`),
    h(
      "div",
      { class: "toolbar" },
      h("div", {}, h("label", { for: "search" }, `Search ${v.noun[1]}`), search),
      h("div", {}, h("label", { for: "sort" }, "Sort by"), sorter),
      // Genres come from TMDB, so a library not matched yet has none to offer.
      v.genrePick.options.length > 1 ? h("div", {}, h("label", { for: "genre" }, "Genre"), v.genrePick) : null,
      v.categoryPick ? h("div", {}, h("label", { for: "category" }, "Show"), v.categoryPick) : null,
    ),
    ai.enabled ? describeIt(v.kind, v.noun) : null,
    v.count,
    v.grid,
    v.more,
  );
}

// The search box, sort, genre and category pickers, set to what's shown. The search box and
// pickers go in `v`, which the reload and its Clear button need; returns the search and sort.
function filterControls(v, genres) {
  const count0 = (g) => (v.kind === "movies" ? g.movies : g.series);
  const search = h("input", { id: "search", type: "search", value: v.q, autocomplete: "off", spellcheck: "false" });
  const sorter = h(
    "select",
    { id: "sort" },
    SORTS.map(([key, name]) => h("option", { value: key, selected: key === v.sort }, name)),
  );
  v.genrePick = h(
    "select",
    { id: "genre" },
    h("option", { value: "" }, "Any genre"),
    genres.filter((g) => count0(g) > 0).map((g) => h("option", { value: g.name, selected: g.name.toLowerCase() === v.genre.toLowerCase() }, g.name)),
  );
  v.categoryPick = v.categories.length
    ? h(
        "select",
        { id: "category" },
        h("option", { value: "" }, "Everything"),
        v.categories.map((c) => h("option", { value: c, selected: c === v.category }, `${CATEGORY[c] ?? c} only`)),
        h("option", { value: "none", selected: v.category === "none" }, `Without ${v.categories.map((c) => (CATEGORY[c] ?? c).toLowerCase()).join(" or ")}`),
      )
    : null;
  v.search = search;
  return { search, sorter };
}

// Typing a search, sorting and narrowing each start the list again from the top.
function listen(v, sorter) {
  const { search, genrePick, categoryPick } = v;
  const again = () => {
    v.shown = PAGE_SIZE;
    remember(v);
    reload(v).catch((err) => failed(v, err));
  };
  let typing;
  search.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(() => {
      // Typed just before leaving the page: the next page owns the URL now.
      if (v.gen !== generation || search.value.trim() === v.q) return;
      v.q = search.value.trim();
      again();
    }, 300);
  });
  sorter.addEventListener("change", () => {
    v.sort = validSort(sorter.value);
    again();
  });
  genrePick.addEventListener("change", () => {
    v.genre = genrePick.value;
    again();
  });
  categoryPick?.addEventListener("change", () => {
    v.category = categoryPick.value;
    again();
  });
}

// The URL says what's shown without adding history entries, so Back leaves the page.
function remember(v) {
  const next = new URLSearchParams();
  if (v.q) next.set("q", v.q);
  if (v.sort !== "title") next.set("sort", v.sort);
  if (v.genre) next.set("genre", v.genre);
  if (v.category) next.set("category", v.category);
  if (v.shown > PAGE_SIZE) next.set("shown", v.shown);
  history.replaceState(history.state, "", `#/${v.page}${next.toString() ? `?${next}` : ""}`);
}

// Loads the first `shown` titles for the current search and sort, replacing what's there.
async function reload(v) {
  const { kind, noun, count, grid, more } = v;
  const mine = ++v.reloads;
  const stale = () => v.gen !== generation || mine !== v.reloads;
  count.textContent = `Loading ${noun[1]}…`;
  more.replaceChildren();
  const query = new URLSearchParams({ sort: v.sort });
  if (v.anime) query.set("anime", v.anime);
  if (v.q) query.set("q", v.q);
  if (v.genre) query.set("genre", v.genre);
  if (v.category === "none") query.set("exclude_category", v.categories.join(","));
  else if (v.category) query.set("category", v.category);
  const load = (offset, limit) => {
    query.set("offset", offset);
    query.set("limit", limit);
    return api(`/${kind}?${query}`);
  };
  const page = await load(0, v.shown);
  if (stale()) return;
  grid.replaceChildren(...page.items.map((item) => titleCard(kind, item)));
  const narrowed = Boolean(v.q || v.genre || v.category);
  if (page.total === 0) {
    count.textContent = narrowed ? `No ${noun[1]} match${v.q ? ` “${v.q}”` : ""}${v.genre ? ` in ${v.genre}` : ""}.` : `No ${noun[1]} yet.`;
    if (narrowed) {
      const clear = () => {
        v.search.value = v.q = "";
        v.genrePick.value = v.genre = "";
        if (v.categoryPick) v.categoryPick.value = v.category = "";
        remember(v);
        v.search.focus();
        reload(v).catch((err) => failed(v, err));
      };
      more.replaceChildren(h("button", { type: "button", class: "quiet", onclick: clear }, "Clear search and filters"));
    }
    return;
  }
  const total = plural(page.total, noun[0], noun[1]);
  count.textContent = [total, v.q ? `match “${v.q}”` : null, v.genre ? `in ${v.genre}` : null].filter(Boolean).join(" ");
  offerMore(v, { page, load, stale, loaded: page.items.length });
}

// Show more (N left) under the list, loading the next page into it when pressed.
function offerMore(v, got) {
  const { kind, grid, more } = v;
  const { page, load, stale } = got;
  const left = page.total - got.loaded;
  if (left <= 0) return more.replaceChildren();
  const button = h("button", { type: "button", class: "quiet" }, `Show more (${number.format(left)} left)`);
  button.addEventListener("click", async () => {
    button.disabled = true;
    button.textContent = "Loading…";
    try {
      const next = await load(got.loaded, PAGE_SIZE);
      if (stale()) return;
      const cards = next.items.map((item) => titleCard(kind, item));
      append(grid, cards);
      got.loaded += next.items.length;
      v.shown = Math.min(got.loaded, 500);
      remember(v);
      offerMore(v, got);
      // Keep keyboard users where they were: on the first new title.
      cards[0]?.querySelector("a").focus();
      announce(`Showing ${number.format(got.loaded)} of ${number.format(page.total)}`);
    } catch (err) {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      const retry = () => {
        offerMore(v, got);
        more.querySelector("button")?.focus();
      };
      more.replaceChildren(failure(err, retry));
    }
  });
  more.replaceChildren(button);
}

// A reload that failed: back to sign-in when signed out, otherwise Try again in the list.
function failed(v, err) {
  if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
  v.count.textContent = "";
  v.more.replaceChildren(failure(err, () => reload(v).catch((e) => failed(v, e))));
}

// Finding a title by describing it, when an admin has turned AI features on. Only the
// words typed leave the server, to the AI service; the page says so before anything is sent.
function describeIt(kind, noun) {
  const input = h("input", { id: "describe", type: "text", autocomplete: "off", maxlength: 300 });
  const error = h("p", { class: "field-error", id: "describe-error", hidden: true });
  const status = h("p", { class: "status", role: "status" });
  const found = h("div");
  const button = h("button", { type: "submit" }, "Find");
  const box = h(
    "form",
    { class: "describe", novalidate: true },
    h("label", { for: "describe" }, `Describe what you're in the mood for`),
    h("p", { class: "note", id: "describe-hint" }, `For example “a slow, funny ${kind === "movies" ? "film" : "show"} about a family”. What you type goes to the AI service an admin set up, and nothing else does.`),
    h("div", { class: "describe-row" }, input, button),
    error,
    status,
    found,
  );
  input.setAttribute("aria-describedby", "describe-hint");
  box.addEventListener("submit", async (event) => {
    event.preventDefault();
    const query = input.value.trim();
    error.hidden = true;
    input.removeAttribute("aria-invalid");
    if (!query) {
      error.textContent = "Describe it in a few words first.";
      error.hidden = false;
      input.setAttribute("aria-invalid", "true");
      input.setAttribute("aria-describedby", "describe-hint describe-error");
      input.focus();
      return;
    }
    button.disabled = true;
    status.textContent = "Asking…";
    found.replaceChildren();
    try {
      const answer = await api("/ai/search", { method: "POST", body: { kind, query } });
      const missing = answer.suggested.filter((s) => s.id === null);
      status.textContent = answer.results.length
        ? `${plural(answer.results.length, noun[0], noun[1])} here fit.`
        : `Nothing in the library fits; ${missing.length ? "these might:" : "try other words."}`;
      const parts = [
        answer.results.length ? h("ul", { class: "grid shelf" }, answer.results.map((item) => titleCard(kind, item))) : null,
        missing.length
          ? h(
              "p",
              { class: "note" },
              `Not in the library: ${missing.map((m) => (m.year ? `${m.title} (${m.year})` : m.title)).join(", ")}. `,
              h("a", { href: "#/requests/ask" }, "Ask for one"),
            )
          : null,
      ];
      found.replaceChildren(...parts.filter(Boolean));
    } catch (err) {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      status.textContent = "";
      found.replaceChildren(h("p", { class: "error" }, err.message));
    } finally {
      button.disabled = false;
    }
  });
  return h("details", { class: "describe-box" }, h("summary", {}, "Describe it instead"), box);
}
