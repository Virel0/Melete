// Wanted music, for admins: the wanted artists with their albums, searching for
// one album or every album of an artist (a run that's followed as it goes), and wanted songs,
// added here and searched for on Soulseek. Results and grabs work as video's do.

import { confirmInline, field, form, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural } from "/web/lib.js";
import { searchMissing } from "/web/wanted-missing.js";
import { searchResults } from "/web/wanted.js";

const PAGE = 50;
// How often a whole-artist run is looked at while it goes.
const FOLLOW_MS = 3000;

// A list filtered as you type and shown 50 at a time, as wanted movies and series are.
function filteredList(all, { id, label, matches, row, noun }) {
  const filter = h("input", { id, type: "search", autocomplete: "off" });
  const count = h("p", { class: "status", "aria-live": "polite" });
  const list = h("ul", { class: "rows" });
  const more = h("div", { class: "more" });
  let shown = 0;
  let matching = all;
  const draw = () => {
    const q = filter.value.trim().toLowerCase();
    matching = all.filter((item) => !q || matches(item, q));
    shown = 0;
    list.replaceChildren();
    count.textContent = q ? `${number.format(matching.length)} of ${plural(all.length, noun[0], noun[1])}` : plural(all.length, noun[0], noun[1]);
    showMore();
  };
  const showMore = () => {
    const next = matching.slice(shown, shown + PAGE).map(row);
    list.append(...next);
    shown += next.length;
    const left = matching.length - shown;
    more.replaceChildren(
      left > 0
        ? h("button", { type: "button", class: "quiet", onclick: () => {
            showMore();
            next.at(-1)?.nextElementSibling?.querySelector("button, select, input")?.focus();
          } }, `Show more (${number.format(left)} left)`)
        : "",
    );
  };
  let typing;
  filter.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(draw, 250);
  });
  draw();
  return [all.length ? h("div", { class: "toolbar" }, h("div", {}, h("label", { for: id }, label), filter)) : null, count, list, more];
}

// Runs a search and shows its results in `results`; `button` is off meanwhile.
async function runSearch(ctx, button, results, path, waiting, name) {
  button.disabled = true;
  results.replaceChildren(h("p", { class: "status", role: "status" }, waiting));
  try {
    const found = await api(path, { method: "POST" });
    results.replaceChildren(searchResults(found, ctx));
    announce(`${plural(found.releases.length, "result", "results")} for ${name}`);
  } catch (err) {
    if (err instanceof SignedOut) return ctx.signedOut();
    results.replaceChildren(h("p", { class: "error" }, err.message));
  } finally {
    button.disabled = false;
  }
}

// ---------- Wanted artists and their albums ----------

const year = (date) => (date ? date.slice(0, 4) : null);

function albumRow(ctx, album) {
  const results = h("div");
  const search = h("button", { type: "button", class: "quiet" }, "Search");
  search.addEventListener("click", () =>
    runSearch(
      ctx,
      search,
      results,
      `/wanted/albums/${album.id}/search`,
      "Asking the indexers and Soulseek; this can take up to a minute…",
      album.title,
    ),
  );
  const facts = [year(album.release_date), album.album_type, ...album.secondary_types].filter(Boolean);
  const actions = h("div", { class: "row-actions" }, search);
  actions.append(monitorButton(ctx, "albums", album, album.title), unwantButton(ctx, "albums", album, album.title, actions, ""));
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, album.title),
      " ",
      album.monitored ? h("span", { class: "badge" }, "Monitored") : h("span", { class: "badge" }, "Not monitored"),
      album.album_id ? [" ", h("a", { class: "badge", href: `#/music/albums/${album.album_id}` }, "In the library")] : null,
      facts.length ? h("p", { class: "note" }, `${facts.join(", ")}.`) : null,
      results,
    ),
    actions,
  );
}

// Monitor or stop monitoring a wanted artist or album.
function monitorButton(ctx, kind, entry, name) {
  const button = h("button", { type: "button", class: "quiet", "aria-label": `${entry.monitored ? "Stop monitoring" : "Monitor"} ${name}` }, entry.monitored ? "Stop monitoring" : "Monitor");
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      await api(`/wanted/${kind}/${entry.id}/monitored`, { method: "PUT", body: { monitored: !entry.monitored } });
      ctx.reload(entry.monitored ? `Stopped monitoring ${name}.` : `Monitoring ${name}.`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      button.textContent = err.message;
    }
  });
  return button;
}

