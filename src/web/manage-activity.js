// Calendar, Activity (Queue, History, Replaced files) and Wanted (Missing, Cutoff unmet, Taken
// off) in Manage's Movies, Series, Anime and Music: the shared pages narrowed to the service's
// own titles, and its history and Wanted lists from its managed list.

import { group } from "/web/forms.js";
import { SignedOut, airDate, api, h, number, plural } from "/web/lib.js";
import { emptyState, failed, placeholders, stateLabel } from "/web/page-parts.js";
import { calendarPage } from "/web/calendar.js";
import { downloads } from "/web/wanted-downloads.js";
import { removedTitles, trash } from "/web/wanted-set-aside.js";
import { EVENTS, eventRow } from "/web/manage-history.js";
import { libraryPage } from "/web/manage-library.js";

// The wanted ids of every anime series, to tell anime's downloads and dates from the rest.
async function animeIds() {
  const ids = new Set();
  for (let offset = 0; ; offset += 500) {
    const page = await api(`/manage/anime?${new URLSearchParams({ wanted: "true", limit: 500, offset })}`);
    for (const t of page.items) ids.add(t.wanted_id);
    if (offset + page.items.length >= page.total) return ids;
  }
}

// Whether a wanted series belongs to this service: Series or Anime, by `anime`.
const seriesOf = (key, anime) => (id) => id !== null && (key === "anime") === anime.has(id);

// ---------- Calendar ----------

export async function calendarFor(ctx) {
  const key = ctx.service.key;
  if (key === "music") return musicCalendar(ctx);
  const anime = key === "movies" ? new Set() : await animeIds();
  const keep = key === "movies" ? (e) => e.wanted_movie_id !== null : (e) => seriesOf(key, anime)(e.wanted_series_id);
  const href = (e) => (e.wanted_movie_id ? `#/admin/movies/title/${e.wanted_movie_id}` : e.wanted_series_id ? `#/admin/${key}/title/${e.wanted_series_id}` : null);
  let parts = [];
  await calendarPage(ctx.params, ctx, { base: `#/admin/${key}/calendar`, keep, shows: false, href, render: (_title, ...nodes) => (parts = nodes) });
  return parts;
}

// Music's calendar: wanted albums by their release dates, the last month's and what's coming.
async function musicCalendar(ctx) {
  const artists = (await api("/wanted/artists")).filter((a) => a.monitored);
  const lists = await Promise.all(artists.map((a) => api(`/wanted/artists/${a.id}/albums`).then((albums) => albums.map((al) => ({ ...al, artist: a })))));
  if (!ctx.current()) return [];
  const since = new Date(Date.now() - 31 * 86_400_000).toISOString().slice(0, 10);
  const today = new Date().toISOString().slice(0, 10);
  const albums = lists
    .flat()
    .filter((a) => a.monitored && a.release_date && a.release_date.padEnd(10, "-01").slice(0, 10) >= since)
    .sort((a, b) => a.release_date.localeCompare(b.release_date));
  const day = (d) => (d.length === 10 ? airDate(d) : d);
  const cell = (label, content) => h("td", { "data-label": label }, content);
  return [
    h("h1", {}, "Calendar"),
    h("p", { class: "note" }, "Wanted albums of monitored artists out in the last month or coming, by the release dates MusicBrainz gives."),
    albums.length
      ? h(
          "div",
          { class: "table-wrap stacks" },
          h(
            "table",
            { class: "data" },
            h("caption", { class: "visually-hidden" }, "Albums by release date"),
            h("thead", {}, h("tr", {}, ["Album", "Out", "Artist", "State"].map((c) => h("th", { scope: "col" }, c)))),
            h(
              "tbody",
              {},
              albums.map((a) =>
                h(
                  "tr",
                  {},
                  h("th", { scope: "row" }, h("a", { href: `#/admin/music/album/${a.id}?artist=${a.artist.id}` }, a.title)),
                  cell("Out", day(a.release_date)),
                  cell("Artist", h("a", { href: `#/admin/music/artist/${a.artist.id}` }, a.artist.name)),
                  cell("State", a.album_id ? stateLabel("have", "In the library") : a.release_date > today ? stateLabel("notyet", "Not out yet") : stateLabel("missing", "Missing")),
                ),
              ),
            ),
          ),
        )
      : emptyState("No wanted album came out in the last month or is coming.", null),
  ];
}

