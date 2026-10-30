// Wanted movies and series, for admins, and a search's results with a Grab button on each,
// which the downloads, RSS and music sections show too. The Wanted page's sections are listed
// in wanted-page.js.

import { confirmInline, group, when } from "/web/forms.js";
import { SignedOut, announce, api, h, number, plural, seasonName, size } from "/web/lib.js";
import { searchMissing } from "/web/wanted-missing.js";
import { tryLink } from "/web/release-try.js";

const PAGE = 50;

// ---------- A search's results ----------

// Every result with its decision, accepted ones first, each accepted one grabbable. A video's
// decision has a score, custom formats and rejections; an album's or song's a rank and reasons.
export function searchResults(search, ctx) {
  const asked = search.indexers.map((i) =>
    i.error ? `${i.name}: ${i.error}` : `${i.name}: ${plural(i.results, "result", "results")}`,
  );
  if (search.soulseek) {
    const s = search.soulseek;
    asked.push(s.error ? `Soulseek: ${s.error}` : `Soulseek: ${plural(s.results, "result", "results")}`);
  }
  const accepted = search.releases.filter((r) => r.accepted).length;
  const rows = search.releases.map((r) => {
    const d = r.decision;
    const result = h("span", { class: "note", role: "status" });
    let grab = null;
    const blocked = h("span", { class: "badge state-failed" }, "Blocklisted");
    if (r.accepted && !r.blocklisted) {
      grab = h("button", { type: "button" }, "Grab");
      grab.addEventListener("click", async () => {
        grab.disabled = true;
        result.textContent = r.source === "soulseek" ? "Asking slskd for it…" : "Sending to Deluge…";
        try {
          await api(`/releases/${r.id}/grab`, { method: "POST" });
          result.textContent = "Grabbed. Follow it under Downloads.";
          announce(`Grabbed ${r.title}`);
        } catch (err) {
          if (err instanceof SignedOut) return ctx.signedOut();
          result.textContent = err.message;
          grab.disabled = false;
        }
      });
    }
    // Never to be grabbed again; undone under Indexers, Blocklist.
    const block = r.blocklisted ? null : h("button", { type: "button", class: "quiet", "aria-label": `Blocklist ${r.title}` }, "Blocklist");
    block?.addEventListener("click", async () => {
      block.disabled = true;
      try {
        await api("/blocklist", { method: "POST", body: { release_id: r.id } });
        grab?.remove();
        block.replaceWith(blocked);
        result.textContent = "It's never grabbed now, by hand or by a sync. Take it off under Indexers, Blocklist.";
        announce(`Blocklisted ${r.title}`);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        result.textContent = err.message;
        block.disabled = false;
      }
    });
    const facts = [
      d.quality,
      d.score !== undefined ? `score ${d.score}` : null,
      r.size ? size(r.size) : null,
      r.seeders !== null ? plural(r.seeders, "seeder", "seeders") : null,
      r.published ? when(r.published) : null,
    ].filter(Boolean);
    return h(
      "li",
      { class: r.accepted ? "accepted" : null },
      h(
        "div",
        {},
        h("span", { class: "path" }, r.title),
        r.source === "soulseek" ? [" ", h("span", { class: "badge" }, "Soulseek")] : null,
        h("p", { class: "note" }, facts.join(", ") + "."),
        d.formats?.length ? h("p", { class: "note" }, `Formats: ${d.formats.map((f) => `${f.name} ${f.score > 0 ? "+" : ""}${f.score}`).join(", ")}.`) : null,
        r.accepted ? null : h("p", { class: "note" }, `Turned down: ${(d.rejections ?? d.reasons ?? []).join("; ")}. `, tryLink(r, search.profile_id)),
      ),
      h(
        "div",
        { class: "row-actions" },
        r.blocklisted ? blocked : (grab ?? h("span", { class: "badge" }, "Turned down")),
        block,
        result,
      ),
    );
  });
  return h(
    "div",
    { class: "search-results" },
    h("p", {}, `${plural(search.releases.length, "result", "results")}, ${number.format(accepted)} accepted. Searched ${when(search.searched_at)}.`),
    h("p", { class: "note" }, `${asked.join(". ")}.`),
    rows.length ? h("ul", { class: "rows" }, rows) : null,
  );
}