// Take a wanted artist (with their albums) or album off the list, asked first in its row.
function unwantButton(ctx, kind, entry, name, actions, more) {
  const button = h("button", { type: "button", class: "quiet", "aria-label": `Take ${name} off the list` }, "Take off the list");
  button.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Take ${name} off the wanted list${more}? Nothing on disk changes; Melete stops looking.${entry.source === "lidarr" ? " Importing from Lidarr leaves it off; adding it again by hand puts it back." : ""}`,
      confirm: "Take it off",
      keep: "Keep it",
      run: async () => {
        await api(`/wanted/${kind}/${entry.id}`, { method: "DELETE" });
        ctx.reload(`Took ${name} off the wanted list.`);
      },
    }),
  );
  return button;
}

// What a whole-artist run has done so far.
function runView(run) {
  const done = run.albums.length;
  const head =
    run.state === "queued"
      ? "Waiting for another search to finish…"
      : run.state === "running"
        ? `Searching: ${plural(done, "album", "albums")} done so far…`
        : run.state === "done"
          ? `Done: ${run.message}.`
          : `Stopped: ${run.message}.`;
  const lines = run.albums.map((a) => {
    let said;
    if (a.skipped) said = `skipped, ${a.skipped}`;
    else if (a.error) said = `couldn't search: ${a.error}`;
    else if (!run.grab) said = "searched";
    else {
      const got = a.grabbed.find((g) => g.download_id);
      const refused = a.grabbed.map((g) => g.refused).filter(Boolean);
      said = got
        ? `grabbed ${got.title}`
        : refused.length
          ? `nothing grabbed: ${refused.join("; ")}`
          : "searched, nothing accepted";
    }
    return h("li", {}, h("strong", {}, a.title), `: ${said}.`);
  });
  return [h("p", { class: run.state === "failed" ? "error" : "status" }, head), lines.length ? h("ul", { class: "plain" }, lines) : null];
}

// One wanted artist: a box to select it, its facts and profile, searching for every album,
// its albums, and Monitor and Take off the list.
function artistRow(ctx, artist, profiles, select) {
  const { toggle, albums } = albumsToggle(ctx, artist);
  const { searchAll, run } = searchEveryAlbum(ctx, artist);
  const { profile, profileNote } = artistProfile(ctx, artist, profiles);
  const counts = `${number.format(artist.on_disk)} of ${plural(artist.monitored_albums, "monitored album", "monitored albums")} in the library`;
  const where = artist.library ? `Goes to ${artist.library}` : "No music library matched";
  const actions = h("div", { class: "row-actions" });
  actions.append(monitorButton(ctx, "artists", artist, artist.name), unwantButton(ctx, "artists", artist, artist.name, actions, ", with their albums"));
  const pick = h("input", { type: "checkbox", id: `pick-artist-${artist.id}`, checked: select.selected.has(artist.id) });
  pick.addEventListener("change", () => {
    if (pick.checked) select.selected.add(artist.id);
    else select.selected.delete(artist.id);
    select.changed();
  });
  return h(
    "li",
    {},
    h("div", { class: "pick" }, pick, h("label", { for: pick.id, class: "visually-hidden" }, `Select ${artist.name}`)),
    h(
      "div",
      {},
      h("strong", {}, artist.name),
      " ",
      artist.monitored ? h("span", { class: "badge" }, "Monitored") : h("span", { class: "badge" }, "Not monitored"),
      h("p", { class: "note" }, `${counts}. ${where}. From ${artist.source === "manual" ? "Melete" : "Lidarr"}.`),
      h(
        "div",
        { class: "search-row" },
        h("div", {}, h("label", { for: `artist-profile-${artist.id}` }, "Quality profile"), profile),
        profileNote,
      ),
      searchAll,
      run,
      h("div", { class: "search-row" }, toggle),
      albums,
    ),
    actions,
  );
}

