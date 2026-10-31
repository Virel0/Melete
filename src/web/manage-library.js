// A Manage service's Library, the page it opens on: every title the service holds or wants,
// from its managed list, as posters or a table. Above it, what needs attention in the service
// and a toolbar (search for everything missing or below its cutoff, select several); then the
// filters, each with its count, and the key to the state marks below. What's shown (filter,
// search, sort, view, library) is kept in the address, without adding history entries.

import { SignedOut, announce, api, h, number, plural, size } from "/web/lib.js";
import { clip, emptyState, failed, placeholders, stateKey } from "/web/page-parts.js";
import { KINDS, attentionStrip, profilesFor, stateOf, titleHref } from "/web/manage-parts.js";
import { folderOf, selectKey, selectionBar } from "/web/manage-select.js";
import { searchMissing } from "/web/wanted-missing.js";
import { art } from "/web/cards.js";

const PAGE_SIZE = 100;
// Titles fetched a page at a time when selecting all shown: the most a list gives.
const ALL_PAGE = 500;

// The filters, each with how it narrows the managed list and where its count comes from.
const SHOWS = [
  { key: "", name: "All", query: {}, count: (c) => c.in_library + c.missing + c.downloading + c.unreleased + c.unmonitored },
  { key: "missing", name: "Missing", query: { state: "missing" }, count: (c) => c.missing },
  { key: "downloading", name: "Downloading", query: { state: "downloading" }, count: (c) => c.downloading },
  { key: "cutoff", name: "Cutoff unmet", query: { cutoff_unmet: "true" }, count: (c) => c.cutoff_unmet, upgrades: true },
  { key: "off", name: "Not monitored", query: { monitored: "false" } },
  { key: "notyet", name: "Not out yet", query: { state: "unreleased" }, count: (c) => c.unreleased },
  { key: "wanted", name: "Wanted", query: { wanted: "true" } },
];
const SORTS = [
  ["title", "Title"],
  ["added", "Added"],
  ["year", "Year"],
  ["size", "Size"],
];
const VIEWS = [
  ["posters", "Posters"],
  ["table", "Table"],
];

// `ctx.service` is the service from manage-services.js; `ctx.params` the address's query.
// `ctx.fixed` makes it one of Wanted's lists instead: { key, name, show, run }, the list always
// narrowed to `show`, a table unless asked otherwise, and its run of searches opened.
export async function libraryPage(ctx) {
  const kind = KINDS[ctx.service.key];
  const p = ctx.params;
  const fixed = ctx.fixed;
  const v = {
    ctx,
    kind,
    base: `#/admin/${ctx.service.key}/${fixed?.key ?? "library"}`,
    fixed,
    show: fixed?.show ?? SHOWS.find((s) => s.key === p.get("show") && (!s.upgrades || kind.upgrades))?.key ?? "",
    sort: SORTS.some(([k]) => k === p.get("sort")) ? p.get("sort") : "title",
    view: p.get("view") === "posters" || (!fixed && p.get("view") !== "table") ? "posters" : "table",
    q: p.get("q") ?? "",
    library: p.get("library") ?? "",
    reloads: 0,
    chosen: new Map(),
    selecting: false,
  };
  const heading = h("h1", {}, fixed?.name ?? ctx.service.name);
  const [libraries, runs, profiles] = await Promise.all([
    api("/libraries").catch(() => []),
    kind.area ? api("/missing-searches").catch(() => []) : [],
    profilesFor(kind).catch(() => []),
  ]);
  if (!ctx.current()) return [];
  // Series in an anime library are Anime's, so Series doesn't offer those.
  const kindOfLibrary = ctx.service.key === "movies" ? "movies" : ctx.service.key === "music" ? "music" : "shows";
  const offered = (l) => l.kind === kindOfLibrary && !(ctx.service.key === "series" && l.category === "anime");
  v.libraries = libraries.filter(offered).map((l) => l.name);
  v.latestRun = runs.find((r) => r.area === kind.area) ?? null;
  v.profiles = profiles;
  v.filters = h("div", { class: "filters", role: "group", "aria-label": "Show" });
  v.count = h("p", { class: "status", "aria-live": "polite" });
  v.list = h("div", { class: "library-list" }, placeholders(kind.many, { kind: v.view === "table" ? "rows" : "cards" }));
  v.more = h("div", { class: "more" });
  v.runs = h("div", { class: "library-runs" });
  v.selection = selectionBar(ctx, kind, {
    selected: () => [...v.chosen.values()],
    profiles,
    done: (message) => reloadKeeping(v, message),
    all: () => selectAll(v),
    clear: () => {
      v.chosen.clear();
      v.selection.update();
      draw(v);
    },
    shown: () => v.total ?? 0,
  });
  v.selection.bar.hidden = true;
  const parts = [
    heading,
    fixed ? null : attentionStrip(ctx.findings),
    toolbar(v),
    v.runs,
    v.selection.bar,
    fixed ? null : v.filters,
    v.count,
    v.list,
    v.more,
    stateKey(["have", "coming", "missing", "off", "notyet"]),
  ];
  if (fixed?.run && (fixed.run !== "cutoff" || kind.upgrades)) openRun(v, fixed.run, { focus: false });
  // The list comes after the page is in place; a failed one says so where the list goes.
  reload(v).catch((err) => fail(v, err));
  return parts;
}

