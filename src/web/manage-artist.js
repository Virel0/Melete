// An artist's page in Manage → Music: monitoring, the rule for albums they release later, the
// profile, Search every album, Edit, Open in Listen and Take off the list; then their albums
// grouped by type, each monitored, searched and chosen on its own and opening onto its page;
// the albums of theirs in the library that aren't wanted; and their history. Reached as
// `#/admin/music/artist/<wanted id>`, or "f" and a library artist's id when they aren't wanted.

import { confirmInline, when } from "/web/forms.js";
import { SignedOut, airDate, announce, api, h, plural, size } from "/web/lib.js";
import { emptyState, stateLabel } from "/web/page-parts.js";
import { KINDS, monitorSwitch, profilesFor, stateOf } from "/web/manage-parts.js";
import { searchAndChoose, searchAndGrab } from "/web/manage-search.js";
import { historyPart } from "/web/manage-history.js";
import { searchEveryAlbum } from "/web/wanted-music.js";
import { art } from "/web/cards.js";

const NEW = [
  ["all", "Every album they release from now on, and older ones MusicBrainz adds"],
  ["new", "Albums released from when Melete first looked"],
  ["none", "None: only the albums wanted now"],
];

// The groups albums are shown in, in this order; a secondary type wins over the primary one.
const GROUPS = [
  ["Albums", (a) => a.album_type === "Album"],
  ["EPs", (a) => a.album_type === "EP"],
  ["Singles", (a) => a.album_type === "Single"],
  ["Compilations", (a) => a.secondary_types.includes("Compilation")],
  ["Live", (a) => a.secondary_types.includes("Live")],
  ["Other", () => true],
];

function groupOf(album) {
  const second = ["Compilation", "Live"].find((t) => album.secondary_types.includes(t));
  if (second) return second === "Live" ? "Live" : "Compilations";
  return GROUPS.find(([, fits]) => fits(album))[0];
}

export async function artistPage(ctx) {
  const kind = KINDS.music;
  const by = /^f\d+$/.test(ctx.id ?? "") ? { folder: ctx.id.slice(1) } : /^\d+$/.test(ctx.id ?? "") ? { wanted_id: ctx.id } : null;
  const page = by ? await api(`${kind.list}?${new URLSearchParams({ ...by, limit: 1 })}`) : { items: [] };
  const t = page.items[0];
  if (!ctx.current()) return [];
  if (!t) return [h("h1", {}, "Not found"), emptyState("Music has no such artist. They may have been taken off the list.", { href: "#/admin/music/library", label: "Go to Music" })];
  const [albums, here, profiles, loads] = await Promise.all([
    t.wanted ? api(`/wanted/artists/${t.wanted_id}/albums`) : [],
    t.artist_id ? api(`/music/artists/${t.artist_id}`).catch(() => null) : null,
    t.wanted ? profilesFor(kind).catch(() => []) : [],
    t.wanted ? api("/downloads") : [],
  ]);
  if (!ctx.current()) return [];
  const ids = new Set(albums.map((a) => a.id));
  const v = { ctx, kind, t, albums, here, profiles, downloads: loads.filter((d) => ids.has(d.wanted_album_id) && d.state !== "imported") };
  ctx.setTitle(t.title);
  return [header(v), albumsPart(v), unwanted(v), t.wanted ? historyPart(ctx, "artists", t.wanted_id) : null];
}

function header(v) {
  const { t, ctx } = v;
  const said = h("p", { class: "form-result", role: "status" });
  const panel = h("div", { class: "title-panel" });
  const monitor = t.wanted
    ? monitorSwitch({ ctx, label: `Monitor ${t.title}`, on: t.monitored, shown: true, said, change: (on) => api(`/wanted/artists/${t.wanted_id}/monitored`, { method: "PUT", body: { monitored: on } }) })
    : null;
  const facts = [
    t.wanted ? ["New albums", newAlbums(v, said)] : null,
    ["Profile", t.profile ?? (t.wanted ? "None chosen: searches are refused until one is" : "—")],
    ["Library", t.library ?? "—"],
    ["Size", t.size ? size(t.size) : "Nothing on disk"],
    t.added_at ? ["Added", when(t.added_at)] : null,
  ].filter(Boolean);
  const box = art(t.poster);
  box.classList.add("square");
  return [
    h(
      "div",
      { class: "manage-head" },
      box,
      h(
        "div",
        { class: "manage-head-facts" },
        h("div", { class: "title-line" }, h("h1", {}, t.title), monitor),
        h("p", { class: "title-state" }, stateOf(t)),
        t.wanted ? null : h("p", { class: "note" }, "Not on the wanted list, so Melete doesn't look for their albums. Add them under Add new."),
        h("dl", { class: "kv" }, facts.map(([k, val]) => [h("dt", {}, k), h("dd", {}, val)])),
        toolbar(v, panel, said),
        said,
      ),
    ),
    panel,
  ];
}

