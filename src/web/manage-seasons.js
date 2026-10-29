// A series' seasons on its Manage page, each with its own monitoring, how much of it is here,
// Search and Search and choose; opened, its episodes, each with its monitoring, air date,
// state, size, and its own Search and Search and choose. Episodes are TMDB's list joined with
// the files on disk, so they're listed once something of the series is in the library.

import { SignedOut, airDate, api, append, h, plural, seasonName, size } from "/web/lib.js";
import { emptyState, failed, placeholders, stateLabel } from "/web/page-parts.js";
import { fileRows, monitorSwitch } from "/web/manage-parts.js";
import { searchAndChoose, searchAndGrab } from "/web/manage-search.js";

export function seasonsPart(v) {
  const wanted = new Map((v.wanted?.seasons ?? []).map((s) => [s.season, s.monitored]));
  const here = new Map((v.folder?.seasons ?? []).map((s) => [s.season, s]));
  const numbers = [...new Set([...wanted.keys(), ...here.keys()])].sort((a, b) => (a === 0) - (b === 0) || a - b);
  const sections = numbers.map((n) => season(v, n, wanted.get(n) ?? false, here.get(n)));
  return [
    h("h2", { class: "part" }, "Seasons"),
    sections.length
      ? sections
      : emptyState(v.t.wanted ? "No seasons are known for it yet. They're listed once TMDB's list or a file of it is here." : "No seasons on disk.", null),
    v.folder ? null : h("p", { class: "note" }, "Episodes are listed once something of it is in the library."),
  ];
}

const now = () => new Date().toISOString().slice(0, 10);

function season(v, n, monitored, here) {
  const { t, ctx } = v;
  const name = seasonName(n);
  const said = h("p", { class: "form-result", role: "status" });
  const results = h("div", {});
  const episodes = h("div", { class: "episodes-box", hidden: true });
  const switchOn = t.wanted
    ? monitorSwitch({
        ctx,
        label: `Monitor ${name}`,
        on: monitored,
        said,
        change: (on) => api(`/wanted/series/${t.wanted_id}/seasons/${n}`, { method: "PUT", body: { monitored: on } }),
      })
    : null;
  const count = here ? `${here.available} of ${plural(here.episodes || here.available, "episode", "episodes")}` : "Nothing on disk";
  const mark = !monitored && t.wanted ? "off" : here && here.available >= here.episodes ? "have" : "missing";
  const ask = { path: `/wanted/series/${t.wanted_id}/search`, body: { season: n }, label: `${t.title}, ${name}` };
  const buttons = [];
  if (t.wanted) {
    const search = h("button", { type: "button", class: "quiet" }, "Search");
    search.addEventListener("click", async () => {
      search.disabled = true;
      await searchAndGrab(ctx, ask, said);
      search.disabled = false;
    });
    buttons.push(search, h("button", { type: "button", class: "quiet", onclick: () => searchAndChoose(ctx, ask, results) }, "Search and choose"));
  }
  if (v.folder && here) {
    const open = h("button", { type: "button", class: "quiet", "aria-expanded": "false" }, "Episodes");
    open.addEventListener("click", () => {
      const opening = episodes.hidden;
      episodes.hidden = !opening;
      open.setAttribute("aria-expanded", String(opening));
      if (opening && !episodes.dataset.loaded) loadEpisodes(v, n, episodes);
    });
    buttons.push(open);
  }
  return h(
    "section",
    { class: "season", "aria-label": name },
    h(
      "header",
      {},
      switchOn,
      h("h3", {}, name),
      stateLabel(mark, !monitored && t.wanted ? `Not monitored · ${count}` : count),
      h("div", { class: "row-actions" }, buttons),
    ),
    said,
    results,
    episodes,
  );
}

async function loadEpisodes(v, n, box) {
  box.dataset.loaded = "1";
  box.replaceChildren(placeholders("episodes", { kind: "rows", count: 4 }));
  try {
    const detail = await api(`/series/${v.folder.id}/seasons/${n}`);
    if (!v.ctx.current()) return;
    box.replaceChildren();
    append(box, episodeTable(v, n, detail.episodes));
  } catch (err) {
    if (err instanceof SignedOut) return v.ctx.signedOut();
    delete box.dataset.loaded;
    box.replaceChildren(failed("the episodes", err, () => loadEpisodes(v, n, box)));
  }
}