// ---------- Activity ----------

// Queue: the downloads page, narrowed to the service (Series and Anime apart).
export async function queueFor(ctx) {
  const key = ctx.service.key;
  if (key === "series" || key === "anime") {
    const anime = await animeIds();
    return downloads({ ...ctx, kind: "series", downloadsOnly: (d) => seriesOf(key, anime)(d.wanted_series_id) });
  }
  return downloads({ ...ctx, kind: key });
}

// Replaced files: the trash, narrowed to the service's libraries.
export function replacedFor(ctx) {
  const key = ctx.service.key;
  const only = {
    movies: (l) => l?.kind === "movies",
    series: (l) => l?.kind === "shows" && l.category !== "anime",
    anime: (l) => l?.kind === "shows" && l.category === "anime",
    music: (l) => l?.kind === "music",
  }[key];
  return trash({ ...ctx, trashOnly: only });
}

const HISTORY_PAGE = 50;

// History: everything that happened to the service's titles, newest first, by kind.
export async function historyFor(ctx) {
  const service = ctx.service.key === "music" ? "artists" : ctx.service.key;
  const kind = Object.hasOwn(EVENTS, ctx.params.get("kind") ?? "") ? ctx.params.get("kind") : "";
  const pick = h("select", { id: "history-kind" }, h("option", { value: "" }, "Everything"), Object.entries(EVENTS).map(([k, [words]]) => h("option", { value: k, selected: k === kind }, words)));
  pick.addEventListener("change", () => {
    location.hash = `#/admin/${ctx.service.key}/history${pick.value ? `?kind=${pick.value}` : ""}`;
  });
  const box = h("div", {}, placeholders("history", { kind: "rows" }));
  const more = h("div", { class: "more" });
  const items = [];
  const load = async () => {
    const q = new URLSearchParams({ limit: HISTORY_PAGE, offset: items.length });
    if (kind) q.set("kind", kind);
    const page = await api(`/manage/${service}/history?${q}`);
    if (!ctx.current()) return;
    items.push(...page.items);
    box.replaceChildren(table(items));
    const left = page.total - items.length;
    more.replaceChildren(left > 0 ? h("button", { type: "button", class: "quiet", onclick: () => load().catch(failing) }, `Show more (${number.format(left)} left)`) : "");
    if (!items.length) box.replaceChildren(emptyState(kind ? "Nothing of that kind has happened in the last year." : "Nothing has happened to these titles in the last year.", null));
  };
  const failing = (err) => {
    if (err instanceof SignedOut) return ctx.signedOut();
    box.replaceChildren(failed("the history", err, () => load().catch(failing)));
  };
  load().catch(failing);
  return [
    group(
      "History",
      h("p", { class: "note" }, "Searched, grabbed, imported, failed, replaced by an upgrade, taken off, blocklisted, extras found and files renamed, kept a year."),
      h("div", { class: "field" }, h("label", { for: "history-kind" }, "Show"), pick),
      box,
      more,
    ),
  ];
}

function table(items) {
  return h(
    "div",
    { class: "table-wrap stacks" },
    h(
      "table",
      { class: "data" },
      h("caption", { class: "visually-hidden" }, `History, newest first: ${plural(items.length, "event", "events")}`),
      h("thead", {}, h("tr", {}, ["What happened", "When", "Title", "What", "Word on it"].map((c) => h("th", { scope: "col" }, c)))),
      h("tbody", {}, items.map((e) => eventRow(e, { withTitle: true }))),
    ),
  );
}

// ---------- Wanted ----------

// Missing and Cutoff unmet: the Library narrowed to them, as a table, with their run open.
export const missingFor = (ctx) => libraryPage({ ...ctx, fixed: { key: "missing", name: "Missing", show: "missing", run: "missing" } });
export const cutoffFor = (ctx) => libraryPage({ ...ctx, fixed: { key: "cutoff", name: "Cutoff unmet", show: "cutoff", run: "cutoff" } });

// Taken off: titles taken off the wanted list, which imports leave off until allowed back.
export function takenOffFor(ctx) {
  const area = { movies: "movies", series: "series", anime: "series", music: "music" }[ctx.service.key];
  return removedTitles(ctx, area);
}