// The rule for albums they release later, changed as soon as it's chosen.
function newAlbums(v, said) {
  const { t } = v;
  const pick = h("select", { class: "compact", "aria-label": "New albums" }, NEW.map(([key, words]) => h("option", { value: key, selected: key === t.monitor_new }, words)));
  pick.addEventListener("change", async () => {
    pick.disabled = true;
    try {
      await api(`/wanted/artists/${t.wanted_id}`, { method: "PUT", body: { monitor_new: pick.value } });
      said.textContent = "New albums: saved. Followed while MusicBrainz is on.";
    } catch (err) {
      if (err instanceof SignedOut) return v.ctx.signedOut();
      said.textContent = err.message;
      pick.value = t.monitor_new;
    } finally {
      pick.disabled = false;
    }
  });
  return pick;
}

function toolbar(v, panel, said) {
  const { t, ctx, kind } = v;
  const bar = h("div", { class: "row-actions title-actions" });
  const open = (node) => {
    panel.replaceChildren(...[node].flat());
    panel.querySelector("h2, select, input, button")?.focus();
  };
  if (t.wanted) {
    bar.append(
      h("button", { type: "button", onclick: () => {
        const { searchAll, run } = searchEveryAlbum(ctx, { id: t.wanted_id, name: t.title });
        open([h("h2", { class: "part", tabindex: "-1" }, "Search every album"), h("p", { class: "note" }, "Every monitored album that isn't on disk or on its way, oldest first, on the indexers and Soulseek. It goes on if you leave."), searchAll, run]);
      } }, "Search every album…"),
      h("button", { type: "button", class: "quiet", onclick: () => open(editPanel(v, said)) }, "Edit…"),
    );
  }
  const listen = kind.watch(t);
  if (listen) bar.append(h("a", { class: "button quiet", href: listen }, "Open in Listen"));
  if (t.wanted) {
    const off = h("button", { type: "button", class: "quiet" }, "Take off the list…");
    off.addEventListener("click", () =>
      confirmInline(bar, {
        question: `Take ${t.title} off the wanted list, with their albums? Melete stops looking for them; their files in the library stay, and they can be added again.`,
        confirm: "Take off",
        run: async () => {
          await api(`/wanted/artists/${t.wanted_id}`, { method: "DELETE" });
          announce(`Took ${t.title} off the wanted list`);
          location.hash = "#/admin/music/library";
        },
      }),
    );
    bar.append(off);
  }
  return bar;
}

function editPanel(v, said) {
  const { t } = v;
  const id = "artist-profile";
  const pick = h("select", { id }, h("option", { value: "" }, "The one Lidarr names, if any"), v.profiles.map((p) => h("option", { value: String(p.id), selected: p.id === t.profile_id }, p.name)));
  const save = h("button", { type: "submit" }, "Save");
  const result = h("p", { class: "form-result", role: "status" });
  const form = h("form", { class: "edit-title-form", novalidate: true }, h("h2", { class: "part", tabindex: "-1" }, "Edit"), h("div", { class: "field" }, h("label", { for: id }, "Music profile"), pick), h("div", { class: "form-actions" }, save, result));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    save.disabled = true;
    try {
      await api(`/wanted/artists/${t.wanted_id}/profile`, { method: "PUT", body: { profile_id: pick.value ? Number(pick.value) : null } });
      result.textContent = "Saved. Their albums are decided with it from now on.";
      said.textContent = "";
    } catch (err) {
      if (err instanceof SignedOut) return v.ctx.signedOut();
      result.textContent = err.message;
    } finally {
      save.disabled = false;
    }
  });
  return form;
}

// ---------- Albums ----------

const today = () => new Date().toISOString().slice(0, 10);