// An episode's state: on disk, on its way, not out yet, not monitored, or missing.
function episodeState(v, n, e, monitored) {
  const size_ = e.files.reduce((sum, f) => sum + f.size, 0);
  if (e.files.length) return ["have", "In the library", size_];
  const coming = v.downloads.some((d) => d.state !== "failed" && (d.season === null || (d.season === n && (d.episode === null || d.episode === e.episode))));
  if (coming) return ["coming", "On its way", 0];
  if (!e.air_date || e.air_date > now()) return ["notyet", e.air_date ? `Airs ${airDate(e.air_date)}` : "Not aired", 0];
  if (!monitored) return ["off", "Not monitored", 0];
  return ["missing", "Missing", 0];
}

function episodeTable(v, n, list) {
  const { t, ctx } = v;
  if (!list.length) return h("p", { class: "status" }, "TMDB lists no episodes for it, and none are on disk.");
  const seasonOn = (v.wanted?.seasons ?? []).find((s) => s.season === n)?.monitored ?? false;
  const left = new Set((v.wanted?.unmonitored_episodes ?? []).filter((x) => x.season === n).map((x) => x.episode));
  const results = h("div", {});
  const cell = (label, content, cls) => h("td", { "data-label": label, class: cls }, content);
  const rows = list.map((e) => {
    const monitored = seasonOn && !left.has(e.episode);
    const [mark, words, bytes] = episodeState(v, n, e, monitored);
    const label = `Episode ${e.episode}${e.name ? `: ${e.name}` : ""}`;
    const ask = { path: `/wanted/series/${t.wanted_id}/search`, body: { season: n, episode: e.episode }, label: `${t.title}, ${seasonName(n)}, episode ${e.episode}` };
    const said = h("span", { class: "note", role: "status" });
    const actions = t.wanted
      ? [
          h("button", { type: "button", class: "quiet", "aria-label": `Search for episode ${e.episode}`, onclick: () => searchAndGrab(ctx, ask, said) }, "Search"),
          h("button", { type: "button", class: "quiet", "aria-label": `Search and choose for episode ${e.episode}`, onclick: () => searchAndChoose(ctx, ask, results) }, "Choose"),
          said,
        ]
      : null;
    return h(
      "tr",
      {},
      t.wanted
        ? h("td", { class: "tick" }, monitorSwitch({ ctx, label: `Monitor ${label}`, on: monitored, said, change: (on) => api(`/wanted/series/${t.wanted_id}/seasons/${n}/episodes/${e.episode}`, { method: "PUT", body: { monitored: on } }) }))
        : null,
      h("th", { scope: "row" }, `${e.episode}. ${e.name ?? "Untitled"}`),
      cell("Aired", e.air_date ? airDate(e.air_date) : "—"),
      cell("State", stateLabel(mark, words)),
      cell("Size", bytes ? size(bytes) : "—", "r"),
      cell("Search", actions ? h("div", { class: "row-actions" }, actions) : "—"),
    );
  });
  const files = list.flatMap((e) => e.files);
  const filesBox = h("div", {});
  const showFiles = files.length ? h("button", { type: "button", class: "quiet" }, `Files (${files.length})`) : null;
  showFiles?.addEventListener("click", async () => {
    showFiles.disabled = true;
    filesBox.replaceChildren(await fileRows(files));
    showFiles.hidden = true;
  });
  return [
    h(
      "div",
      { class: "table-wrap stacks" },
      h(
        "table",
        { class: "data episodes-table" },
        h("caption", { class: "visually-hidden" }, `${seasonName(n)}'s episodes`),
        h("thead", {}, h("tr", {}, [t.wanted ? "Monitored" : null, "Episode", "Aired", "State", "Size", "Search"].filter(Boolean).map((c) => h("th", { scope: "col", class: c === "Size" ? "r" : null }, c)))),
        h("tbody", {}, rows),
      ),
    ),
    results,
    showFiles,
    filesBox,
  ];
}
