// Settings: your account for everyone (account.js); for admins, users (users.js), libraries,
// matching titles by hand, and where releases and wanted titles come from (sources.js).
// Sections sit in a side list, each one a group of small forms, so nothing is more than one
// click from the list.

import { confirmInline, group, sectioned, when } from "/web/forms.js";
import { SignedOut, announce, api, append, h, loading, number, plural } from "/web/lib.js";
import { account } from "/web/account.js";
import { diagnostics } from "/web/diagnostics.js";
import { services } from "/web/services.js";
import { arrs, indexers, lists, profiles } from "/web/sources.js";
import { users } from "/web/users.js";
import { titleRescan } from "/web/library-rescan.js";

const SECTIONS = [
  { key: "account", name: "Your account", admin: false, page: account },
  { key: "users", name: "Users", admin: true, page: users },
  { key: "libraries", name: "Libraries", admin: true, page: libraries },
  { key: "services", name: "Outside services", admin: true, page: services },
  { key: "matching", name: "Matching", admin: true, page: matching },
  { key: "indexers", name: "Indexers", admin: true, page: indexers },
  { key: "profiles", name: "Quality profiles", admin: true, page: profiles },
  { key: "lists", name: "Import lists", admin: true, page: lists },
  { key: "arrs", name: "Sonarr and Radarr", admin: true, page: arrs },
  { key: "diagnostics", name: "Diagnostics", admin: true, page: diagnostics },
];

// ---------- Libraries ----------

export async function libraries(ctx) {
  const all = await api("/libraries");
  if (!ctx.current()) return [];
  const status = h("p", { class: "form-result", role: "status" });
  // Asks for a scan, of every library or of one, and says so in the status line.
  const scanButton = (label, path, said) => {
    const button = h("button", { type: "button", class: "quiet" }, label);
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await api(path, { method: "POST" });
        status.textContent = `${said} It also matches new titles and reads their files; reload this page to see the counts change.`;
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        status.textContent = err.message;
      } finally {
        button.disabled = false;
      }
    });
    return button;
  };
  const scan = scanButton("Scan all now", "/libraries/scan", "Scan of every library started.");
  scan.classList.remove("quiet");
  const rows = all.map((l) =>
    h(
      "li",
      {},
      h(
        "div",
        {},
        h("strong", {}, l.name),
        " ",
        h("span", { class: "badge" }, { movies: "Movies", shows: "Series", music: "Music" }[l.kind] ?? l.kind),
        l.category ? [" ", h("span", { class: "badge" }, { anime: "Anime", kids: "Kids" }[l.category] ?? l.category)] : null,
        h("p", { class: "note path" }, l.path),
        h(
          "p",
          {},
          [
            plural(l.titles, "title", "titles"),
            plural(l.files, "file", "files"),
            l.missing_files ? `${number.format(l.missing_files)} missing` : null,
            l.unparsed_files ? `${number.format(l.unparsed_files)} without an episode number` : null,
          ]
            .filter(Boolean)
            .join(", ") + ".",
        ),
        h("p", { class: "note" }, `Last scanned ${when(l.last_scan_at)}.`),
        scanButton(`Scan ${l.name} now`, `/libraries/${encodeURIComponent(l.name)}/scan`, `Scan of ${l.name} started.`),
      ),
    ),
  );
  return [
    group(
      "Libraries",
      h(
        "p",
        { class: "note" },
        "Set only in .env, one MELETE_LIBRARY_ variable each, since Melete needs them before it starts: add or change one there and restart Melete. They're scanned whole at startup and on the scan schedule (Outside services, Library scans; every 6 hours unless set), and what Melete imports is picked up at once from its own folder. A folder holding a .ignore or .melete-ignore file is skipped. Nothing in them is renamed or moved.",
      ),
      all.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "No libraries are set up."),
      h("div", { class: "form-actions" }, scan, status),
    ),
    all.length ? titleRescan(ctx) : null,
  ];
}

// ---------- Matching ----------

const MATCH_PAGE = 50;

// What the list says once every title is settled.
const empty = () => h("p", { class: "status" }, "Every title is matched to TMDB, or decided by hand.");

// `ctx.matchOnly(item)`, when given, narrows the list to one Manage service's titles.
export async function matching(ctx) {
  const all = (await api("/metadata/unmatched")).filter(ctx.matchOnly ?? (() => true));
  if (!ctx.current()) return [];
  const list = h("ul", { class: "rows matching" });
  if (!all.length) return [group("Titles to match", empty())];
  const page = { ctx, list };

  // A first run without TMDB can leave thousands; they're shown a page at a time.
  const more = h("div", { class: "more" });
  let shown = 0;
  const showMore = () => {
    const next = all.slice(shown, shown + MATCH_PAGE);
    const rows = next.map((item) => matchRow(page, item));
    list.append(...rows);
    shown += next.length;
    const left = all.length - shown;
    more.replaceChildren(left > 0 ? h("button", { type: "button", class: "quiet", onclick: () => {
      showMore();
      rows.at(-1)?.nextElementSibling?.querySelector("button, input")?.focus();
    } }, `Show more (${number.format(left)} left)`) : "");
  };
  showMore();
  return [
    group(
      "Titles to match",
      h("p", { class: "note" }, `${plural(all.length, "title", "titles")} TMDB couldn't settle. A match made here is final: automatic matching leaves it alone.`),
      list,
      more,
    ),
  ];
}

