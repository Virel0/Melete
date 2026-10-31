// A result in Add new and what adding it asks: how it stands here (in the library, wanted, asked
// for already), its poster or cover when Melete has one, and Add…, which opens where it goes, its
// profile, whether it's monitored and searched for now, and a series' seasons. A song found on
// Soulseek, which MusicBrainz doesn't know, is added by its title and artist, both editable. An
// admin's add is a request approved at once, as every add is.

import { SignedOut, announce, api, h } from "/web/lib.js";
import { stateLabel } from "/web/page-parts.js";
import { art } from "/web/cards.js";

// Forms for results with no id of their own (songs found on Soulseek), told apart by number.
let unnamed = 0;

// One result, how it stands here, and Add… to choose where it goes.
export function candidateRow(page, c) {
  const form = h("div", {});
  const marks = [
    c.in_library ? stateLabel("have", "In the library") : null,
    c.wanted ? stateLabel("coming", "Wanted") : null,
    c.requested ? stateLabel("waiting", "Asked for already") : null,
  ].filter(Boolean);
  const add = h("button", { type: "button", class: "quiet find-add", "aria-expanded": "false", "aria-label": `Add ${c.title}${c.artist ? ` by ${c.artist}` : ""}…` }, "Add…");
  add.addEventListener("click", () => {
    const open = add.getAttribute("aria-expanded") !== "true";
    add.setAttribute("aria-expanded", String(open));
    form.replaceChildren(...(open ? [addForm(page, c)] : []));
    if (open) form.querySelector("select, input, button")?.focus();
  });
  const picture = art(c.poster);
  if (c.kind !== "movie" && c.kind !== "series") picture.classList.add("square");
  return h(
    "li",
    { class: "find-row" },
    picture,
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
export function addForm(page, c) {
  const id = `add-${c.kind}-${c.tmdb_id ?? c.mb_recording_id ?? c.mb_release_group_id ?? c.mb_artist_id ?? `n${++unnamed}`}`;
  // A song MusicBrainz doesn't know is asked for by its title and artist, as its files name them.
  const byName = c.kind === "song" && !c.mb_recording_id;
  const named = byName ? [["title", "Song title", c.title], ["artist", "Artist", c.artist ?? ""]].map(([key, words, value]) => {
    const input = h("input", { id: `${id}-${key}`, value, autocomplete: "off", maxlength: 300, required: true });
    return { key, input, box: h("div", { class: "field" }, h("label", { for: input.id }, words), input) };
  }) : [];
  const music = page.key === "music";
  // A song isn't monitored: it's searched for on Soulseek, once.
  const song = c.kind === "song";
  const library = page.libraries.length > 1 ? h("select", { id: `${id}-library` }, h("option", { value: "" }, "As Where they go says"), page.libraries.map((l) => h("option", { value: String(l.id) }, l.name))) : null;
  const profile = h("select", { id: `${id}-profile` }, h("option", { value: "" }, "As Where they go says"), page.profiles.map((p) => h("option", { value: String(p.id) }, p.name)));
  const monitored = song ? null : h("input", { type: "checkbox", id: `${id}-monitored`, checked: true });
  const now = h("input", { type: "checkbox", id: `${id}-now`, checked: true });
  const seasons = c.kind === "series" ? h("input", { id: `${id}-seasons`, inputmode: "numeric", placeholder: "1, 2", "aria-describedby": `${id}-seasons-hint` }) : null;
  const said = h("p", { class: "form-result", role: "status" });
  const submit = h("button", { type: "submit" }, `Add ${c.title}`);
  const form = h(
    "form",
    { class: "add-form", novalidate: true },
    named.map((n) => n.box),
    library ? h("div", { class: "field" }, h("label", { for: library.id }, "Library"), library) : null,
    h("div", { class: "field" }, h("label", { for: profile.id }, music ? "Music profile" : "Profile"), profile),
    seasons ? h("div", { class: "field" }, h("label", { for: seasons.id }, "Seasons to monitor"), h("p", { class: "note", id: `${id}-seasons-hint` }, "Season numbers with commas; leave empty for every season but the specials."), seasons) : null,
    monitored ? h("div", { class: "check" }, monitored, h("label", { for: monitored.id }, "Monitored: looked for by searches and RSS syncs")) : null,
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
    const empty = named.find((n) => !n.input.value.trim());
    if (empty) {
      said.textContent = `Enter the song's ${empty.key}.`;
      return empty.input.focus();
    }
    const body = {
      kind: c.kind,
      title: byName ? named[0].input.value.trim() : undefined,
      artist: byName ? named[1].input.value.trim() : undefined,
      tmdb_id: c.tmdb_id ?? undefined,
      mb_artist_id: c.kind === "artist" ? c.mb_artist_id : undefined,
      mb_release_group_id: c.kind === "album" ? c.mb_release_group_id : undefined,
      mb_recording_id: song && !byName ? c.mb_recording_id : undefined,
      library_id: library?.value ? Number(library.value) : undefined,
      [music ? "music_profile_id" : "profile_id"]: profile.value ? Number(profile.value) : undefined,
      monitored: monitored?.checked,
      search_now: now.checked,
      seasons: chosen ?? undefined,
    };
    submit.disabled = true;
    said.textContent = "Adding…";
    try {
      const r = await api("/requests", { method: "POST", body });
      said.replaceChildren(...added(page, { ...c, title: body.title ?? c.title }, r));
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
  const pages = { artist: `#/admin/music/artist/${r.wanted_id}`, album: null, song: "#/admin/music/songs" };
  const where = c.kind in pages ? pages[c.kind] : `#/admin/${service}/title/${r.wanted_id}`;
  const searched = r.search_started ? (c.kind === "song" ? " Searching Soulseek for it now." : " Searching for it now.") : "";
  const open = c.kind === "song" ? "Open Wanted songs" : "Open its page";
  return [`Added ${c.title}${c.kind === "album" ? ", with its artist" : ""}.${searched} `, where ? h("a", { href: where }, open) : null].filter(Boolean);
}
