// Add new, in Manage's Movies, Series, Anime and Music: search TMDB or MusicBrainz, each result
// marked in the library, wanted or asked for already, and add one with where it goes, its
// profile, whether it's monitored and searched for at once (and a series' seasons). An admin's
// add is a request approved at once, as every add is, so it follows the same path as anyone's.
// Then the import from Radarr, Sonarr or Lidarr, which brings their whole list. And Needs a
// match: the service's titles TMDB or MusicBrainz couldn't settle, matched by hand.

import { group } from "/web/forms.js";
import { SignedOut, announce, api, h } from "/web/lib.js";
import { emptyState, stateLabel } from "/web/page-parts.js";
import { KINDS, profilesFor } from "/web/manage-parts.js";
import { arrs } from "/web/sources-arrs.js";
import { lidarrArtists, musicMatching } from "/web/admin-music.js";
import { matching } from "/web/settings.js";

// What each service searches for: [kind, words] pairs, the first the default.
const SEARCHES = {
  movies: [["movie", "Movies"]],
  series: [["series", "Series"]],
  anime: [["series", "Series"]],
  music: [
    ["artist", "Artists"],
    ["album", "Albums"],
  ],
};
const LIBRARY_KIND = { movies: "movies", series: "shows", anime: "shows", music: "music" };

export async function addPage(ctx) {
  const key = ctx.service.key;
  const [libraries, profiles] = await Promise.all([api("/libraries"), profilesFor(KINDS[key]).catch(() => [])]);
  if (!ctx.current()) return [];
  // Anime offers its anime libraries first; Series leaves them out.
  const mine = libraries
    .filter((l) => l.kind === LIBRARY_KIND[key] && !(key === "series" && l.category === "anime"))
    .sort((a, b) => (key === "anime" ? (b.category === "anime") - (a.category === "anime") : 0));
  const page = { ctx, key, libraries: mine, profiles };
  const imports = key === "music" ? [lidarrArtists()] : await arrs({ ...ctx, kind: key === "movies" ? "movies" : "series" });
  return [searchPart(page), ...imports];
}

function searchPart(page) {
  const kinds = SEARCHES[page.key];
  const q = h("input", { id: "add-q", type: "search", autocomplete: "off", spellcheck: "false", required: true });
  const kind = kinds.length > 1 ? h("select", { id: "add-kind" }, kinds.map(([k, words]) => h("option", { value: k }, words))) : null;
  const results = h("div", { class: "add-results", "aria-live": "polite" });
  const search = h(
    "form",
    { class: "search-row", role: "search", novalidate: true },
    h("div", {}, h("label", { for: "add-q" }, page.key === "music" ? "Artist or album" : "Title"), q),
    kind ? h("div", {}, h("label", { for: "add-kind" }, "Look for"), kind) : null,
    h("button", { type: "submit" }, page.key === "music" ? "Search MusicBrainz" : "Search TMDB"),
  );
  search.addEventListener("submit", async (event) => {
    event.preventDefault();
    const words = q.value.trim();
    if (!words) return q.focus();
    const asked = kind?.value ?? kinds[0][0];
    results.replaceChildren(h("p", { class: "status" }, "Searching…"));
    try {
      const found = await api(`/requests/search?${new URLSearchParams({ kind: asked, q: words })}`);
      if (!page.ctx.current()) return;
      results.replaceChildren(
        found.length
          ? h("ul", { class: "rows" }, found.map((c) => candidateRow(page, c)))
          : emptyState(`Nothing called “${words}” was found. Try another spelling, or fewer words.`, null),
      );
    } catch (err) {
      if (err instanceof SignedOut) return page.ctx.signedOut();
      results.replaceChildren(h("p", { class: "error", role: "alert" }, err.message), h("p", { class: "note" }, "How TMDB and MusicBrainz stand is under ", h("a", { href: "#/admin/system/services" }, "System, Outside services"), "."));
    }
  });
  return group(
    "Add new",
    h("p", { class: "note" }, `Only the words you search for leave the server. Adding one puts it on the wanted list at once, as an approved request.`),
    search,
    results,
  );
}

// One result, how it stands here, and Add… to choose where it goes.
function candidateRow(page, c) {
  const form = h("div", {});
  const marks = [
    c.in_library ? stateLabel("have", "In the library") : null,
    c.wanted ? stateLabel("coming", "Wanted") : null,
    c.requested ? stateLabel("waiting", "Asked for already") : null,
  ].filter(Boolean);
  const add = h("button", { type: "button", class: "quiet", "aria-expanded": "false" }, "Add…");
  add.addEventListener("click", () => {
    const open = add.getAttribute("aria-expanded") !== "true";
    add.setAttribute("aria-expanded", String(open));
    form.replaceChildren(...(open ? [addForm(page, c)] : []));
    if (open) form.querySelector("select, input, button")?.focus();
  });
  return h(
    "li",
    {},
    h(
      "div",
      {},
      h("strong", {}, c.title),
      c.year ? ` (${c.year})` : null,
      c.artist ? h("span", { class: "note" }, ` by ${c.artist}`) : null,
      c.detail ? h("p", { class: "note" }, c.detail) : null,
      marks.length ? h("p", { class: "marks" }, marks) : null,
      form,
    ),
    h("div", { class: "row-actions" }, add),
  );
}

