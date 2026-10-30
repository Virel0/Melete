// The command palette: Ctrl+K (⌘K on a Mac) or / opens it anywhere, and typing finds a
// page, a setting, or a title, artist or song in the library (what the user may see of it).
// It's a modal dialog: focus goes into it and stays there, Escape closes it, and focus goes
// back to where it was. Arrow keys move through the results, Enter opens one.

import { SignedOut, api, h } from "/web/lib.js";

// Where everyone can go. `words` are other names people may type for the same place.
const PAGES = [
  { name: "Home", href: "#/", kind: "Page" },
  { name: "Movies", href: "#/movies", kind: "Page" },
  { name: "Shows", href: "#/shows", kind: "Page", words: "series tv" },
  { name: "Anime", href: "#/anime", kind: "Page", words: "series japanese animation" },
  { name: "My List", href: "#/favourites", kind: "Page", words: "favourites watchlist" },
  { name: "Listen", href: "#/music", kind: "Page", words: "music home" },
  { name: "Albums", href: "#/music/albums", kind: "Listen", words: "music records" },
  { name: "Artists", href: "#/music/artists", kind: "Listen", words: "music bands" },
  { name: "Songs", href: "#/music/songs", kind: "Listen", words: "music tracks" },
  { name: "Genres", href: "#/music/genres", kind: "Listen", words: "music styles" },
  { name: "Radio", href: "#/music/radio", kind: "Listen", words: "music station infinity similar" },
  { name: "Liked songs", href: "#/music/liked", kind: "Listen", words: "music likes favourites hearts" },
  { name: "Playlists", href: "#/music/playlists", kind: "Listen", words: "music aoide" },
  { name: "Calendar", href: "#/calendar", kind: "Page", words: "upcoming coming soon air dates releases schedule" },
  { name: "Ask for something", href: "#/requests/ask", kind: "Requests", words: "request add new find" },
  { name: "Your requests", href: "#/requests/mine", kind: "Requests" },
  { name: "Playlist imports", href: "#/requests/imports", kind: "Requests", words: "spotify m3u" },
  { name: "Your account", href: "#/settings/account", kind: "Setting", words: "password sessions sign out devices" },
  { name: "Change your password", href: "#/settings/account", kind: "Setting" },
  { name: "Sign in a device with a code", href: "#/settings/account", kind: "Setting", words: "quick connect tv" },
  { name: "API keys", href: "#/settings/account", kind: "Setting", words: "token" },
];

