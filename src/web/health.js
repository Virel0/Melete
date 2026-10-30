// How each admin area is doing, from `GET /api/v1/admin/health`: what's wrong, what's
// waiting and what's worth knowing, each with a link to where it's dealt with.

import { api, plural } from "/web/lib.js";

// The areas, in the order the switcher lists them (the server answers anime last).
export const AREAS = [
  { key: "movies", name: "Movies" },
  { key: "series", name: "Series" },
  { key: "anime", name: "Anime" },
  { key: "music", name: "Music" },
  { key: "indexers", name: "Indexers" },
  { key: "requests", name: "Requests" },
  { key: "system", name: "System" },
];

// Where each kind of finding is dealt with.
const WHERE = {
  no_indexers: () => "#/admin/indexers/add",
  indexers_all_off: () => "#/admin/indexers/list",
  indexers_off: () => "#/admin/indexers/list",
  indexer_failed: () => "#/admin/indexers/list",
  // Each service has its own To decide; the Requests area's finding counts them all.
  requests_waiting: (area) => (area === "requests" ? "#/admin/requests/decide" : `#/admin/${area}/decide`),
  downloads_failed: (area) => `#/admin/${area}/activity`,
  follow_failed: () => "#/admin/music",
  files_missing: () => "#/admin/system/libraries",
  files_unparsed: () => "#/admin/system/libraries",
  disk_nearly_full: () => "#/admin/system/status",
  backup_failed: () => "#/admin/system/backups",
  backup_old: () => "#/admin/system/backups",
  gpu_unavailable: () => "#/admin/system/tasks",
  opensubtitles_used_up: () => "#/admin/system/services",
  tmdb_off: () => "#/admin/system/services",
  tmdb_refused: () => "#/admin/system/services",
  tmdb_failing: () => "#/admin/system/services",
  // Each service has its own Needs a match.
  titles_unmatched: (area) => `#/admin/${area}/match`,
};

// Every area's findings, as { movies: [{ level, code, text, href, count }], … }.
export async function areaHealth() {
  const health = await api("/admin/health");
  return Object.fromEntries(
    health.areas.map((a) => [
      a.area,
      a.findings.map((f) => ({ level: f.level, code: f.code, text: f.message, count: f.count, href: (WHERE[f.code] ?? (() => `#/admin/${a.area}`))(a.area) })),
    ]),
  );
}

// The health of every area, kept while moving between pages and asked again after a minute,
// so the header doesn't wait on the server for every page. Each answer is also sent as a
// `melete:health` event, so what shows it can follow. A refused answer (a way in that doesn't
// open admin pages) is thrown, with its status.
let health = null;
let healthAt = 0;
export async function currentHealth(force = false) {
  if (force || !health || Date.now() - healthAt > 60_000) {
    health = await areaHealth();
    healthAt = Date.now();
    document.dispatchEvent(new CustomEvent("melete:health", { detail: health }));
  }
  return health;
}

// After a change that may have fixed or caused something, the next look asks again.
export function forgetHealth() {
  health = null;
}

// The few words the switcher shows beside an area: problems first, then what's waiting.
export function badgeText(findings) {
  const problems = findings.filter((f) => f.level === "problem").length;
  if (problems) return { level: "problem", text: plural(problems, "problem", "problems") };
  const waiting = findings.filter((f) => f.level === "waiting").reduce((n, f) => n + f.count, 0);
  if (waiting) return { level: "waiting", text: `${waiting} waiting` };
  return null;
}