// Show albums: the artist's wanted albums, loaded the first time they're shown.
function albumsToggle(ctx, artist) {
  const albums = h("div", { hidden: true });
  const toggle = h("button", { type: "button", class: "quiet", "aria-expanded": "false" }, "Show albums");
  let loaded = false;
  toggle.addEventListener("click", async () => {
    const open = toggle.getAttribute("aria-expanded") === "true";
    toggle.setAttribute("aria-expanded", String(!open));
    toggle.textContent = open ? "Show albums" : "Hide albums";
    albums.hidden = open;
    if (open || loaded) return;
    albums.replaceChildren(h("p", { class: "status" }, "Loading albums…"));
    try {
      const list = await api(`/wanted/artists/${artist.id}/albums`);
      loaded = true;
      // Oldest first, as an artist's albums are listed elsewhere; undated ones last.
      list.sort((a, b) => (a.release_date ?? "9999").localeCompare(b.release_date ?? "9999"));
      albums.replaceChildren(
        list.length
          ? h("ul", { class: "rows" }, list.map((album) => albumRow(ctx, album)))
          : h("p", { class: "status" }, "No albums wanted for this artist."),
      );
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      albums.replaceChildren(h("p", { class: "error" }, err.message));
    }
  });
  return { toggle, albums };
}

// Every album still to get, searched for (and grabbed) in the background, followed here.
export function searchEveryAlbum(ctx, artist) {
  const run = h("div", { role: "status" });
  const grab = h("input", { type: "checkbox", id: `grab-${artist.id}`, checked: true });
  const start = h("button", { type: "submit", class: "quiet" }, "Search for every album");
  const searchAll = h(
    "form",
    { class: "search-row", novalidate: true },
    h("div", { class: "check" }, grab, h("label", { for: `grab-${artist.id}` }, "Grab the best result of each")),
    start,
  );
  let timer = null;
  const follow = (id) => {
    clearInterval(timer);
    const look = async () => {
      try {
        const now = await api(`/artist-searches/${id}`);
        run.replaceChildren(...runView(now).filter(Boolean));
        if (now.state === "done" || now.state === "failed") {
          clearInterval(timer);
          start.disabled = false;
          announce(`${artist.name}: ${now.message}`);
        }
      } catch (err) {
        clearInterval(timer);
        start.disabled = false;
        if (err instanceof SignedOut) return ctx.signedOut();
        run.replaceChildren(h("p", { class: "error" }, err.message));
      }
    };
    timer = setInterval(look, FOLLOW_MS);
    look();
  };
  ctx.onLeave(() => clearInterval(timer));
  searchAll.addEventListener("submit", async (event) => {
    event.preventDefault();
    start.disabled = true;
    run.replaceChildren(h("p", { class: "status" }, "Starting…"));
    try {
      const started = await api(`/wanted/artists/${artist.id}/search?grab=${grab.checked}`, { method: "POST" });
      follow(started.id);
    } catch (err) {
      start.disabled = false;
      if (err instanceof SignedOut) return ctx.signedOut();
      run.replaceChildren(h("p", { class: "error" }, err.message));
    }
  });
  return { searchAll, run };
}

// The profile its albums are decided with: one chosen here, or the one Lidarr names.
function artistProfile(ctx, artist, profiles) {
  const profile = h(
    "select",
    { id: `artist-profile-${artist.id}`, class: "compact" },
    h("option", { value: "" }, artist.quality_profile ? `Lidarr's: ${artist.quality_profile}` : "Lidarr's (none)"),
    profiles.map((p) => h("option", { value: p.id, selected: p.id === artist.profile_id }, p.name)),
  );
  const profileNote = h("span", { class: "note", role: "status" });
  profile.addEventListener("change", async () => {
    try {
      await api(`/wanted/artists/${artist.id}/profile`, {
        method: "PUT",
        body: { profile_id: profile.value ? Number(profile.value) : null },
      });
      profileNote.textContent = "Saved.";
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      profileNote.textContent = err.message;
    }
  });
  return { profile, profileNote };
}

