// A title's page in Manage's Movies, Series and Anime: its header (poster, monitoring, state,
// profile, library, folder, size), what can be done with it (Search, Search and choose, Edit,
// Fix match, Open in Watch, Take off the list), its seasons and episodes for a series, its
// files, its downloads under way, its running-time checks and its history. Reached as `#/admin/<service>/title/<id>`,
// the id a wanted entry's, or "f" and a folder's for a title in the library that isn't wanted.

import { confirmInline, when } from "/web/forms.js";
import { SignedOut, announce, api, h, size } from "/web/lib.js";
import { emptyState, placeholders, stateLabel } from "/web/page-parts.js";
import { KINDS, fileRows, monitorSwitch, profilesFor, stateOf } from "/web/manage-parts.js";
import { searchAndChoose, searchAndGrab } from "/web/manage-search.js";
import { historyPart } from "/web/manage-history.js";
import { seasonsPart } from "/web/manage-seasons.js";
import { fixMatch } from "/web/settings.js";
import { art } from "/web/cards.js";

const DOWNLOAD_STATES = { downloading: "Downloading", completed: "On the seedbox", transferring: "Copying home", imported: "In the library", failed: "Failed" };

export async function titlePage(ctx) {
  const key = ctx.service.key;
  const kind = KINDS[key];
  const movies = key === "movies";
  const by = /^f\d+$/.test(ctx.id ?? "") ? { folder: ctx.id.slice(1) } : /^\d+$/.test(ctx.id ?? "") ? { wanted_id: ctx.id } : null;
  const page = by ? await api(`${kind.list}?${new URLSearchParams({ ...by, limit: 1 })}`) : { items: [] };
  const t = page.items[0];
  if (!ctx.current()) return [];
  if (!t) {
    return [h("h1", {}, "Not found"), emptyState(`${ctx.service.name} has no such title. It may have been taken off the list.`, { href: `#/admin/${key}/library`, label: `Go to ${ctx.service.name}` })];
  }
  const folderId = movies ? t.movie_id : t.series_id;
  const [folder, wanted, profiles, loads] = await Promise.all([
    folderId ? api(`/${movies ? "movies" : "series"}/${folderId}`).catch(() => null) : null,
    !movies && t.wanted ? api("/wanted/series").then((all) => all.find((w) => w.id === t.wanted_id) ?? null) : null,
    t.wanted ? profilesFor(kind).catch(() => []) : [],
    t.wanted ? api("/downloads") : [],
  ]);
  if (!ctx.current()) return [];
  const mine = loads.filter((d) => (movies ? d.wanted_movie_id : d.wanted_series_id) === t.wanted_id && d.state !== "imported");
  const v = { ctx, key, kind, movies, t, folder, wanted, profiles, downloads: mine };
  ctx.setTitle?.(t.title);
  return [
    header(v),
    movies ? null : seasonsPart(v),
    movies ? filesPart(v) : null,
    activityPart(v),
    runtimePart(v),
    t.wanted ? historyPart(ctx, key, t.wanted_id) : null,
  ];
}

// ---------- The header ----------

function header(v) {
  const { t, ctx } = v;
  const said = h("p", { class: "form-result", role: "status" });
  const panel = h("div", { class: "title-panel" });
  const monitor = t.wanted
    ? monitorSwitch({ ctx, label: `Monitor ${t.title}`, on: t.monitored, shown: true, said, change: (on) => api(`${v.kind.wanted}/${t.wanted_id}/monitored`, { method: "PUT", body: { monitored: on } }) })
    : null;
  const upgrade = t.cutoff_unmet ? " · upgrade wanted, below its profile's cutoff" : "";
  const folderPath = v.folder?.files?.[0]?.path.split("/").slice(0, -1).join("/") ?? null;
  const facts = [
    ["Profile", t.profile ?? (t.wanted ? "None chosen: searches are refused until one is" : "—")],
    ["Library", t.library ?? "—"],
    v.wanted ? ["Series type", { standard: "Standard", daily: "Daily", anime: "Anime" }[v.wanted.series_type] ?? v.wanted.series_type] : null,
    folderPath ? ["Folder", folderPath] : null,
    ["Size", t.size ? size(t.size) : "Nothing on disk"],
    t.quality ? ["Quality", t.quality] : null,
    t.added_at ? ["Added", when(t.added_at)] : null,
  ].filter(Boolean);
  // What a button opens (search results, Edit, Fix match) goes under the header, full width.
  return [
    h(
      "div",
      { class: "manage-head" },
      art(t.poster),
      h(
        "div",
        { class: "manage-head-facts" },
        h("div", { class: "title-line" }, h("h1", {}, t.title, t.year ? h("span", { class: "year" }, ` (${t.year})`) : null), monitor),
        h("p", { class: "title-state" }, stateOf(t), upgrade),
        t.wanted ? null : h("p", { class: "note" }, "Not on the wanted list, so Melete doesn't look for it or its upgrades. Add it under Add new."),
        h("dl", { class: "kv" }, facts.map(([k, val]) => [h("dt", {}, k), h("dd", {}, val)])),
        toolbar(v, panel, said),
        said,
      ),
    ),
    panel,
  ];
}