// Admins' areas, pages and settings, by the names on them.
const ADMIN = [
  { name: "System status", href: "#/admin/system/status", kind: "Manage", words: "admin overview health problems every service dashboard disk space free" },
  { name: "Tasks", href: "#/admin/system/tasks", kind: "Manage", words: "background jobs run now scan schedule" },
  { name: "Backups", href: "#/admin/system/backups", kind: "Manage", words: "database copies restore" },
  { name: "Downloading", href: "#/admin/system/downloading", kind: "Manage", words: "deluge seedbox sftp torrents" },
  { name: "Movies: library", href: "#/admin/movies/library", kind: "Manage", words: "radarr posters table missing downloading cutoff monitored" },
  { name: "Series: library", href: "#/admin/series/library", kind: "Manage", words: "sonarr shows posters table missing episodes" },
  { name: "Anime: library", href: "#/admin/anime/library", kind: "Manage", words: "sonarr japanese animation posters table missing" },
  { name: "Music: library", href: "#/admin/music/library", kind: "Manage", words: "lidarr artists albums missing" },
  { name: "Movies: missing", href: "#/admin/movies/missing", kind: "Manage", words: "radarr wanted search all missing" },
  { name: "Movies: cutoff unmet", href: "#/admin/movies/cutoff", kind: "Manage", words: "upgrades quality below cutoff search" },
  { name: "Movies: history", href: "#/admin/movies/history", kind: "Manage", words: "grabbed imported failed events activity" },
  { name: "Movies: calendar", href: "#/admin/movies/calendar", kind: "Manage", words: "release dates cinema digital disc" },
  { name: "Movies: taken off the list", href: "#/admin/movies/removed", kind: "Manage", words: "removed allow back add again" },
  { name: "Movies: import lists", href: "#/admin/movies/sources", kind: "Manage", words: "tmdb imdb rotten tomatoes radarr import" },
  { name: "Movies: quality profiles", href: "#/admin/movies/profiles", kind: "Manage", words: "custom formats" },
  { name: "Series: missing", href: "#/admin/series/missing", kind: "Manage", words: "sonarr missing episodes search" },
  { name: "Series: taken off the list", href: "#/admin/series/removed", kind: "Manage", words: "removed allow back add again" },
  { name: "Series: import lists", href: "#/admin/series/sources", kind: "Manage", words: "sonarr import" },
  { name: "Series: quality profiles", href: "#/admin/series/profiles", kind: "Manage", words: "custom formats" },
  { name: "Anime", href: "#/admin/anime", kind: "Manage", words: "japanese animation sonarr" },
  { name: "Anime: missing", href: "#/admin/anime/missing", kind: "Manage", words: "missing episodes search" },
  { name: "Anime: quality profiles", href: "#/admin/anime/profiles", kind: "Manage", words: "anime custom formats" },
  { name: "Music: missing", href: "#/admin/music/missing", kind: "Manage", words: "lidarr wanted artists albums" },
  { name: "Music: wanted songs", href: "#/admin/music/songs", kind: "Manage", words: "soulseek slskd" },
  { name: "Music: taken off the list", href: "#/admin/music/removed", kind: "Manage", words: "removed artists albums allow back add again" },
  { name: "Music: needs a match", href: "#/admin/music/match", kind: "Manage", words: "musicbrainz unmatched release matching albums" },
  { name: "Music: profiles", href: "#/admin/music/profiles", kind: "Manage", words: "lidarr flac lossless quality import" },
  { name: "Movies: requests to decide", href: "#/admin/movies/decide", kind: "Manage", words: "approve decline waiting" },
  { name: "Series: requests to decide", href: "#/admin/series/decide", kind: "Manage", words: "approve decline waiting" },
  { name: "Anime: requests to decide", href: "#/admin/anime/decide", kind: "Manage", words: "approve decline waiting" },
  { name: "Music: requests to decide", href: "#/admin/music/decide", kind: "Manage", words: "approve decline waiting" },
  { name: "Movies: custom formats", href: "#/admin/movies/formats", kind: "Manage", words: "scores conditions radarr" },
  { name: "Series: custom formats", href: "#/admin/series/formats", kind: "Manage", words: "scores conditions sonarr" },
  { name: "How files are handled", href: "#/admin/movies/files", kind: "Manage", words: "rename move media management trash" },
  { name: "Movies: add new", href: "#/admin/movies/add", kind: "Manage", words: "add search tmdb radarr import" },
  { name: "Series: add new", href: "#/admin/series/add", kind: "Manage", words: "add search tmdb sonarr import" },
  { name: "Anime: add new", href: "#/admin/anime/add", kind: "Manage", words: "add search tmdb sonarr import" },
  { name: "Music: add new", href: "#/admin/music/add", kind: "Manage", words: "add search musicbrainz artist album lidarr import" },
  { name: "Movies: needs a match", href: "#/admin/movies/match", kind: "Manage", words: "unmatched tmdb matching" },
  { name: "Series: needs a match", href: "#/admin/series/match", kind: "Manage", words: "unmatched tmdb matching" },
  { name: "Anime: needs a match", href: "#/admin/anime/match", kind: "Manage", words: "unmatched tmdb matching" },
  { name: "Followed artists", href: "#/admin/music/following", kind: "Manage", words: "new albums following" },
  { name: "Queue", href: "#/admin/movies/activity", kind: "Manage", words: "downloads deluge seedbox activity" },
  { name: "Indexers", href: "#/admin/indexers/list", kind: "Manage", words: "trackers torznab test rename turn off remove" },
  { name: "Add an indexer", href: "#/admin/indexers/add", kind: "Manage", words: "public trackers pirate bay nyaa animetosho uindex torznab jackett own custom" },
  { name: "Import indexers from Prowlarr", href: "#/admin/indexers/add", kind: "Manage" },
  { name: "Soulseek", href: "#/admin/indexers/soulseek", kind: "Manage", words: "slskd shares songs albums" },
  { name: "RSS syncs", href: "#/admin/indexers/activity", kind: "Manage", words: "sync now" },
  { name: "Blocklist", href: "#/admin/indexers/blocklist", kind: "Manage", words: "blocked releases never grab" },
  { name: "Try a release name", href: "#/admin/indexers/try", kind: "Manage", words: "parse release quality profile evaluate test name" },
  { name: "Requests to decide", href: "#/admin/requests/decide", kind: "Manage", words: "approve decline" },
  { name: "Where requests go", href: "#/admin/requests/settings", kind: "Manage", words: "auto approve defaults" },
  { name: "Users", href: "#/admin/system/users", kind: "Manage", words: "accounts reset password admin api keys scopes" },
  { name: "Libraries", href: "#/admin/system/libraries", kind: "Manage", words: "scan folders rescan title folder" },
  { name: "Outside services", href: "#/admin/system/services", kind: "Manage", words: "tmdb musicbrainz flaresolverr openrouter identity provider oidc lyrics" },
  { name: "Matching titles by hand", href: "#/admin/system/matching", kind: "Manage", words: "unmatched tmdb fix" },
  { name: "Searches and downloads", href: "#/admin/system/activity", kind: "Manage", words: "activity progress importing" },
  { name: "Trash", href: "#/admin/system/trash", kind: "Manage", words: "put back restore upgrades" },
  { name: "Diagnostics", href: "#/admin/system/diagnostics", kind: "Manage", words: "reports logs errors" },
];