export async function wantedArtists(ctx) {
  const [artists, profiles, runs] = await Promise.all([api("/wanted/artists"), api("/music/profiles"), api("/missing-searches")]);
  if (!ctx.current()) return [];
  // Select mode, as movies and series have it: tick artists, then act on them all from
  // the bar, which stays in sight.
  const bar = h("div", { class: "select-bar", role: "region", "aria-label": "Selected artists", hidden: true });
  const selected = new Set();
  const everyone = async (label, call) => {
    const failed = [];
    const ids = [...selected];
    for (const id of ids) {
      try {
        await call(id);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        failed.push(`${artists.find((a) => a.id === id)?.name}: ${err.message}`);
      }
    }
    const done = ids.length - failed.length;
    ctx.reload(`${label} ${plural(done, "artist", "artists")}.${failed.length ? ` Not changed: ${failed.join("; ")}.` : ""}`);
  };
  const monitorAll = (on) =>
    everyone(on ? "Monitored" : "Stopped monitoring", (id) => api(`/wanted/artists/${id}/monitored`, { method: "PUT", body: { monitored: on } }));
  const drawBar = () => {
    bar.hidden = selected.size === 0;
    if (!selected.size) return;
    const actions = h("div", { class: "row-actions" });
    const fromLidarr = artists.some((a) => selected.has(a.id) && a.source === "lidarr");
    const remove = h("button", { type: "button", class: "quiet" }, "Take off the list");
    remove.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Take ${plural(selected.size, "artist", "artists")} off the wanted list, with their albums? Nothing on disk changes; Melete stops looking for them.${fromLidarr ? " Importing from Lidarr leaves them off." : ""}`,
        confirm: `Take off ${plural(selected.size, "artist", "artists")}`,
        keep: "Keep them",
        run: () => everyone("Took off", (id) => api(`/wanted/artists/${id}`, { method: "DELETE" })),
      }),
    );
    const clear = h("button", { type: "button", class: "quiet" }, "Clear selection");
    clear.addEventListener("click", () => {
      selected.clear();
      for (const box of bar.parentElement?.querySelectorAll(".pick input") ?? []) box.checked = false;
      drawBar();
      document.getElementById("artist-filter")?.focus();
    });
    actions.append(
      h("button", { type: "button", class: "quiet", onclick: () => monitorAll(true) }, "Monitor"),
      h("button", { type: "button", class: "quiet", onclick: () => monitorAll(false) }, "Stop monitoring"),
      remove,
      clear,
    );
    bar.replaceChildren(h("strong", {}, `${plural(selected.size, "artist", "artists")} selected`), actions);
  };
  const select = { selected, changed: drawBar };
  const listed = filteredList(artists, {
    id: "artist-filter",
    label: "Find an artist",
    matches: (a, q) => a.name.toLowerCase().includes(q),
    row: (a) => artistRow(ctx, a, profiles, select),
    noun: ["artist", "artists"],
  });
  return [
    artists.length ? group("Search all missing", searchMissing(ctx, "albums", runs.find((r) => r.area === "albums") ?? null)) : null,
    group(
      "Wanted artists",
      h("p", { class: "note" }, "Albums are searched for on the indexers and on Soulseek (when slskd is set up) at once; the best accepted result wins, whichever source it's from. Tick artists to act on several at once."),
      artists.length ? null : h("p", { class: "status" }, "No artists wanted yet. Import Lidarr's list in Settings."),
      ...listed,
      bar,
    ),
  ];
}

// ---------- Wanted songs ----------

// "4:23" or "263" to seconds; null when empty; NaN when it's neither.
function seconds(text) {
  const t = text.trim();
  if (!t) return null;
  const m = /^(\d{1,3}):([0-5]\d)$/.exec(t);
  if (m) return Number(m[1]) * 60 + Number(m[2]);
  return /^\d{1,5}$/.test(t) ? Number(t) : Number.NaN;
}

function songRow(ctx, song, libraries) {
  const results = h("div");
  const search = h("button", { type: "button", class: "quiet" }, "Search Soulseek");
  search.addEventListener("click", () =>
    runSearch(ctx, search, results, `/wanted/songs/${song.id}/search`, "Asking Soulseek; this can take up to a minute…", song.title),
  );
  const actions = h("div", { class: "row-actions" }, search);
  const remove = h("button", { type: "button", class: "quiet" }, "Remove");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Stop wanting “${song.title}”? Its searches go with it.`,
      confirm: "Remove song",
      run: async () => {
        await api(`/wanted/songs/${song.id}`, { method: "DELETE" });
        ctx.reload(`Removed “${song.title}”.`);
      },
    }),
  );
  actions.append(remove);
  const library = libraries.find((l) => l.id === song.library_id);
  const facts = [
    song.album ? `From ${song.album}` : "On its own",
    song.length_seconds ? `${Math.floor(song.length_seconds / 60)}:${String(song.length_seconds % 60).padStart(2, "0")} long` : null,
    library ? `goes to ${library.name}` : "no library",
    `added ${when(song.added_at)}`,
  ].filter(Boolean);
  return h(
    "li",
    {},
    h("div", {}, h("strong", {}, song.title), " by ", song.artist, h("p", { class: "note" }, `${facts.join(", ")}.`), results),
    actions,
  );
}