function toolbar(v, panel, said) {
  const { t, ctx, key, kind, movies } = v;
  const bar = h("div", { class: "row-actions title-actions" });
  const open = (node) => {
    panel.replaceChildren(node);
    node.querySelector("h2, h3, select, input, button")?.focus();
  };
  const ask = { path: `${kind.wanted}/${t.wanted_id}/search`, body: undefined, label: t.title };
  const buttons = [];
  if (t.wanted && movies) {
    const search = h("button", { type: "button" }, "Search");
    search.addEventListener("click", async () => {
      search.disabled = true;
      await searchAndGrab(ctx, ask, said);
      search.disabled = false;
    });
    const choose = h("button", { type: "button", class: "quiet" }, "Search and choose");
    choose.addEventListener("click", () => {
      const box = h("section", { class: "choose-box", "aria-label": "Search and choose" });
      panel.replaceChildren(box);
      searchAndChoose(ctx, ask, box);
    });
    buttons.push(search, choose);
  }
  if (t.wanted) buttons.push(h("button", { type: "button", class: "quiet", onclick: () => open(editPanel(v, said)) }, "Edit…"));
  if (v.folder) buttons.push(h("button", { type: "button", class: "quiet", onclick: () => open(matchPanel(v)) }, "Fix match…"));
  const watch = kind.watch(t);
  if (watch) buttons.push(h("a", { class: "button quiet", href: watch }, "Open in Watch"));
  if (t.wanted) {
    const off = h("button", { type: "button", class: "quiet" }, "Take off the list…");
    off.addEventListener("click", () =>
      confirmInline(bar, {
        question: `Take ${t.title} off the wanted list? Melete stops looking for it and its upgrades; its files in the library stay, and it can be added again.`,
        confirm: "Take off",
        run: async () => {
          await api(`${kind.wanted}/${t.wanted_id}`, { method: "DELETE" });
          announce(`Took ${t.title} off the wanted list`);
          location.hash = `#/admin/${key}/library`;
        },
      }),
    );
    buttons.push(off);
  }
  bar.append(...buttons);
  return bar;
}