// ---------- Wanted movies and series ----------

// A wanted series' service: anime when wanted as anime or its library has the anime category.
const seriesService = (w, libraries) =>
  w.series_type === "anime" || libraries.find((l) => l.name === w.library)?.category === "anime" ? "anime" : "series";

export async function wantedList(ctx, kind) {
  const movies = kind === "movies";
  const [all, profiles, runs, libraries] = await Promise.all([api(`/wanted/${kind}`), api("/profiles"), api("/missing-searches"), api("/libraries")]);
  if (!ctx.current()) return [];
  const app = movies ? "Radarr" : "Sonarr";
  // What the list's parts share: the kind of title, its own app's name, profiles and noun.
  const page = {
    ctx,
    kind,
    movies,
    all,
    app,
    // Series and Anime each have their own profiles; a series wanted as anime, or going to a
    // library with the anime category, is Anime's (the server checks the choice).
    ownFor: (w) => profiles.filter((p) => p.service === (movies ? "movies" : seriesService(w, libraries))),
    noun: movies ? ["movie", "movies"] : ["series", "series"],
    filter: h("input", { id: "wanted-filter", type: "search", autocomplete: "off" }),
    list: h("ul", { class: "rows" }),
  };
  const missingOnly = h("input", { type: "checkbox", id: "wanted-missing" });
  const count = h("p", { class: "status", "aria-live": "polite" });
  const more = h("div", { class: "more" });
  const { bar } = selectMode(page);
  const draw = pagedRows(page, missingOnly, count, more);
  let typing;
  page.filter.addEventListener("input", () => {
    clearTimeout(typing);
    typing = setTimeout(draw, 250);
  });
  missingOnly.addEventListener("change", draw);
  draw();

  return [
    all.length ? group("Search all missing", searchMissing(ctx, kind, runs.find((r) => r.area === kind) ?? null)) : null,
    group(
      movies ? "Wanted movies" : "Wanted series",
      all.length
        ? h(
            "div",
            { class: "toolbar" },
            h("div", {}, h("label", { for: "wanted-filter" }, "Find a title"), page.filter),
            h("div", { class: "check" }, missingOnly, h("label", { for: "wanted-missing" }, "Only ones not in a library")),
          )
        : h("p", { class: "status" }, `Nothing wanted yet. Import ${app}'s list or add an import list in Settings.`),
      count,
      page.list,
      more,
      bar,
    ),
  ];
}

// Select mode: tick titles, then act on them all from the bar, which stays in sight.
// Sets page.selected and page.drawBar, which the rows use, and returns the bar.
function selectMode(page) {
  const { ctx, kind, all, app, noun, list, filter } = page;
  const selected = new Set();
  const bar = h("div", { class: "select-bar", role: "region", "aria-label": "Selected titles", hidden: true });
  const barSaid = h("p", { class: "form-result", role: "status" });
  const everyone = async (label, ids, call) => {
    const failed = [];
    for (const id of ids) {
      try {
        await call(id);
      } catch (err) {
        if (err instanceof SignedOut) return ctx.signedOut();
        failed.push(`${all.find((w) => w.id === id)?.title}: ${err.message}`);
      }
    }
    const done = ids.length - failed.length;
    ctx.reload(`${label} ${plural(done, noun[0], noun[1])}.${failed.length ? ` Not changed: ${failed.join("; ")}.` : ""}`);
  };
  const monitorAll = (on) =>
    everyone(on ? "Monitored" : "Stopped monitoring", [...selected], (id) =>
      api(`/wanted/${kind}/${id}/monitored`, { method: "PUT", body: { monitored: on } }),
    );
  const drawBar = () => {
    bar.hidden = selected.size === 0;
    if (!selected.size) return;
    const actions = h("div", { class: "row-actions" });
    const remove = h("button", { type: "button", class: "quiet" }, "Take off the list");
    remove.addEventListener("click", () =>
      confirmInline(actions, {
        question: `Take ${plural(selected.size, noun[0], noun[1])} off the wanted list? Nothing on disk changes; Melete stops looking for them. Importing from ${app} and import lists leave them off; adding one again by hand puts it back.`,
        confirm: `Take off ${plural(selected.size, noun[0], noun[1])}`,
        keep: "Keep them",
        run: () => everyone("Took off", [...selected], (id) => api(`/wanted/${kind}/${id}`, { method: "DELETE" })),
      }),
    );
    const clear = h("button", { type: "button", class: "quiet" }, "Clear selection");
    clear.addEventListener("click", () => {
      selected.clear();
      for (const box of list.querySelectorAll(".pick input")) box.checked = false;
      drawBar();
      filter.focus();
    });
    actions.append(
      h("button", { type: "button", class: "quiet", onclick: () => monitorAll(true) }, "Monitor"),
      h("button", { type: "button", class: "quiet", onclick: () => monitorAll(false) }, "Stop monitoring"),
      remove,
      clear,
    );
    bar.replaceChildren(h("strong", {}, `${plural(selected.size, noun[0], noun[1])} selected`), actions, barSaid);
  };
  page.selected = selected;
  page.drawBar = drawBar;
  return { bar };
}