function fail(v, err) {
  if (err instanceof SignedOut) return v.ctx.signedOut();
  v.list.replaceChildren(failed(v.kind.many, err, () => reload(v).catch((e) => fail(v, e))));
}

// Search, sort, view, the library when there's more than one, and what acts on the list.
function toolbar(v) {
  const { kind } = v;
  const search = h("input", { id: "library-search", type: "search", value: v.q, autocomplete: "off", spellcheck: "false" });
  let typing;
  search.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(() => {
      if (!v.ctx.current() || search.value.trim() === v.q) return;
      v.q = search.value.trim();
      again(v);
    }, 300);
  });
  v.search = search;
  const sorter = h("select", { id: "library-sort" }, SORTS.map(([key, name]) => h("option", { value: key, selected: key === v.sort }, name)));
  sorter.addEventListener("change", () => {
    v.sort = sorter.value;
    again(v);
  });
  const library = v.libraries.length > 1 ? h("select", { id: "library-library" }, h("option", { value: "" }, "Every library"), v.libraries.map((n) => h("option", { value: n, selected: n === v.library }, n))) : null;
  library?.addEventListener("change", () => {
    v.library = library.value;
    again(v);
  });
  v.libraryPick = library;
  const views = h(
    "div",
    { class: "segmented", role: "group", "aria-label": "View" },
    VIEWS.map(([key, name]) => {
      const b = h("button", { type: "button", class: "quiet", "aria-pressed": String(v.view === key) }, name);
      b.addEventListener("click", () => {
        if (v.view === key) return;
        v.view = key;
        for (const other of views.querySelectorAll("button")) other.setAttribute("aria-pressed", String(other === b));
        remember(v);
        draw(v);
      });
      return b;
    }),
  );
  const select = h("button", { type: "button", class: "quiet", "aria-pressed": "false" }, "Select");
  select.addEventListener("click", () => {
    v.selecting = !v.selecting;
    select.setAttribute("aria-pressed", String(v.selecting));
    select.textContent = v.selecting ? "Done selecting" : "Select";
    if (!v.selecting) v.chosen.clear();
    v.selection.bar.hidden = !v.selecting;
    v.selection.update();
    draw(v);
  });
  // Wanted's lists have their run open already.
  const runButtons = v.fixed ? [] : [
    kind.area ? h("button", { type: "button", class: "quiet", onclick: () => openRun(v, "missing") }, "Search all missing…") : null,
    kind.upgrades ? h("button", { type: "button", class: "quiet", onclick: () => openRun(v, "cutoff") }, "Search for upgrades…") : null,
  ];
  return h(
    "div",
    { class: "toolbar library-toolbar" },
    h("div", {}, h("label", { for: "library-search" }, `Search ${kind.many}`), search),
    h("div", {}, h("label", { for: "library-sort" }, "Sort by"), sorter),
    library ? h("div", {}, h("label", { for: "library-library" }, "Library"), library) : null,
    h("div", { class: "library-actions" }, views, select, runButtons),
  );
}

// The run of searches for everything missing, or for upgrades, shown above the list with what
// it searches and its Start and Stop; the latest run is followed while it goes on.
function openRun(v, goal, { focus = true } = {}) {
  if (goal === "missing") {
    v.runs.replaceChildren(searchMissing(v.ctx, v.kind.area, v.latestRun?.goal === "missing" ? v.latestRun : null));
  } else {
    v.runs.replaceChildren(searchUpgrades(v));
  }
  if (focus) v.runs.querySelector("button:not([hidden])")?.focus();
}