export async function wantedSongs(ctx) {
  const [songs, profiles, libraries] = await Promise.all([api("/wanted/songs"), api("/music/profiles"), api("/libraries")]);
  if (!ctx.current()) return [];
  const music = libraries.filter((l) => l.kind === "music");

  const title = field("song-title", "Title", h("input", { autocomplete: "off" }));
  const artist = field("song-artist", "Artist", h("input", { autocomplete: "off" }));
  const album = field("song-album", "Album (optional)", h("input", { autocomplete: "off" }), "With it, the song goes with that album; without it, in the artist's Singles folder.");
  const length = field(
    "song-length",
    "Length (optional)",
    h("input", { inputmode: "numeric", autocomplete: "off", class: "year" }),
    "Like 4:23. Tells a radio edit or a live take from it.",
  );
  const library =
    music.length > 1
      ? field(
          "song-library",
          "Library",
          h("select", {}, h("option", { value: "" }, "Where the artist's albums go"), music.map((l) => h("option", { value: l.id }, l.name))),
          "Needed unless the artist is a wanted artist.",
        )
      : null;
  const profile = field(
    "song-profile",
    "Quality profile",
    h("select", {}, h("option", { value: "" }, "The artist's, else FLAC preferred"), profiles.map((p) => h("option", { value: p.id }, p.name))),
  );
  const fields = [title, artist, album, length, library, profile].filter(Boolean);
  const add = form(fields, "Add song", async () => {
    const got = { title: title.input.value.trim(), artist: artist.input.value.trim() };
    const secs = seconds(length.input.value);
    if (!got.title) title.fail("Enter the song's title.");
    if (!got.artist) artist.fail("Enter who it's by.");
    if (Number.isNaN(secs) || secs === 0) length.fail("Enter the length as minutes and seconds, like 4:23, or leave it empty.");
    if (!got.title || !got.artist || Number.isNaN(secs) || secs === 0) return null;
    const body = {
      ...got,
      ...(album.input.value.trim() ? { album: album.input.value.trim() } : {}),
      ...(secs ? { length_seconds: secs } : {}),
      ...(library?.input.value ? { library_id: Number(library.input.value) } : {}),
      ...(profile.input.value ? { music_profile_id: Number(profile.input.value) } : {}),
    };
    const added = await api("/wanted/songs", { method: "POST", body });
    ctx.reload(`Added “${added.title}” by ${added.artist}.`);
    return null;
  });

  return [
    group(
      "Add a song",
      h("p", { class: "note" }, "Songs are searched for on Soulseek only: torrents come as albums."),
      music.length ? add : h("p", { class: "status" }, "There's no music library to put songs in yet."),
    ),
    group(
      "Wanted songs",
      songs.length ? null : h("p", { class: "status" }, "No songs wanted yet."),
      ...filteredList(songs, {
        id: "song-filter",
        label: "Find a song",
        matches: (s, q) => `${s.title} ${s.artist} ${s.album ?? ""}`.toLowerCase().includes(q),
        row: (s) => songRow(ctx, s, libraries),
        noun: ["song", "songs"],
      }),
    ),
  ];
}
