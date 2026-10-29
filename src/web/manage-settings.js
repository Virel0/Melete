// Requests and Settings in Manage's Movies, Series, Anime and Music: To decide and Where they
// go narrowed to the service; and, shown here and changed where they're shared, the indexers
// with what the service asks them for, the downloading path, and how files are handled.

import { group, when } from "/web/forms.js";
import { api, h, plural } from "/web/lib.js";
import { stateLabel } from "/web/page-parts.js";
import { decide, settings as whereTheyGo } from "/web/requests.js";

const REQUESTS = { movies: "movies", series: "series", anime: "anime", music: "music" };

export const toDecideFor = (ctx) => decide({ ...ctx, requestsService: REQUESTS[ctx.service.key] });
export const whereTheyGoFor = (ctx) => whereTheyGo({ ...ctx, requestsService: REQUESTS[ctx.service.key] });

// The Newznab category ranges each service searches indexers in.
const CATEGORIES = {
  movies: [2000, 2999, "movie"],
  series: [5000, 5999, "TV"],
  anime: [5000, 5999, "TV"],
  music: [3000, 3999, "audio"],
};

// Indexers: the shared list, read-only, with what this service asks each for and its last answer.
export async function indexersFor(ctx) {
  const [all, syncs] = await Promise.all([api("/indexers"), api("/rss/syncs?limit=1")]);
  if (!ctx.current()) return [];
  const [low, high, what] = CATEGORIES[ctx.service.key];
  const last = new Map((syncs[0]?.indexers ?? []).map((i) => [i.indexer_id, i]));
  const cell = (label, content) => h("td", { "data-label": label }, content);
  const rows = all.map((ix) => {
    const asked = ix.categories.filter((c) => c >= low && c <= high);
    const seen = last.get(ix.id);
    const answer = !ix.enabled
      ? stateLabel("off", "Turned off")
      : seen?.error
        ? stateLabel("failed", seen.error)
        : seen
          ? stateLabel("have", `${plural(seen.results, "release", "releases")}, ${when(syncs[0].synced_at)}`)
          : stateLabel("off", "Not in an RSS sync yet");
    return h(
      "tr",
      {},
      h("th", { scope: "row" }, ix.name),
      cell("On", ix.enabled ? "On" : "Off"),
      cell("Asked for", asked.length ? asked.join(", ") : `Not asked: it has no ${what} categories`),
      cell("Last answer", answer),
    );
  });
  return [
    group(
      "Indexers",
      h("p", { class: "note" }, `Indexers are shared by every service, so they're added and changed in one place. This is what ${ctx.service.name} asks them for: their ${what} categories.`),
      h("p", {}, h("a", { class: "button", href: "#/admin/indexers/list" }, "Manage indexers")),
      all.length
        ? h(
            "div",
            { class: "table-wrap stacks" },
            h("table", { class: "data" }, h("caption", { class: "visually-hidden" }, "Indexers"), h("thead", {}, h("tr", {}, ["Indexer", "On", "Asked for", "Last answer"].map((c) => h("th", { scope: "col" }, c)))), h("tbody", {}, rows)),
          )
        : h("p", { class: "status" }, "No indexers yet, so nothing can be searched for."),
    ),
  ];
}

// Downloading: how finished downloads reach the library, read-only, from the outside services.
export async function downloadingFor(ctx) {
  const all = await api("/settings/services");
  if (!ctx.current()) return [];
  const keys = ctx.service.key === "music" ? ["downloads", "seedbox", "slskd"] : ["downloads", "seedbox"];
  const shown = keys.map((k) => all.find((s) => s.key === k)).filter(Boolean);
  return [
    group(
      "Downloading",
      h("p", { class: "note" }, "Shared by every service and set under System. Torrents go to Deluge on the seedbox and are copied home over SFTP; music can also come through slskd."),
      h(
        "ul",
        { class: "rows" },
        shown.map((s) => h("li", {}, h("div", {}, h("strong", {}, s.name), " ", s.on ? stateLabel("have", "On") : stateLabel("off", "Off"), h("p", { class: "note" }, s.about)))),
      ),
      h("p", {}, h("a", { href: "#/admin/system/downloading" }, "Change them under System, Downloading")),
    ),
  ];
}

// How files are handled: the fixed rules, stated, so nobody looks for a rename switch.
export function filesFor() {
  const facts = [
    "Files already in the library are never renamed, moved or deleted.",
    "A new file is copied home into the title's folder (a new “Title (Year)” folder when there's none; an episode beside its season's files), written under a hidden name, then put in place without replacing anything.",
    "When something is already at a file's name, the import stops rather than overwrite it.",
    "An upgrade moves the file it replaces to its library's .melete-trash folder, once the new one is in place, scanned and plays as long; it's deleted 30 days later, and can be put back until then under Activity, Replaced files.",
    "Only videos and subtitles are copied, not samples or info files; new files can be read and written by the group.",
  ];
  return [group("How files are handled", h("p", { class: "note" }, "These are fixed rules, not settings."), h("ul", { class: "plain facts" }, facts.map((f) => h("li", {}, f))))];
}