// Search for upgrades: one run for every monitored title below its cutoff, followed as a run
// for what's missing is, at Wanted's Cutoff unmet.
function searchUpgrades(v) {
  const said = h("p", { role: "status" });
  const start = h("button", { type: "button" }, "Search for upgrades");
  start.addEventListener("click", async () => {
    start.disabled = true;
    try {
      const run = await api("/cutoff-searches", { method: "POST", body: { area: v.kind.area } });
      v.latestRun = run;
      said.textContent = `Searching for upgrades: ${plural(run.total, "title", "titles")} so far. It goes on if you leave; another run can't start until it ends.`;
      announce("Searching for upgrades");
    } catch (err) {
      if (err instanceof SignedOut) return v.ctx.signedOut();
      said.textContent = err.message;
      said.classList.add("error");
    } finally {
      start.disabled = false;
    }
  });
  const anime = v.ctx.service.key === "series" || v.ctx.service.key === "anime" ? " Series and anime share one run." : "";
  return h(
    "div",
    { class: "missing-search" },
    h("p", { class: "note" }, `Searches for a better release of every monitored ${v.kind.one} whose file is below its profile's cutoff, one at a time and at most four a minute; only a release better than the file is grabbed.${anime}`),
    said,
    h("div", { class: "row-actions" }, start),
  );
}

// The filter buttons, each with its count; the counts are the whole service's, whatever's shown.
function drawFilters(v, counts, extra) {
  const shown = SHOWS.filter((s) => !s.upgrades || v.kind.upgrades);
  v.filters.replaceChildren(
    ...shown.map((s) => {
      const n = s.count ? s.count(counts) : extra[s.key];
      const b = h("button", { type: "button", class: "quiet", "aria-pressed": String(v.show === s.key) }, s.name, n === undefined ? null : h("span", { class: "count" }, number.format(n)));
      b.addEventListener("click", () => {
        v.show = s.key;
        again(v);
      });
      return b;
    }),
  );
}

// What's shown changed: the list starts again from the top, and the address says so.
function again(v) {
  v.shown = PAGE_SIZE;
  remember(v);
  reload(v).catch((err) => fail(v, err));
}

function remember(v) {
  const next = new URLSearchParams();
  if (v.show && !v.fixed) next.set("show", v.show);
  if (v.q) next.set("q", v.q);
  if (v.sort !== "title") next.set("sort", v.sort);
  if (v.view !== (v.fixed ? "table" : "posters")) next.set("view", v.view);
  if (v.library) next.set("library", v.library);
  history.replaceState(history.state, "", `${v.base}${next.toString() ? `?${next}` : ""}`);
}

function query(v, extra = {}) {
  const show = SHOWS.find((s) => s.key === v.show);
  const q = new URLSearchParams({ sort: v.sort, ...show.query, ...extra });
  if (v.q) q.set("q", v.q);
  if (v.library) q.set("library", v.library);
  return q;
}

// After a change to selected titles: the list again, with what happened said above it.
async function reloadKeeping(v, message) {
  v.chosen.clear();
  v.selection.update();
  await reload(v).catch((err) => fail(v, err));
  v.count.textContent = `${message} ${v.count.textContent}`;
}

// Loads the first page for what's shown, and the counts the filters show.
async function reload(v) {
  const { kind } = v;
  const mine = ++v.reloads;
  const stale = () => !v.ctx.current() || mine !== v.reloads;
  v.count.textContent = `Loading ${kind.many}…`;
  v.more.replaceChildren();
  const head = (extra) => api(`${kind.list}?${new URLSearchParams({ limit: 1, ...extra })}`).then((r) => r.total);
  const [page, off, wanted] = await Promise.all([
    api(`${kind.list}?${query(v, { limit: PAGE_SIZE, offset: 0 })}`),
    head({ monitored: "false" }),
    head({ wanted: "true" }),
  ]);
  if (stale()) return;
  v.items = page.items;
  v.total = page.total;
  v.selection.update();
  drawFilters(v, page.counts, { off, wanted });
  draw(v);
  const narrowed = Boolean((v.show && !v.fixed) || v.q || v.library);
  if (page.total === 0) {
    v.count.textContent = "";
    const none = v.fixed && !narrowed ? emptyState(`Nothing here: no ${kind.many} are ${v.fixed.name.toLowerCase()}.`, { href: `#/admin/${v.ctx.service.key}/library`, label: "Go to the Library" }) : null;
    v.list.replaceChildren(
      none ??
      narrowed
        ? emptyState(`No ${kind.many} match${v.q ? ` “${v.q}”` : ""}.`, { label: "Clear search and filters", onclick: () => clear(v) })
        : emptyState(`No ${kind.many} yet. They appear here once a library holds them or they're wanted.`, null),
    );
    return;
  }
  v.count.textContent = `${plural(page.total, kind.one, kind.many)}${narrowed ? " shown" : ""}.`;
  offerMore(v, stale);
}

function clear(v) {
  v.q = v.library = "";
  v.show = v.fixed?.show ?? "";
  v.search.value = "";
  if (v.libraryPick) v.libraryPick.value = "";
  again(v);
  v.search.focus();
}

