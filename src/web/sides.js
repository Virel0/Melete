// The three sides of the web UI and what the header lists for each; shell.js draws them.
//
// - Watch, for everyone: Home, Movies, Shows, Anime (when a library has the anime category) and
//   My List. Anime is kept apart from shows wherever the web separates them.
// - Listen, for everyone: the music sections.
// - Manage, for admins, and only where the way in opens admin pages: Manage's services, each
//   with its health in words (manage-services.js).
//
// Each entry is { key, name, href, here(route) }: `here` says whether the page on screen is
// that entry, given the route ({ parts, params } from the address). A new page is added here:
// a new entry, or a wider `here`. Addresses stay as they are (`#/…`, `#/music/…`,
// `#/admin/…`), so links and Back keep working.

import { listenGroups, listenLists } from "/web/music-sidebar.js";

export const SIDES = [
  { key: "watch", name: "Watch", home: "#/" },
  { key: "listen", name: "Listen", home: "#/music" },
  { key: "manage", name: "Manage", home: "#/admin/movies" },
];

// Which side an address belongs to, or null for the pages that belong to none (your account,
// requests, the calendar): those keep the side you came from.
export function sideOf({ parts }) {
  const [first, second] = parts;
  if (first === "music") return "listen";
  if (first === "admin" || first === "wanted") return "manage";
  if (first === "settings") return !second || second === "account" ? null : "manage";
  if (first === "requests" || first === "calendar") return null;
  return "watch";
}

// Watch's entries, given the categories the video libraries use (from /browse/libraries).
export function watchEntries(categories) {
  const list = ({ parts }, kind) => parts[0] === kind && !parts[1];
  return [
    { key: "home", name: "Home", href: "#/", here: ({ parts }) => parts.length === 0 },
    { key: "movies", name: "Movies", href: "#/movies", here: (route) => list(route, "movies") },
    { key: "shows", name: "Shows", href: "#/shows", here: (route) => list(route, "shows") },
    categories.includes("anime") ? { key: "anime", name: "Anime", href: "#/anime", here: (route) => list(route, "anime") } : null,
    { key: "favourites", name: "My List", href: "#/favourites", here: (route) => list(route, "favourites") },
  ].filter(Boolean);
}

// Listen's entries: the music sections, Home first (where `#/music` opens).
export function listenEntries() {
  const section = (key) => ({ parts }) => parts[0] === "music" && (parts[1] ?? "") === key && !parts[2];
  const home = section("home");
  return [
    { key: "home", name: "Home", href: "#/music", here: (route) => home(route) || section("")(route) },
    { key: "albums", name: "Albums", href: "#/music/albums", here: section("albums") },
    { key: "artists", name: "Artists", href: "#/music/artists", here: section("artists") },
    { key: "songs", name: "Songs", href: "#/music/songs", here: section("songs") },
    { key: "genres", name: "Genres", href: "#/music/genres", here: section("genres") },
    { key: "composers", name: "Composers", href: "#/music/composers", here: section("composers") },
    { key: "works", name: "Works", href: "#/music/works", here: section("works") },
    { key: "radio", name: "Radio", href: "#/music/radio", here: section("radio") },
  ];
}

// What's the listener's own: their liked songs and albums, and playlists. The sidebar lists them under the
// sections; the header doesn't, so on a narrow screen the menu button opens them.
function yoursEntries() {
  const section = (key) => ({ parts }) => parts[0] === "music" && parts[1] === key;
  return [
    { name: "Liked songs", href: "#/music/liked", here: section("liked") },
    { name: "Liked albums", href: "#/music/liked-albums", here: section("liked-albums") },
    { name: "Playlists", href: "#/music/playlists", here: section("playlists") },
  ];
}

// Listen's sidebar: its sections, which the header also lists, so a narrow screen has no menu
// button for them; then, once the listener's pins and playlists come (music-sidebar.js), their
// own pages, what they pinned and their playlists. The listener's own pages are left out for
// someone to whom likes and playlists are closed.
export function listenSidebar(route) {
  const items = (list) => list.map((e) => ({ href: e.href, name: e.name, here: e.here(route) }));
  const hash = location.hash;
  return {
    label: "Listen",
    title: "Listen",
    groups: [{ inHeader: true, items: items(listenEntries()) }],
    more: listenLists().then((lists) => listenGroups(lists, hash, items(yoursEntries()))),
  };
}
