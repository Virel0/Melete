// Manage's services, in the order the header's switcher lists them: Movies, Series, Anime,
// Music, Indexers and System. Each has its pages in groups, the way its sidebar lists them
// (Library first, then Activity, Wanted, Requests and Settings), and the admin health areas
// whose findings it shows in words. admin.js draws the pages; this file only says what they are
// and where they are, so the header and the sidebar can list them without loading them.
//
// An item is { key, name } for `#/admin/<service>/<key>`, or { area, key, name } for a page
// at `#/admin/<area>/<key>` that the service has as its own (System keeps the old requests
// pages, every service's requests together, at their addresses, in a group it doesn't list:
// `hidden`). A new page goes in its service's group here, and its function in admin.js's PAGES or
// manage.js's MANAGE under the same key. An item may also own addresses under it (`under`: a
// title's page at `#/admin/movies/title/12` is the Library's). Anime has pages of its own;
// where it borrows a series page, that page says so.

const LIBRARY = { key: "library", name: "Library", under: ["title"] };
const ADD = { key: "add", name: "Add new" };
const MATCH = { key: "match", name: "Needs a match" };
// Each opens on its Library, which took the place of its overview; its wanted list's page is
// its Missing now.
const MOVED = { overview: "library", wanted: "missing" };

const CALENDAR = { items: [{ key: "calendar", name: "Calendar" }] };
const ACTIVITY = {
  name: "Activity",
  items: [
    { key: "activity", name: "Queue" },
    { key: "history", name: "History" },
    { key: "replaced", name: "Replaced files" },
  ],
};
const MISSING = { key: "missing", name: "Missing" };
const TAKEN_OFF = { key: "removed", name: "Taken off" };

const VIDEO = [
  { items: [LIBRARY, ADD, MATCH] },
  CALENDAR,
  ACTIVITY,
  { name: "Wanted", items: [MISSING, { key: "cutoff", name: "Cutoff unmet" }, TAKEN_OFF] },
  { name: "Requests", items: [{ key: "decide", name: "To decide" }, { key: "defaults", name: "Where they go" }] },
  {
    name: "Settings",
    items: [
      { key: "profiles", name: "Profiles" },
      { key: "formats", name: "Custom formats" },
      { key: "sources", name: "Import lists" },
      { key: "indexers", name: "Indexers" },
      { key: "downloading", name: "Downloading" },
      { key: "files", name: "How files are handled" },
    ],
  },
];

export const SERVICES = [
  { key: "movies", name: "Movies", health: ["movies"], moved: MOVED, groups: VIDEO },
  { key: "series", name: "Series", health: ["series"], moved: MOVED, groups: VIDEO },
  { key: "anime", name: "Anime", health: ["anime"], moved: MOVED, groups: VIDEO, pagesOf: "series" },
  {
    key: "music",
    name: "Music",
    health: ["music"],
    // Its Matching page is its Needs a match now.
    moved: { ...MOVED, matching: "match" },
    groups: [
      { items: [{ ...LIBRARY, under: ["artist", "album"] }, ADD, MATCH] },
      CALENDAR,
      ACTIVITY,
      {
        name: "Wanted",
        items: [MISSING, { key: "songs", name: "Wanted songs" }, { key: "following", name: "Followed artists" }, TAKEN_OFF],
      },
      {
        name: "Requests",
        items: [
          { key: "decide", name: "To decide" },
          { key: "defaults", name: "Where they go" },
          { key: "imports", name: "Playlist imports" },
        ],
      },
      {
        name: "Settings",
        items: [
          { key: "profiles", name: "Profiles" },
          { key: "indexers", name: "Indexers" },
          { key: "downloading", name: "Downloading" },
          { key: "files", name: "How files are handled" },
        ],
      },
    ],
  },
  {
    key: "indexers",
    name: "Indexers",
    health: ["indexers"],
    // It opens on its list, with what needs attention above it; the overview it had is gone.
    moved: { overview: "list" },
    groups: [
      {
        items: [
          { key: "list", name: "Indexers" },
          { key: "add", name: "Add an indexer" },
          { key: "soulseek", name: "Soulseek" },
          { key: "activity", name: "RSS syncs" },
          { key: "blocklist", name: "Blocklist" },
          { key: "try", name: "Try a release name" },
        ],
      },
    ],
  },
  {
    key: "system",
    name: "System",
    health: ["system"],
    // It opens on Status, which took the place of its overview and of every service's health.
    moved: { overview: "status" },
    groups: [
      { items: [{ key: "status", name: "Status" }] },
      { name: "Activity", items: [{ key: "tasks", name: "Tasks" }, { key: "activity", name: "Searches and downloads" }] },
      {
        // Each service has its own requests pages now; these keep their old addresses, every
        // service's requests together, without being listed.
        name: "Requests",
        hidden: true,
        items: [
          { area: "requests", key: "overview", name: "Requests overview" },
          { area: "requests", key: "decide", name: "To decide" },
          { area: "requests", key: "imports", name: "Playlist imports" },
          { area: "requests", key: "settings", name: "Where they go" },
        ],
      },
      {
        name: "Settings",
        items: [
          { key: "users", name: "Users" },
          { key: "libraries", name: "Libraries" },
          { key: "services", name: "Outside services" },
          { key: "downloading", name: "Downloading" },
          { key: "matching", name: "Matching" },
        ],
      },
      {
        name: "Upkeep",
        items: [
          { key: "backups", name: "Backups" },
          { key: "trash", name: "Trash" },
          { key: "diagnostics", name: "Diagnostics" },
        ],
      },
    ],
  },
];

// The address of a service's item.
export function itemHref(service, item) {
  return `#/admin/${item.area ?? service.key}/${item.key}`;
}

// Every item of every service, with the area its address is under.
const ITEMS = SERVICES.flatMap((service) =>
  service.groups.flatMap((g) => g.items.map((item) => ({ service, item, area: item.area ?? service.key }))),
);

// Which service and item an address under #/admin is: { service, item }, the item null when
// the address names no page the service lists. An area's address without a page is its first
// page; "#/admin" alone is System's Status. A service's `moved` maps an old page's key to
// the one that took its place; an address an item owns (`under`) is that item's.
export function placeOf(parts) {
  // "#/admin" alone is System's Status.
  const [, area = "system", asked = parts[1] ? undefined : "status"] = parts;
  // A page that moved keeps its old address, which leads to where it went.
  const key = SERVICES.find((s) => s.key === area)?.moved?.[asked] ?? asked;
  const owns = (item) => key === undefined || item.key === key || item.under?.includes(key);
  const found = ITEMS.find((x) => x.area === area && owns(x.item));
  if (found) return { service: found.service, item: found.item };
  const service = SERVICES.find((s) => s.key === area);
  return service ? { service, item: null } : null;
}

// The sidebar for a service, its page `place.item` marked (sidebar.js says what it holds).
export function serviceSidebar({ service, item }) {
  return {
    label: `${service.name} pages`,
    title: service.name,
    groups: service.groups.filter((g) => !g.hidden).map((g) => ({
      name: g.name,
      items: g.items.map((i) => ({ href: itemHref(service, i), name: i.name, here: i === item })),
    })),
  };
}

// Every finding a service shows, from the admin health answer (`found`, by area).
export function findingsOf(service, found) {
  return service.health.flatMap((area) => found?.[area] ?? []);
}