// Show more (N left), loading the next page onto the list.
function offerMore(v, stale) {
  const left = v.total - v.items.length;
  if (left <= 0) return v.more.replaceChildren();
  const button = h("button", { type: "button", class: "quiet" }, `Show more (${number.format(left)} left)`);
  button.addEventListener("click", async () => {
    button.disabled = true;
    button.textContent = "Loading…";
    try {
      const next = await api(`${v.kind.list}?${query(v, { limit: PAGE_SIZE, offset: v.items.length })}`);
      if (stale()) return;
      const first = v.items.length;
      v.items.push(...next.items);
      draw(v);
      v.list.querySelectorAll("a, input")[first]?.focus();
      offerMore(v, stale);
    } catch (err) {
      if (err instanceof SignedOut) return v.ctx.signedOut();
      button.disabled = false;
      button.textContent = `Couldn't load more: ${err.message} Try again`;
    }
  });
  v.more.replaceChildren(button);
}

// The list as posters or a table, with a box to tick on each wanted title while selecting.
function draw(v) {
  if (!v.items) return;
  v.list.replaceChildren(v.view === "table" ? table(v) : posters(v));
}

// A box to tick on each title while selecting; one only in the library can be monitored, which
// puts it on the wanted list.
function tick(v, t) {
  if (!v.selecting) return null;
  if (t.wanted_id === null && folderOf(t) === null) return h("span", { class: "tick-none" });
  const key = selectKey(t);
  const box = h("input", { type: "checkbox", "aria-label": `Select ${t.title}`, checked: v.chosen.has(key) });
  box.addEventListener("change", () => {
    if (box.checked) v.chosen.set(key, t);
    else v.chosen.delete(key);
    v.selection.update();
  });
  return box;
}

// Select all shown: every title the filters show, page by page, not only those drawn. A change
// of filter meanwhile leaves the selection as it was.
async function selectAll(v) {
  const mine = v.reloads;
  const all = [];
  for (let offset = 0; ; offset += ALL_PAGE) {
    const page = await api(`${v.kind.list}?${query(v, { limit: ALL_PAGE, offset })}`);
    if (!v.ctx.current() || mine !== v.reloads) return;
    all.push(...page.items);
    if (page.items.length < ALL_PAGE || all.length >= page.total) break;
  }
  for (const t of all) if (t.wanted_id !== null || folderOf(t) !== null) v.chosen.set(selectKey(t), t);
  v.selection.update();
  draw(v);
}

// A title's name, linked to its page here.
function named(v, t, tag = "span") {
  const href = titleHref(v.ctx.service.key, t);
  const text = [clip(t.title, tag === "a" ? "span" : tag)];
  return href ? h("a", { href, class: "title-link" }, text) : h("span", { class: "title-link" }, text);
}

// Artists' pictures are square, as album covers are.
function artOf(v, t) {
  const box = art(t.poster);
  if (v.ctx.service.key === "music") box.classList.add("square");
  return box;
}

function posters(v) {
  return h(
    "ul",
    { class: "grid library-grid" },
    v.items.map((t) =>
      h(
        "li",
        { class: "library-card" },
        tick(v, t),
        artOf(v, t),
        named(v, t),
        t.year ? h("span", { class: "meta" }, String(t.year)) : null,
        stateOf(t),
      ),
    ),
  );
}

function table(v) {
  const music = v.ctx.service.key === "music";
  const heads = [v.selecting ? h("th", { scope: "col" }, h("span", { class: "visually-hidden" }, "Selected")) : null, "Title", music ? null : "Year", "State", "Monitored", "Profile", "Library", "Size"];
  const cell = (label, content, extra = {}) => h("td", { "data-label": label, ...extra }, content);
  return h(
    "div",
    { class: "table-wrap stacks" },
    h(
      "table",
      { class: "data" },
      h("caption", { class: "visually-hidden" }, `${v.ctx.service.name}: ${plural(v.total, v.kind.one, v.kind.many)}`),
      h("thead", {}, h("tr", {}, heads.filter(Boolean).map((x) => (typeof x === "string" ? h("th", { scope: "col", class: x === "Size" ? "r" : null }, x) : x)))),
      h(
        "tbody",
        {},
        v.items.map((t) =>
          h(
            "tr",
            {},
            v.selecting ? h("td", { class: "tick" }, tick(v, t)) : null,
            h("th", { scope: "row" }, named(v, t)),
            music ? null : cell("Year", t.year ? String(t.year) : "—", { class: "num" }),
            cell("State", stateOf(t)),
            cell("Monitored", !t.wanted ? "Not wanted" : t.monitored ? "Yes" : "No"),
            cell("Profile", t.profile ?? "—"),
            cell("Library", t.library ?? "—"),
            cell("Size", t.size ? size(t.size) : "—", { class: "r" }),
          ),
        ),
      ),
    ),
  );
}
