// Manage: one place for running Melete, split into services the way the apps it replaces
// were split: Movies, Series, Anime, Music, Indexers and System (manage-services.js lists them
// and their pages). Each service has the same kind of pages in the same order (its first page,
// then what's happening, what's wanted, and its settings), so learning one teaches the rest.
// The header lists the services with what needs attention in each, and the sidebar a service's
// pages (shell.js). This file draws the pages.

import { group, onSignedOut } from "/web/forms.js";
import { SignedOut, append, h, show } from "/web/lib.js";
import { failed, placeholders } from "/web/page-parts.js";
import { currentHealth } from "/web/health.js";
import { placeOf } from "/web/manage-services.js";
import { activity } from "/web/progress.js";
import { decide, imports, settings as requestSettings } from "/web/requests.js";
import { libraries, matching } from "/web/settings.js";
import { users } from "/web/users.js";
import { services } from "/web/services.js";
import { diagnostics } from "/web/diagnostics.js";
import { indexerAdd, indexerList, soulseek } from "/web/indexers.js";
import { releaseTry } from "/web/release-try.js";
import { lists } from "/web/sources-lists.js";
import { profiles } from "/web/sources-profiles.js";
import { rss } from "/web/wanted-rss.js";
import { blocklist, trash } from "/web/wanted-set-aside.js";
import { wantedSongs } from "/web/wanted-music.js";
import { musicFollowing, musicProfiles } from "/web/admin-music.js";
import { addPage, needsMatch } from "/web/manage-add.js";
import { historyFor, queueFor, replacedFor, takenOffFor } from "/web/manage-activity.js";
import { downloadingFor, filesFor, indexersFor, toDecideFor, whereTheyGoFor } from "/web/manage-settings.js";
import { MANAGE, managePage } from "/web/manage.js";
import { requestsOverview } from "/web/admin-overviews.js";
import { systemStatus } from "/web/system-status.js";
import { downloading, systemBackups, systemTasks } from "/web/system-upkeep.js";

// What every service has the same way: Add new, Needs a match, its Activity, Taken off, its
// Requests and the settings it shows from where they're shared.
const SERVICE_PAGES = {
  add: addPage,
  match: needsMatch,
  activity: queueFor,
  history: historyFor,
  replaced: replacedFor,
  removed: takenOffFor,
  decide: toDecideFor,
  defaults: whereTheyGoFor,
  indexers: indexersFor,
  downloading: downloadingFor,
  files: filesFor,
};
// A video service's profiles, custom formats and import lists (Radarr's and Sonarr's whole lists
// are imported under Add new).
const VIDEO_PAGES = {
  ...SERVICE_PAGES,
  profiles: (ctx) => profiles({ ...ctx, only: "profiles" }),
  formats: (ctx) => profiles({ ...ctx, only: "formats" }),
  sources: lists,
};

// Each area's pages by key; manage-services.js names them and lists them in each service's
// sidebar. The redesigned pages are in manage.js's MANAGE, found first.
const PAGES = {
  movies: VIDEO_PAGES,
  series: VIDEO_PAGES,
  anime: VIDEO_PAGES,
  music: { ...SERVICE_PAGES, following: musicFollowing, songs: wantedSongs, profiles: musicProfiles, imports },
  indexers: {
    list: indexerList,
    add: indexerAdd,
    soulseek,
    activity: rss,
    blocklist,
    try: releaseTry,
  },
  requests: {
    overview: async (ctx) => requestsOverview(ctx, attention("requests", "Requests", await currentHealth(true))),
    decide,
    imports,
    settings: requestSettings,
  },
  system: {
    status: systemStatus,
    tasks: systemTasks,
    activity,
    users,
    libraries,
    services,
    downloading,
    matching,
    backups: systemBackups,
    trash,
    diagnostics,
  },
};

// "#/admin" (System's Status), "#/admin/indexers", "#/admin/indexers/list": a service's page, which the header and
// the sidebar (shell.js) place it in.
export async function adminPage(parts, ctx) {
  onSignedOut(ctx.signedOut);
  if (!ctx.me.is_admin) {
    show("Manage", h("h1", {}, "Manage"), h("p", { class: "status" }, "This part of Melete is for admins. An admin can make you one under Manage, System, Users."));
    return;
  }
  const place = placeOf(parts);
  if (!place?.item) {
    show("Not found", h("h1", {}, "Not found"), h("p", {}, h("a", { href: "#/admin/movies" }, "Go to Manage")));
    return;
  }
  const { service, item } = place;
  const area = item.area ?? service.key;
  // A page the item owns under it (a title's) is drawn by its own key, with the id after it.
  const key = item.under?.includes(parts[2]) ? parts[2] : item.key;
  const own = !item.area && MANAGE[service.key]?.[key];
  if (own) return managePage(own, place, { ...ctx, id: key === item.key ? undefined : parts[3] });
  if (key !== item.key) {
    show("Not found", h("h1", {}, "Not found"), h("p", {}, h("a", { href: `#/admin/${service.key}` }, `Go to ${service.name}`)));
    return;
  }
  const heading = item.key === "overview" && !item.area ? service.name : item.name;
  const body = h("div", { class: "admin-body" }, h("h1", {}, heading), placeholders(heading.toLowerCase(), { kind: "lines" }));
  show(`${item.name} – ${service.name}`, body);
  // A page Anime has of its own, else the series one it borrows, saying so.
  const page = PAGES[area]?.[item.key] ?? PAGES[service.pagesOf]?.[item.key];
  const borrowed = !PAGES[area]?.[item.key] && service.pagesOf;
  const content = h("div", { class: "admin-content" }, placeholders(heading.toLowerCase(), { kind: "lines" }));
  const note = borrowed ? h("p", { class: "note" }, "Anime uses the series pages until it has its own, so what's here is every series, anime or not.") : "";
  body.replaceChildren(h("h1", {}, heading), note, content);
  const fill = async (message) => {
    // Movies, Series and Music narrow the pages they share to their own kind of title.
    const kind = ["movies", "series", "music"].includes(service.pagesOf ?? area) ? service.pagesOf ?? area : undefined;
    const parts = await page({ ...ctx, kind, service, reload: (said) => fill(said).catch(failing) });
    if (!ctx.current()) return;
    content.replaceChildren();
    append(content, [message ? h("p", { class: "flash", role: "status" }, message) : null, ...parts]);
  };
  const failing = (err) => {
    if (err instanceof SignedOut) return ctx.signedOut();
    content.replaceChildren(failed(heading.toLowerCase(), err, () => fill().catch(failing)));
  };
  await fill().catch(failing);
}

// What one finding looks like: its words, and a link to where it's dealt with.
function findingRow(f) {
  const label = { problem: "Problem", waiting: "Waiting", note: "Note" }[f.level];
  return h(
    "li",
    { class: `finding finding-${f.level}` },
    h("div", {}, h("span", { class: `badge${f.level === "problem" ? " state-failed" : ""}` }, label), " ", f.text),
    h("div", { class: "row-actions" }, h("a", { href: f.href }, f.level === "note" ? "Open" : "Deal with it")),
  );
}

// Problems first, then what's waiting, then notes.
const ORDER = { problem: 0, waiting: 1, note: 2 };
const byLevel = (list) => [...list].sort((a, b) => ORDER[a.level] - ORDER[b.level]);

// What needs attention in one area, as the first group of its overview.
function attention(key, name, found) {
  const list = byLevel(found?.[key] ?? []);
  return group(
    "What needs attention",
    list.length ? h("ul", { class: "rows" }, list.map(findingRow)) : h("p", { class: "status" }, `Nothing in ${name} needs attention.`),
  );
}