const SHORTCUTS = [
  ["Up", "Down", "move"],
  ["Enter", null, "open"],
  ["Esc", null, "close"],
];

let dialog = null;

// Sets the palette up once signed in; `me` decides whether admin places are listed.
export function installPalette(me, opener) {
  dialog?.remove();
  // What the palette's parts share: its places, its parts, and what's shown and chosen.
  const pal = {
    places: me.is_admin ? [...PAGES, ...ADMIN] : PAGES,
    ...paletteParts(me),
    shown: [],
    active: 0,
    returnTo: null,
    asking: 0,
    titles: [],
    typing: undefined,
  };
  const { input, results, said, keys } = pal;
  dialog = h(
    "dialog",
    { class: "palette", "aria-labelledby": "palette-label" },
    h("label", { for: "palette-input", id: "palette-label" }, "Go to"),
    input,
    results,
    said,
    keys,
  );
  document.body.append(dialog);
  listen(pal);
  const open = () => openPalette(pal);
  opener.onclick = open;
  return open;
}

// The box to type in, the results list, what's said about them, and the keys to use.
function paletteParts(me) {
  const input = h("input", {
    id: "palette-input",
    type: "text",
    role: "combobox",
    "aria-expanded": "true",
    "aria-controls": "palette-results",
    "aria-autocomplete": "list",
    autocomplete: "off",
    spellcheck: "false",
    placeholder: me.is_admin ? "A page, setting, title, artist or song" : "A page, title, artist or song",
  });
  const results = h("ul", { id: "palette-results", role: "listbox", "aria-label": "Results" });
  const said = h("p", { class: "visually-hidden", role: "status" });
  const keys = h(
    "ul",
    { class: "palette-keys", "aria-label": "Keys" },
    SHORTCUTS.map(([a, b, what]) => h("li", {}, h("kbd", {}, a), b ? [" ", h("kbd", {}, b)] : null, ` ${what}`)),
  );
  return { input, results, said, keys };
}

// Closes the palette and goes to the place or title chosen.
function go(item) {
  dialog.close();
  if (location.hash === item.href) dispatchEvent(new HashChangeEvent("hashchange"));
  else location.hash = item.href;
}

// Marks the result the arrows are on, for the eye and for screen readers.
function mark(pal) {
  const { input, results } = pal;
  [...results.children].forEach((li, i) => li.setAttribute("aria-selected", String(i === pal.active)));
  const current = results.children[pal.active];
  if (current) {
    input.setAttribute("aria-activedescendant", current.id);
    current.scrollIntoView({ block: "nearest" });
  } else input.removeAttribute("aria-activedescendant");
}

