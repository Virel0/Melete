// Listen's lists: searchable, sortable and paged with "Show more", like the movie and series
// lists. The URL keeps the search, sort, filters and how many were shown, so Back returns to
// the same place.
//
// pagedList(ctx, { title, base, noun, sorts, defaultSort, path, item, list, filters, intro })
// draws one: `path` is the API list it pages through, `item(entry)` makes one entry of `list`,
// `filters` are selects ({ key, label, options }) sent as query parameters, and `intro` goes
// under the heading.

import { SignedOut, announce, api, append, failure, h, number, plural, show } from "/web/lib.js";

export const PAGE_SIZE = 60;

export function select(id, options, chosen) {
  return h(
    "select",
    { id },
    options.map(([value, name]) => h("option", { value, selected: value === chosen }, name)),
  );
}

// ---------- Paged lists ----------

// A searchable, sortable list paged with "Show more", like the movie and series lists.
// `load(query)` gets a page; `item` makes one entry; the URL keeps the search, sort, filter and
// how many were shown, so Back returns to the same place.
export async function pagedList(ctx, { title, base, noun, sorts, defaultSort, path, item, list, filters = [], intro = null }) {
  const params = ctx.params;
  const valid = (key) => (sorts.some(([k]) => k === key) ? key : defaultSort);
  // What the list's parts share: what it lists, what's shown, and the boxes it's shown in.
  const v = {
    ctx,
    base,
    noun,
    defaultSort,
    path,
    item,
    list,
    filters,
    valid,
    q: params.get("q") ?? "",
    sort: valid(params.get("sort")),
    chosen: Object.fromEntries(filters.map((f) => [f.key, params.get(f.key) ?? ""])),
    shown: Math.min(Math.max(Number(params.get("shown")) || PAGE_SIZE, PAGE_SIZE), 600),
    reloads: 0,
  };
  v.search = h("input", { id: "search", type: "search", value: v.q, autocomplete: "off", spellcheck: "false" });
  const sorter = select("sort", sorts, v.sort);
  v.filterSelects = filters.map((f) => select(f.key, f.options, v.chosen[f.key]));
  v.count = h("p", { class: "status", "aria-live": "polite" });
  v.more = h("div", { class: "more" });
  listen(v, sorter);

  show(
    title,
    h("h1", {}, title),
    intro,
    h(
      "div",
      { class: "toolbar" },
      h("div", {}, h("label", { for: "search" }, `Search ${noun[1]}`), v.search),
      // One way to sort is no choice.
      sorts.length > 1 ? h("div", {}, h("label", { for: "sort" }, "Sort by"), sorter) : null,
      filters.map((f, i) => h("div", {}, h("label", { for: f.key }, f.label), v.filterSelects[i])),
    ),
    v.count,
    list,
    v.more,
  );
  await reload(v);
}

// Typing a search, sorting and filtering each start the list again from the top.
function listen(v, sorter) {
  const { ctx, search, filters, filterSelects } = v;
  let typing;
  search.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(() => {
      if (!ctx.current() || search.value.trim() === v.q) return;
      v.q = search.value.trim();
      v.shown = PAGE_SIZE;
      remember(v);
      reload(v).catch((err) => failed(v, err));
    }, 300);
  });
  sorter.addEventListener("change", () => {
    v.sort = v.valid(sorter.value);
    v.shown = PAGE_SIZE;
    remember(v);
    reload(v).catch((err) => failed(v, err));
  });
  for (const [i, f] of filters.entries()) {
    filterSelects[i].addEventListener("change", () => {
      v.chosen[f.key] = filterSelects[i].value;
      v.shown = PAGE_SIZE;
      remember(v);
      reload(v).catch((err) => failed(v, err));
    });
  }
}

// The URL says what's shown without adding history entries.
function remember(v) {
  const next = new URLSearchParams();
  if (v.q) next.set("q", v.q);
  if (v.sort !== v.defaultSort) next.set("sort", v.sort);
  for (const f of v.filters) if (v.chosen[f.key]) next.set(f.key, v.chosen[f.key]);
  if (v.shown > PAGE_SIZE) next.set("shown", v.shown);
  history.replaceState(history.state, "", `${v.base}${next.toString() ? `?${next}` : ""}`);
}

// Loads the first `shown` entries for the search, sort and filters, replacing what's there.
async function reload(v) {
  const { ctx, noun, filters, chosen, list, count, more } = v;
  const mine = ++v.reloads;
  const stale = () => !ctx.current() || mine !== v.reloads;
  count.textContent = `Loading ${noun[1]}…`;
  more.replaceChildren();
  const query = new URLSearchParams({ sort: v.sort });
  if (v.q) query.set("q", v.q);
  for (const f of filters) if (chosen[f.key]) query.set(f.key, chosen[f.key]);
  const load = (offset, limit) => {
    query.set("offset", offset);
    query.set("limit", limit);
    return api(`${v.path}?${query}`);
  };
  const page = await load(0, v.shown);
  if (stale()) return;
  list.replaceChildren(...page.items.map(v.item));
  if (page.total === 0) {
    const narrowed = v.q || filters.some((f) => chosen[f.key]);
    count.textContent = narrowed ? `No ${noun[1]} match.` : `No ${noun[1]} yet. They show up here once a music library is scanned.`;
    if (narrowed) {
      const clear = () => {
        v.search.value = v.q = "";
        for (const [i, f] of filters.entries()) v.filterSelects[i].value = chosen[f.key] = "";
        remember(v);
        v.search.focus();
        reload(v).catch((err) => failed(v, err));
      };
      more.replaceChildren(h("button", { type: "button", class: "quiet", onclick: clear }, "Clear search and filters"));
    }
    return;
  }
  const total = plural(page.total, noun[0], noun[1]);
  count.textContent = v.q ? `${total} match “${v.q}”` : total;
  offerMore(v, { page, load, stale, loaded: page.items.length });
}

// Show more (N left), loading the next page into the list when pressed.
function offerMore(v, got) {
  const { ctx, item, list, more } = v;
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
      const entries = next.items.map(item);
      append(list, entries);
      got.loaded += next.items.length;
      v.shown = Math.min(got.loaded, 600);
      remember(v);
      offerMore(v, got);
      // Keep keyboard users where they were: on the first new entry.
      entries[0]?.querySelector("a")?.focus();
      announce(`Showing ${number.format(got.loaded)} of ${number.format(page.total)}`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      more.replaceChildren(failure(err, () => offerMore(v, got)));
    }
  });
  more.replaceChildren(button);
}

// A reload that failed: back to sign-in when signed out, otherwise Try again in the list.
function failed(v, err) {
  if (err instanceof SignedOut) return v.ctx.signedOut();
  v.count.textContent = "";
  v.more.replaceChildren(failure(err, () => reload(v).catch((e) => failed(v, e))));
}

// A Genre filter listing what the library has, counted in `counted` ("albums" or "tracks");
// none when the genres can't be read, and the list still works without it.
export async function genreFilter(ctx, counted) {
  const genres = await api("/music/genres").catch((err) => {
    if (err instanceof SignedOut) throw err;
    return [];
  });
  if (!genres.length) return [];
  const options = [["", "All genres"], ...genres.map((g) => [g.name, `${g.name} (${number.format(g[counted])})`])];
  return [{ key: "genre", label: "Genre", options }];
}