// The titles matching the filter, 50 at a time with Show more; returns what redraws them.
function pagedRows(page, missingOnly, count, more) {
  const { all, movies, filter, list } = page;
  let shown = 0;
  let matching = all;
  const draw = () => {
    const q = filter.value.trim().toLowerCase();
    matching = all.filter((w) => (!q || w.title.toLowerCase().includes(q)) && (!missingOnly.checked || !(movies ? w.movie_id : w.series_id)));
    shown = 0;
    list.replaceChildren();
    count.textContent = q || missingOnly.checked ? `${number.format(matching.length)} of ${plural(all.length, "title", "titles")}` : plural(all.length, "title", "titles");
    showMore();
  };
  const showMore = () => {
    const next = matching.slice(shown, shown + PAGE).map((w) => row(w, page));
    list.append(...next);
    shown += next.length;
    const left = matching.length - shown;
    more.replaceChildren(
      left > 0
        ? h("button", { type: "button", class: "quiet", onclick: () => {
            showMore();
            next.at(-1)?.nextElementSibling?.querySelector("select, input, button")?.focus();
          } }, `Show more (${number.format(left)} left)`)
        : "",
    );
  };
  return draw;
}

// One wanted title: a box to select it, its facts, its quality profile, a search for it, and
// Monitor and Take off the list.
function row(w, page) {
  const { kind, movies, app, selected, drawBar } = page;
  const onDisk = movies ? w.movie_id : w.series_id;
  const pick = h("input", { type: "checkbox", id: `pick-${w.id}`, checked: selected.has(w.id) });
  pick.addEventListener("change", () => {
    if (pick.checked) selected.add(w.id);
    else selected.delete(w.id);
    drawBar();
  });
  const { profile, profileNote } = profilePicker(w, page);
  const results = h("div");
  return h(
    "li",
    {},
    h("div", { class: "pick" }, pick, h("label", { for: `pick-${w.id}`, class: "visually-hidden" }, `Select ${w.title}`)),
    h(
      "div",
      {},
      h("strong", {}, w.year ? `${w.title} (${w.year})` : w.title),
      " ",
      w.monitored ? h("span", { class: "badge" }, "Monitored") : h("span", { class: "badge" }, "Not monitored"),
      onDisk ? h("a", { class: "badge", href: `#/${kind}/${onDisk}` }, "In the library") : null,
      h("p", { class: "note" }, `${w.library ?? "No library matched"}. From ${w.source === "manual" ? "a list or by hand" : app}.`),
      h("div", { class: "search-row" }, h("div", {}, h("label", { for: `profile-${w.id}` }, "Quality profile"), profile), profileNote),
      searchForm(w, page, results),
      results,
    ),
    rowActions(w, page, profileNote),
  );
}