// The places matching what's typed, best first, then the titles found for it.
function draw(pal) {
  const { input, results, said } = pal;
  const q = input.value.trim().toLowerCase();
  const scored = pal.places
    .map((p) => {
      const name = p.name.toLowerCase();
      // Other names count when one of their words starts with what's typed ("ind" isn't "find").
      const other = (p.words ?? "").split(" ").some((w) => w.startsWith(q));
      const score = !q ? 1 : name.startsWith(q) ? 3 : name.includes(q) ? 2 : other ? 1 : 0;
      return { p, score };
    })
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score)
    .map((x) => x.p);
  pal.shown = [...scored.slice(0, q ? 12 : 8), ...pal.titles];
  pal.active = Math.min(pal.active, Math.max(0, pal.shown.length - 1));
  results.replaceChildren(
    ...pal.shown.map((item, i) =>
      h(
        "li",
        { id: `palette-option-${i}`, role: "option", onclick: () => go(item), onmousemove: () => { if (pal.active !== i) { pal.active = i; mark(pal); } } },
        h("span", { class: "palette-name" }, item.name),
        h("span", { class: "palette-kind" }, item.kind),
      ),
    ),
  );
  if (!pal.shown.length) results.append(h("li", { class: "palette-empty", role: "presentation" }, `Nothing called “${input.value.trim()}”.`));
  said.textContent = pal.shown.length ? `${pal.shown.length} results` : "No results";
  mark(pal);
}

// Titles in the library, asked for once typing pauses.
function findTitles(pal) {
  clearTimeout(pal.typing);
  const q = pal.input.value.trim();
  pal.titles = [];
  if (q.length < 2) return;
  pal.typing = setTimeout(async () => {
    const mine = ++pal.asking;
    const query = new URLSearchParams({ q, limit: "4" });
    // Music is left out quietly for someone the server keeps it from.
    const music = (path) => api(path).catch((err) => {
      if (err instanceof SignedOut) throw err;
      return { items: [] };
    });
    try {
      const [movies, series, artists, albums, songs] = await Promise.all([
        api(`/movies?${query}`),
        api(`/series?${query}`),
        music(`/music/artists?${query}`),
        music(`/music/albums?${query}`),
        music(`/music/tracks?${query}&q_in=track&sort=title`),
      ]);
      if (mine !== pal.asking || !dialog.open) return;
      pal.titles = [
        ...movies.items.map((m) => ({ name: m.year ? `${m.title} (${m.year})` : m.title, href: `#/movies/${m.id}`, kind: "Movie" })),
        ...series.items.map((s) => ({ name: s.year ? `${s.title} (${s.year})` : s.title, href: `#/series/${s.id}`, kind: "Series" })),
        ...(artists.items ?? []).map((a) => ({ name: a.name, href: `#/music/artists/${a.id}`, kind: "Artist" })),
        ...(albums.items ?? []).map((a) => ({ name: `${a.title} by ${a.artist}`, href: `#/music/albums/${a.id}`, kind: "Album" })),
        // A song opens its album with the song picked out, ready to play.
        ...(songs.items ?? []).map((t) => ({ name: `${t.title} by ${t.artist}, from ${t.album}`, href: `#/music/albums/${t.album_id}?track=${t.id}`, kind: "Song" })),
      ];
      draw(pal);
    } catch (err) {
      if (err instanceof SignedOut) return;
      pal.said.textContent = `Couldn't search the library: ${err.message}`;
    }
  }, 250);
}

// Typing, the arrows and Enter in the box; a click outside it and closing it.
function listen(pal) {
  const { input } = pal;
  input.addEventListener("input", () => {
    pal.active = 0;
    findTitles(pal);
    draw(pal);
  });
  input.addEventListener("keydown", (event) => {
    const { shown } = pal;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!shown.length) return;
      pal.active = (pal.active + (event.key === "ArrowDown" ? 1 : shown.length - 1)) % shown.length;
      mark(pal);
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (shown[pal.active]) go(shown[pal.active]);
    }
  });
  // A click on the backdrop (outside the box) closes it, as Escape does.
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
  dialog.addEventListener("close", () => pal.returnTo?.focus?.());
}

// Opens the palette empty, remembering where focus was to give it back on closing.
function openPalette(pal) {
  if (dialog.open) return;
  pal.returnTo = document.activeElement;
  pal.input.value = "";
  pal.titles = [];
  pal.active = 0;
  draw(pal);
  dialog.showModal();
  pal.input.focus();
}

// Ctrl+K or ⌘K anywhere, and / when not typing into something.
export function paletteKeys(open) {
  document.addEventListener("keydown", (event) => {
    if (!open()) return;
    const typing = event.target.closest?.("input, textarea, select, [contenteditable]");
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      open()();
    } else if (event.key === "/" && !typing && !event.ctrlKey && !event.metaKey && !event.altKey) {
      event.preventDefault();
      open()();
    }
  });
}