// Takes a settled title's row away, says so, and moves focus to the row after it.
function settle(page, li, message) {
  // One title matched from its own page: that page says so its own way.
  if (page.done) return page.done(message);
  const { list } = page;
  const next = li.nextElementSibling ?? li.previousElementSibling;
  li.remove();
  announce(message);
  if (!list.children.length) list.replaceWith(empty());
  else next?.querySelector("button, input")?.focus();
}

// How each title stands with TMDB, as a row says it.
const MATCH_WORDS = {
  ambiguous: "TMDB has more than one title it could be.",
  unmatched: "TMDB found nothing for the name.",
  matched: "Matched to TMDB; choose another entry to change it.",
  null: "Not looked up on TMDB yet.",
};

// Matching one title from its own page: its candidates, a TMDB search and "It has no TMDB
// entry", as a row of Matching has them; `done(message)` once it's settled.
export function fixMatch(ctx, item, done) {
  return matchRow({ ctx, list: null, done }, item);
}

// Matches a title to a TMDB entry by hand.
async function match(page, item, li, tmdbId, label) {
  await api(`/${item.kind === "movie" ? "movies" : "series"}/${item.id}/match`, { method: "PUT", body: { tmdb_id: tmdbId } });
  settle(page, li, `Matched ${item.rel_path} to ${label}`);
}

// One title to match: what TMDB offered, a search of its own, and It has no TMDB entry.
function matchRow(page, item) {
  const li = h("li", {});
  const results = h("div", { class: "results" });
  const search = tmdbSearch(page, item, li, results);
  const actions = h("div", { class: "row-actions" });
  actions.append(
    h(
      "button",
      {
        type: "button",
        class: "quiet",
        onclick: () =>
          confirmInline(actions, {
            question: "Record that TMDB has no entry for it? Melete stops trying to match it; matching it by hand still works later.",
            confirm: "No TMDB entry",
            run: async () => {
              await api(`/${item.kind === "movie" ? "movies" : "series"}/${item.id}/match`, { method: "DELETE" });
              settle(page, li, `${item.rel_path} is marked as having no TMDB entry`);
            },
          }),
      },
      "It has no TMDB entry",
    ),
  );
  append(li, [
    h("div", {},
      h("strong", { class: "path" }, item.rel_path),
      " ",
      h("span", { class: "badge" }, item.kind === "movie" ? "Movie" : "Series"),
      h("p", { class: "note" }, `${item.library}. ${MATCH_WORDS[item.status] ?? MATCH_WORDS.unmatched}`),
      item.candidates.length ? choices(page, item, li, results, item.candidates) : null,
      search,
      results,
    ),
    actions,
  ]);
  return li;
}

// A Match to … button for each TMDB entry the title could be.
function choices(page, item, li, results, candidates) {
  return h(
    "ul",
    { class: "choices" },
    candidates.map((c) => {
      const label = c.year ? `${c.title} (${c.year})` : c.title;
      const button = h("button", { type: "button", class: "quiet" }, `Match to ${label}`);
      button.addEventListener("click", async () => {
        button.disabled = true;
        try {
          await match(page, item, li, c.tmdb_id, label);
        } catch (err) {
          if (err instanceof SignedOut) return page.ctx.signedOut();
          button.disabled = false;
          results.prepend(h("p", { class: "error" }, err.message));
        }
      });
      return h("li", {}, button, c.original_title && c.original_title !== c.title ? h("span", { class: "note" }, ` ${c.original_title}`) : null);
    }),
  );
}

// Searching TMDB by title and year, with what's found offered in `results`.
function tmdbSearch(page, item, li, results) {
  const id = `${item.kind}-${item.id}`;
  const query = h("input", { id: `q-${id}`, type: "search", value: item.parsed_title, autocomplete: "off" });
  const year = h("input", { id: `y-${id}`, inputmode: "numeric", value: item.parsed_year ?? "", class: "year" });
  const search = h("form", { class: "search-row", novalidate: true },
    h("div", {}, h("label", { for: `q-${id}` }, "Title"), query),
    h("div", {}, h("label", { for: `y-${id}` }, "Year"), year),
    h("button", { type: "submit", class: "quiet" }, "Search TMDB"),
  );
  search.addEventListener("submit", async (event) => {
    event.preventDefault();
    const q = query.value.trim();
    if (!q) return query.focus();
    results.replaceChildren(loading("TMDB's results"));
    const params = new URLSearchParams({ kind: item.kind, q });
    if (/^\d{4}$/.test(year.value.trim())) params.set("year", year.value.trim());
    try {
      const found = await api(`/metadata/search?${params}`);
      results.replaceChildren(found.length ? choices(page, item, li, results, found) : h("p", { class: "status" }, `TMDB has nothing called “${q}”. Try another spelling, or without the year.`));
    } catch (err) {
      if (err instanceof SignedOut) return page.ctx.signedOut();
      results.replaceChildren(h("p", { class: "error" }, err.message));
    }
  });
  return search;
}

// ---------- The page ----------

export function settingsPage(section, ctx) {
  return sectioned({ title: "Settings", base: "#/settings", sections: SECTIONS }, section, ctx);
}