// Edit: the profile it's searched and decided with.
function editPanel(v, said) {
  const { t, kind } = v;
  const id = "title-profile";
  const pick = h("select", { id }, h("option", { value: "" }, kind.profiles.source ? "The one the arr names, if any" : "The one Lidarr names, if any"), v.profiles.map((p) => h("option", { value: String(p.id), selected: p.id === t.profile_id }, p.name)));
  const save = h("button", { type: "submit" }, "Save");
  const result = h("p", { class: "form-result", role: "status" });
  const form = h("form", { class: "edit-title-form", novalidate: true }, h("h2", { class: "part", tabindex: "-1" }, "Edit"), h("div", { class: "field" }, h("label", { for: id }, "Profile"), pick), h("div", { class: "form-actions" }, save, result));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    save.disabled = true;
    try {
      await api(`${kind.wanted}/${t.wanted_id}/profile`, { method: "PUT", body: { profile_id: pick.value ? Number(pick.value) : null } });
      result.textContent = "Saved. Searches and RSS syncs use it from now on.";
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

// Fix match: search TMDB for it, choose an entry, or say it has none.
function matchPanel(v) {
  const f = v.folder;
  const item = { kind: v.movies ? "movie" : "series", id: f.id, rel_path: f.title, parsed_title: f.title, parsed_year: f.year, library: f.library, status: String(f.match_status), candidates: [] };
  return h(
    "section",
    { class: "match-box", "aria-labelledby": "match-title" },
    h("h2", { class: "part", id: "match-title", tabindex: "-1" }, "Fix match"),
    h("p", { class: "note" }, "A match made here is final: automatic matching leaves it alone. Nothing on disk is renamed or moved."),
    h(
      "ul",
      { class: "rows" },
      fixMatch(v.ctx, item, (message) => {
        announce(message);
        v.ctx.reload(message);
      }),
    ),
  );
}

// ---------- Files ----------

function filesPart(v) {
  const files = v.folder?.files ?? [];
  const box = h("div", {});
  if (!files.length) {
    box.append(emptyState(v.t.wanted ? "Not in the library yet. Once it's grabbed and imported, its file shows here." : "No files.", null));
  } else {
    box.append(placeholders("its files", { kind: "rows", count: files.length }));
    const several = files.length > 1 ? h("p", { class: "note" }, "An upgrade replaces only one of them; the file it replaces goes to the trash for 30 days.") : null;
    fileRows(files).then((table) => v.ctx.current() && box.replaceChildren(...[table, several].filter(Boolean)));
  }
  return [h("h2", { class: "part" }, "Files"), box];
}

// ---------- Activity ----------

// Downloads under way for it, or failed, each with how far it got and why it stopped.
function activityPart(v) {
  if (!v.t.wanted) return null;
  const rows = v.downloads.map((d) =>
    h(
      "li",
      {},
      h(
        "div",
        {},
        h("span", { class: "path" }, d.title),
        h("p", {}, d.state === "failed" ? stateLabel("failed", `Failed${d.message ? `: ${d.message}` : ""}`) : stateLabel("coming", `${DOWNLOAD_STATES[d.state] ?? d.state}${d.state === "downloading" ? ` ${Math.round(d.progress)}%` : ""}`)),
        d.season !== null && !v.movies ? h("p", { class: "note" }, d.episode === null ? `Season ${d.season}` : `Season ${d.season}, episode ${d.episode}`) : null,
      ),
    ),
  );
  return [
    h("h2", { class: "part" }, "Downloads"),
    rows.length ? h("ul", { class: "rows" }, rows) : h("p", { class: "status" }, "Nothing on its way."),
    rows.length ? h("p", { class: "note" }, h("a", { href: `#/admin/${v.key}/activity` }, "Remove or search again under Activity")) : null,
  ];
}

// ---------- Running-time checks ----------

// Downloads held back because they didn't run as long as TMDB says, and "The runtime is right
// after all", which lets Melete search for the title again. Drawn only when there's something.
function runtimePart(v) {
  if (!v.t.wanted) return null;
  const box = h("div", {});
  const uri = `/runtime-checks/${v.movies ? "movie" : "series"}/${v.t.wanted_id}`;
  const draw = (s, said) => {
    if (!s.failures && !s.overridden) return box.replaceChildren();
    const actions = h("div", { class: "row-actions" });
    if (s.failures) {
      const right = h("button", { type: "button", class: "quiet" }, "The runtime is right after all");
      right.addEventListener("click", () =>
        confirmInline(actions, {
          question: v.movies
            ? "Check this film's downloads against how long the last held-back one ran, instead of TMDB's runtime, until TMDB's changes? Melete then searches for it by itself again."
            : "Stop checking this series' downloads against TMDB's episode runtimes until they change? Melete then searches for it by itself again.",
          confirm: "The runtime is right",
          keep: "Keep checking",
          run: async () => {
            draw(await api(`${uri}/right`, { method: "POST" }), "Saved. Melete searches for it by itself again.");
            announce("Saved. Melete searches for it by itself again.");
          },
        }),
      );
      actions.append(right);
    }
    const lead = s.stopped
      ? `Melete stopped searching for it by itself: ${s.failures} downloads didn't pass the running-time check.`
      : s.failures
        ? `${s.failures === 1 ? "1 download" : `${s.failures} downloads`} didn't pass the running-time check. After 3, Melete stops searching for it by itself.`
        : null;
    const held = s.overridden
      ? s.override_seconds
        ? `An admin said its runtime is right: downloads are checked against ${Math.round(s.override_seconds / 60)} minutes until TMDB's runtime changes.`
        : "An admin said its runtime is right: its downloads aren't checked until TMDB's runtime changes."
      : null;
    const parts = [
      h("h2", { class: "part" }, "Running-time checks"),
      lead ? h("p", {}, s.stopped ? stateLabel("failed", lead) : lead) : null,
      s.reasons.length
        ? h("ul", { class: "rows" }, s.reasons.map((f) => h("li", {}, h("div", {}, h("span", { class: "path" }, f.release), h("p", { class: "note" }, `${f.reason} (${when(f.at)})`)))))
        : null,
      held ? h("p", { class: "note" }, held) : null,
      actions,
      h("p", { class: "form-result", role: "status" }, said ?? ""),
    ];
    box.replaceChildren(...parts.filter(Boolean));
  };
  api(uri)
    .then((s) => draw(s))
    .catch((err) => {
      if (err instanceof SignedOut) return;
      box.replaceChildren(h("p", { class: "status" }, `Its running-time checks couldn't be read. ${err.message}`));
    });
  return box;
}