// An album's state: on disk, on its way, not out yet, not monitored, or missing.
export function albumState(v, a) {
  if (a.album_id) return ["have", "In the library"];
  if (v.downloads.some((d) => d.wanted_album_id === a.id && d.state !== "failed")) return ["coming", "On its way"];
  if (!a.release_date || a.release_date > today()) return ["notyet", a.release_date ? `Out ${airDate(a.release_date.padEnd(10, "-01").slice(0, 10))}` : "Not out yet"];
  if (!a.monitored || !v.t.monitored) return ["off", "Not monitored"];
  return ["missing", "Missing"];
}

function albumsPart(v) {
  if (!v.t.wanted) return null;
  if (!v.albums.length) return [h("h2", { class: "part" }, "Albums"), emptyState("No albums of theirs are wanted yet. New ones join as the rule above says.", null)];
  const groups = new Map();
  for (const a of [...v.albums].sort((x, y) => (x.release_date ?? "9999").localeCompare(y.release_date ?? "9999"))) {
    const name = groupOf(a);
    groups.set(name, [...(groups.get(name) ?? []), a]);
  }
  const results = h("div", {});
  const order = GROUPS.map(([name]) => name).filter((name) => groups.has(name));
  return [
    h("h2", { class: "part" }, "Albums"),
    order.map((name) => albumGroup(v, name, groups.get(name), results)),
    results,
  ];
}

function albumGroup(v, name, albums, results) {
  const { t, ctx } = v;
  const onDisk = albums.filter((a) => a.album_id).length;
  const cell = (label, content, cls) => h("td", { "data-label": label, class: cls }, content);
  return h(
    "section",
    { class: "season", "aria-label": name },
    h("header", {}, h("h3", {}, name), stateLabel(onDisk === albums.length ? "have" : "missing", `${onDisk} of ${plural(albums.length, "album", "albums")} on disk`)),
    h(
      "div",
      { class: "table-wrap stacks" },
      h(
        "table",
        { class: "data" },
        h("caption", { class: "visually-hidden" }, name),
        h("thead", {}, h("tr", {}, ["Monitored", "Album", "Released", "State", "Search"].map((c) => h("th", { scope: "col" }, c)))),
        h(
          "tbody",
          {},
          albums.map((a) => {
            const [mark, words] = albumState(v, a);
            const said = h("span", { class: "note", role: "status" });
            const ask = { path: `/wanted/albums/${a.id}/search`, body: undefined, label: `${a.title} by ${t.title}` };
            return h(
              "tr",
              {},
              h("td", { class: "tick" }, monitorSwitch({ ctx, label: `Monitor ${a.title}`, on: a.monitored, said, change: (on) => api(`/wanted/albums/${a.id}/monitored`, { method: "PUT", body: { monitored: on } }) })),
              h("th", { scope: "row" }, h("a", { href: `#/admin/music/album/${a.id}?artist=${t.wanted_id}` }, a.title), a.secondary_types.length ? h("span", { class: "note" }, ` ${a.secondary_types.join(", ")}`) : null),
              cell("Released", a.release_date ?? "—"),
              cell("State", stateLabel(mark, words)),
              cell(
                "Search",
                h(
                  "div",
                  { class: "row-actions" },
                  h("button", { type: "button", class: "quiet", "aria-label": `Search for ${a.title}`, onclick: () => searchAndGrab(ctx, ask, said) }, "Search"),
                  h("button", { type: "button", class: "quiet", "aria-label": `Search and choose for ${a.title}`, onclick: () => searchAndChoose(ctx, ask, results) }, "Choose"),
                  said,
                ),
              ),
            );
          }),
        ),
      ),
    ),
  );
}

// Albums of theirs in the library that no wanted album is.
function unwanted(v) {
  const linked = new Set(v.albums.map((a) => a.album_id).filter(Boolean));
  const rest = (v.here?.album_list ?? []).filter((a) => !linked.has(a.id));
  if (!rest.length) return null;
  return [
    h("h2", { class: "part" }, v.t.wanted ? "In the library, not wanted" : "In the library"),
    h(
      "ul",
      { class: "rows" },
      rest.map((a) => h("li", {}, h("div", {}, h("a", { href: `#/admin/music/album/f${a.id}` }, a.title), a.year ? h("span", { class: "note" }, ` ${a.year}`) : null, h("p", { class: "note" }, plural(a.tracks, "track", "tracks"))))),
    ),
  ];
}