// Where it goes, its profile, monitored, searched for now, and a series' seasons.
function addForm(page, c) {
  const id = `add-${c.kind}-${c.tmdb_id ?? c.mb_artist_id ?? c.mb_release_group_id}`;
  const music = page.key === "music";
  const library = page.libraries.length > 1 ? h("select", { id: `${id}-library` }, h("option", { value: "" }, "As Where they go says"), page.libraries.map((l) => h("option", { value: String(l.id) }, l.name))) : null;
  const profile = h("select", { id: `${id}-profile` }, h("option", { value: "" }, "As Where they go says"), page.profiles.map((p) => h("option", { value: String(p.id) }, p.name)));
  const monitored = h("input", { type: "checkbox", id: `${id}-monitored`, checked: true });
  const now = h("input", { type: "checkbox", id: `${id}-now`, checked: true });
  const seasons = c.kind === "series" ? h("input", { id: `${id}-seasons`, inputmode: "numeric", placeholder: "1, 2", "aria-describedby": `${id}-seasons-hint` }) : null;
  const said = h("p", { class: "form-result", role: "status" });
  const submit = h("button", { type: "submit" }, `Add ${c.title}`);
  const form = h(
    "form",
    { class: "add-form", novalidate: true },
    library ? h("div", { class: "field" }, h("label", { for: library.id }, "Library"), library) : null,
    h("div", { class: "field" }, h("label", { for: profile.id }, music ? "Music profile" : "Profile"), profile),
    seasons ? h("div", { class: "field" }, h("label", { for: seasons.id }, "Seasons to monitor"), h("p", { class: "note", id: `${id}-seasons-hint` }, "Season numbers with commas; leave empty for every season but the specials."), seasons) : null,
    h("div", { class: "check" }, monitored, h("label", { for: monitored.id }, "Monitored: looked for by searches and RSS syncs")),
    h("div", { class: "check" }, now, h("label", { for: now.id }, "Search for it now")),
    h("div", { class: "form-actions" }, submit, said),
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const chosen = seasons?.value.trim() ? seasons.value.split(/[\s,]+/).filter(Boolean).map(Number) : null;
    if (chosen?.some((n) => !Number.isInteger(n) || n < 0)) {
      said.textContent = "Seasons are whole numbers, 0 for the specials, with commas between.";
      return seasons.focus();
    }
    const body = {
      kind: c.kind,
      tmdb_id: c.tmdb_id ?? undefined,
      mb_artist_id: c.kind === "artist" ? c.mb_artist_id : undefined,
      mb_release_group_id: c.kind === "album" ? c.mb_release_group_id : undefined,
      library_id: library?.value ? Number(library.value) : undefined,
      [music ? "music_profile_id" : "profile_id"]: profile.value ? Number(profile.value) : undefined,
      monitored: monitored.checked,
      search_now: now.checked,
      seasons: chosen ?? undefined,
    };
    submit.disabled = true;
    said.textContent = "Adding…";
    try {
      const r = await api("/requests", { method: "POST", body });
      said.replaceChildren(...added(page, c, r));
      announce(said.textContent);
    } catch (err) {
      if (err instanceof SignedOut) return page.ctx.signedOut();
      said.textContent = err.message;
      submit.disabled = false;
    }
  });
  return form;
}

// What adding did: on the list (with a link to its page), or waiting for a place.
function added(page, c, r) {
  if (r.state !== "approved" || r.wanted_id === null) return [`Asked for, but it couldn't be placed: choose where it goes under Requests, To decide.`];
  const service = r.service === "anime" || r.service === "series" ? r.service : page.key;
  const where = c.kind === "artist" ? `#/admin/music/artist/${r.wanted_id}` : c.kind === "album" ? null : `#/admin/${service}/title/${r.wanted_id}`;
  const searched = r.search_started ? " Searching for it now." : "";
  return [`Added ${c.title}${c.kind === "album" ? ", with its artist" : ""}.${searched} `, where ? h("a", { href: where }, "Open its page") : null].filter(Boolean);
}

// Needs a match: the titles of this service TMDB couldn't settle (a series in an anime library
// is Anime's), or the albums MusicBrainz couldn't, for Music.
export async function needsMatch(ctx) {
  const key = ctx.service.key;
  if (key === "music") return musicMatching(ctx);
  const anime = new Set((await api("/libraries")).filter((l) => l.category === "anime").map((l) => l.name));
  const only = {
    movies: (item) => item.kind === "movie",
    series: (item) => item.kind === "series" && !anime.has(item.library),
    anime: (item) => item.kind === "series" && anime.has(item.library),
  }[key];
  return matching({ ...ctx, matchOnly: only });
}
