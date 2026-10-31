// A title's history on its Manage page: what happened to it, newest first (searched, grabbed,
// imported, failed, upgraded, taken off, blocklisted, an extra found, a file renamed or set aside
// as the wrong one, removed from Deluge once imported), and its searches still kept, each of whose
// results can be shown again without searching.

import { when } from "/web/forms.js";
import { SignedOut, api, append, h, number, plural, seasonName } from "/web/lib.js";
import { emptyState, failed, placeholders, stateLabel } from "/web/page-parts.js";
import { results } from "/web/manage-search.js";

// Each kind of event in words, with the state mark it's drawn with.
export const EVENTS = {
  searched: ["Searched", "off"],
  grabbed: ["Grabbed", "coming"],
  imported: ["Imported", "have"],
  failed: ["Failed", "failed"],
  upgraded: ["Replaced by an upgrade", "have"],
  taken_off: ["Taken off the list", "off"],
  blocklisted: ["Blocklisted", "failed"],
  extra_found: ["Extra found", "have"],
  renamed: ["Renamed or replaced", "off"],
  set_aside: ["Set aside as the wrong file", "failed"],
  removed_from_deluge: ["Removed from Deluge", "off"],
};

const PAGE = 25;

// What it was about, for a series: a season or an episode.
export function part(e) {
  if (e.season === null) return null;
  return e.episode === null ? seasonName(e.season) : `${seasonName(e.season)}, episode ${e.episode}`;
}

// One event as a table row.
export function eventRow(e, { withTitle = false } = {}) {
  const [words, mark] = EVENTS[e.kind] ?? [e.kind, "off"];
  const cell = (label, content) => h("td", { "data-label": label }, content);
  return h(
    "tr",
    {},
    h("th", { scope: "row" }, stateLabel(mark, words)),
    cell("When", when(e.at)),
    withTitle ? cell("Title", e.title) : null,
    cell("What", [part(e), e.release].filter(Boolean).join(": ") || "—"),
    cell("Word on it", e.message ?? "—"),
  );
}

// The history part of a title's page: `service` is movies, series, anime or artists.
export function historyPart(ctx, service, wantedId) {
  const box = h("div", {}, placeholders("history", { kind: "rows", count: 3 }));
  const searches = h("div", {});
  const load = async () => {
    const [page, past] = await Promise.all([
      api(`/manage/${service}/history?${new URLSearchParams({ wanted_id: wantedId, limit: PAGE })}`),
      api(`/manage/${service}/searches?wanted_id=${wantedId}`),
    ]);
    if (!ctx.current()) return;
    box.replaceChildren();
    append(box, [
      page.items.length
        ? h(
            "div",
            { class: "table-wrap stacks" },
            h(
              "table",
              { class: "data" },
              h("caption", { class: "visually-hidden" }, "History, newest first"),
              h("thead", {}, h("tr", {}, ["What happened", "When", "What", "Word on it"].map((c) => h("th", { scope: "col" }, c)))),
              h("tbody", {}, page.items.map((e) => eventRow(e))),
            ),
          )
        : emptyState("Nothing has happened to it yet: searches, grabs, imports and upgrades show here.", null),
      page.total > page.items.length ? h("p", { class: "note" }, `The newest ${number.format(page.items.length)} of ${number.format(page.total)}; the rest are under Activity, History.`) : null,
    ]);
    searches.replaceChildren(...pastSearches(ctx, past));
  };
  const tryLoad = () =>
    load().catch((err) => {
      if (err instanceof SignedOut) return ctx.signedOut();
      box.replaceChildren(failed("its history", err, tryLoad));
    });
  tryLoad();
  return [h("h2", { class: "part" }, "History"), box, searches];
}

// Searches kept 30 days, each with Show its results.
function pastSearches(ctx, past) {
  if (!past.length) return [];
  return [
    h("h3", { class: "part-sub" }, "Searches kept"),
    h(
      "ul",
      { class: "rows" },
      past.map((s) => {
        const shown = h("div", {});
        const show = h("button", { type: "button", class: "quiet", "aria-label": `Show the results of the ${s.rss ? "RSS sync" : "search"} of ${when(s.searched_at)}` }, "Show its results");
        const what = [s.album, s.season === null ? null : s.episode === null ? seasonName(s.season) : `${seasonName(s.season)}, episode ${s.episode}`].filter(Boolean).join(", ");
        show.addEventListener("click", async () => {
          show.disabled = true;
          try {
            const search = await api(`/searches/${s.id}`);
            shown.replaceChildren(results(ctx, search, what || "it"));
            show.hidden = true;
          } catch (err) {
            if (err instanceof SignedOut) return ctx.signedOut();
            shown.replaceChildren(h("p", { class: "error" }, err.message));
            show.disabled = false;
          }
        });
        return h(
          "li",
          {},
          h(
            "div",
            {},
            h("strong", {}, s.rss ? "RSS sync" : "Search"),
            ` ${when(s.searched_at)}${what ? `, ${what}` : ""}: ${plural(s.results, "result", "results")}, ${number.format(s.accepted)} accepted.`,
            shown,
          ),
          h("div", { class: "row-actions" }, show),
        );
      }),
    ),
  ];
}