// Monitor (or Stop monitoring) and Take off the list, for one title.
function rowActions(w, page, profileNote) {
  const { ctx, kind, app } = page;
  const actions = h("div", { class: "row-actions" });
  const toggle = h("button", { type: "button", class: "quiet", "aria-label": `${w.monitored ? "Stop monitoring" : "Monitor"} ${w.title}` }, w.monitored ? "Stop monitoring" : "Monitor");
  toggle.addEventListener("click", async () => {
    toggle.disabled = true;
    try {
      await api(`/wanted/${kind}/${w.id}/monitored`, { method: "PUT", body: { monitored: !w.monitored } });
      ctx.reload(w.monitored ? `Stopped monitoring ${w.title}.` : `Monitoring ${w.title}.`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      profileNote.textContent = err.message;
      toggle.disabled = false;
    }
  });
  const remove = h("button", { type: "button", class: "quiet", "aria-label": `Take ${w.title} off the list` }, "Take off the list");
  remove.addEventListener("click", () =>
    confirmInline(actions, {
      question: `Take “${w.title}” off the wanted list? Nothing on disk changes; Melete stops looking for it. Importing from ${app} and import lists leave it off; adding it again by hand puts it back.`,
      confirm: "Take it off",
      keep: "Keep it",
      run: async () => {
        await api(`/wanted/${kind}/${w.id}`, { method: "DELETE" });
        ctx.reload(`Took “${w.title}” off the wanted list.`);
      },
    }),
  );
  actions.append(toggle, remove);
  return actions;
}

// The title's quality profile: its app's own, or one of Melete's, saved as soon as it's picked.
function profilePicker(w, page) {
  const { ctx, kind, app, ownFor } = page;
  const profile = h(
    "select",
    { id: `profile-${w.id}`, class: "compact" },
    h("option", { value: "" }, w.quality_profile ? `${app}'s: ${w.quality_profile}` : `${app}'s (none)`),
    ownFor(w).map((p) => h("option", { value: p.id, selected: p.id === w.profile_id }, p.name)),
  );
  const profileNote = h("span", { class: "note", role: "status" });
  profile.addEventListener("change", async () => {
    try {
      await api(`/wanted/${kind}/${w.id}/profile`, { method: "PUT", body: { profile_id: profile.value ? Number(profile.value) : null } });
      w.profile_id = profile.value ? Number(profile.value) : null;
      profileNote.textContent = "Saved.";
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      profileNote.textContent = err.message;
    }
  });
  return { profile, profileNote };
}

// Searching every indexer for the title (a series' season, or one episode of it), with the
// results shown in `results`.
function searchForm(w, page, results) {
  const { ctx, kind, movies } = page;
  const search = h("form", { class: "search-row", novalidate: true });
  let season = null;
  let episode = null;
  if (!movies) {
    // The latest monitored season is the likeliest to be missing something.
    const latest = w.seasons.filter((s) => s.monitored && s.season > 0).at(-1);
    season = h(
      "select",
      { id: `season-${w.id}`, class: "compact" },
      w.seasons.map((s) => h("option", { value: s.season, selected: s === latest }, `${seasonName(s.season)}${s.monitored ? "" : " (not monitored)"}`)),
    );
    episode = h("input", { id: `episode-${w.id}`, inputmode: "numeric", class: "year", autocomplete: "off" });
    search.append(
      h("div", {}, h("label", { for: `season-${w.id}` }, "Season"), season),
      h("div", {}, h("label", { for: `episode-${w.id}` }, "Episode"), episode),
    );
  }
  const button = h("button", { type: "submit", class: "quiet" }, movies ? "Search indexers" : "Search");
  search.append(button);
  search.addEventListener("submit", async (event) => {
    event.preventDefault();
    let body;
    if (!movies) {
      if (!w.seasons.length) {
        results.replaceChildren(h("p", { class: "error" }, "Sonarr listed no seasons for it."));
        return;
      }
      const ep = episode.value.trim();
      if (ep && !/^\d+$/.test(ep)) {
        results.replaceChildren(h("p", { class: "error" }, "The episode is a number; leave it empty to search for the whole season."));
        episode.focus();
        return;
      }
      body = { season: Number(season.value), ...(ep ? { episode: Number(ep) } : {}) };
    }
    button.disabled = true;
    results.replaceChildren(h("p", { class: "status", role: "status" }, "Asking every indexer; this can take up to a minute…"));
    try {
      const found = await api(`/wanted/${kind}/${w.id}/search`, { method: "POST", body });
      results.replaceChildren(searchResults(found, ctx));
      announce(`${plural(found.releases.length, "result", "results")} for ${w.title}`);
    } catch (err) {
      if (err instanceof SignedOut) return ctx.signedOut();
      results.replaceChildren(h("p", { class: "error" }, err.message));
    } finally {
      button.disabled = false;
    }
  });
  return search;
}
