// A series' seasons and episodes: a select of seasons (Specials last) with how many episodes
// the chosen one has and how many are watched, then its episodes, each row one link that plays
// it (from where it was left) with Mark watched as a toggle at its end; the season can be marked
// watched as a whole. For an admin, each episode's file is folded under it, with "Wrong file?"
// for one Melete brought in.
//
// episodesPicker(series, progress, { opening, gen, admin }) → the section; `progress` maps a
// file id to the user's progress entry and is kept up to date as things are marked; `opening`
// is the season shown first (the one Play would start), `gen` the navigation it belongs to.

import { SignedOut, airDate, announce, api, h, plural, runtime, seasonName } from "/web/lib.js";
import { generation, signIn } from "/web/page-state.js";
import { playHref } from "/web/cards.js";
import { failed, placeholders } from "/web/page-parts.js";
import { episodeCode, partway, timeLeft } from "/web/watch-play.js";
import { fileItem } from "/web/title-more.js";
import { icon } from "/web/music-controls.js";

// Numbered seasons first, Specials last.
export function seasonOrder(seasons) {
  return [...seasons].sort((a, b) => (a.season === 0) - (b.season === 0) || a.season - b.season);
}

export function episodesPicker(series, progress, { opening, gen, admin = false }) {
  const seasons = seasonOrder(series.seasons);
  const pane = h("div", { class: "episode-pane", id: "episode-pane" });
  const select = h(
    "select",
    { id: "season-choice", class: "season-choice", "aria-controls": "episode-pane" },
    seasons.map((x, i) => h("option", { value: String(i) }, seasonName(x.season))),
  );
  const count = h("span", { class: "season-count" });
  const tools = h("span", { class: "season-tools" });
  // Focus stays on the select; the season's name and count are announced.
  const choose = (i, said = false) => {
    select.value = String(i);
    const params = new URLSearchParams(location.hash.split("?")[1] ?? "");
    params.set("season", seasons[i].season);
    history.replaceState(history.state, "", `#/series/${series.id}?${params}`);
    showSeason(series, seasons[i], progress, { pane, count, tools }, gen, admin).then((n) => {
      if (said && n !== undefined) announce(`${seasonName(seasons[i].season)}, ${plural(n, "episode", "episodes")}`);
    });
  };
  select.addEventListener("change", () => choose(Number(select.value), true));
  const first = Math.max(0, seasons.findIndex((x) => x.season === opening));
  choose(first);
  return h(
    "section",
    { class: "episodes-picker", "aria-labelledby": "seasons-head" },
    h("h2", { id: "seasons-head", class: "visually-hidden" }, "Seasons and episodes"),
    h(
      "div",
      { class: "episode-pane-head" },
      seasons.length > 1 ? h("label", { for: "season-choice", class: "visually-hidden" }, "Season") : null,
      seasons.length > 1 ? h("span", { class: "season-pick" }, select) : h("h3", { class: "season-only" }, seasonName(seasons[0].season)),
      count,
      tools,
    ),
    pane,
  );
}

// "10 episodes, 9 watched", or "8 of 10 episodes" while some aren't in the library.
function seasonCount(chosen, season, progress) {
  const have = chosen.episodes > chosen.available ? `${chosen.available} of ${plural(chosen.episodes, "episode", "episodes")}` : plural(chosen.available, "episode", "episodes");
  const watched = season ? season.episodes.filter((e) => e.files.length && progress.get(e.files[0].id)?.watched).length : 0;
  return watched ? `${have}, ${watched} watched` : have;
}

// Loads a season into the pane: Mark season watched and its episodes, and its count beside the
// select. Returns how many episodes it has, or nothing when another season was chosen meanwhile.
async function showSeason(series, chosen, progress, { pane, count, tools }, gen, admin) {
  const mine = String(chosen.season);
  pane.dataset.season = mine;
  count.textContent = seasonCount(chosen, null, progress);
  tools.replaceChildren();
  pane.replaceChildren(placeholders(seasonName(chosen.season).toLowerCase(), { kind: "rows", count: 4 }));
  let season;
  try {
    season = await api(`/series/${series.id}/seasons/${chosen.season}`);
  } catch (err) {
    if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
    if (pane.dataset.season !== mine) return;
    pane.replaceChildren(failed(seasonName(chosen.season).toLowerCase(), err, () => showSeason(series, chosen, progress, { pane, count, tools }, gen, admin)));
    return;
  }
  if (gen !== generation || pane.dataset.season !== mine) return;
  const list = h("ol", { class: "episode-list" });
  const markAll = h("button", { type: "button", class: "tertiary with-icon season-mark" });
  // Redrawn after marking; focus goes back to the button that was pressed, in its new row.
  const draw = (focusOn = null) => {
    list.replaceChildren(...season.episodes.map((e) => episodeRow(series, chosen.season, e, progress, () => draw(e), admin)));
    if (focusOn) list.children[season.episodes.indexOf(focusOn)]?.querySelector(".episode-mark")?.focus();
    const files = season.episodes.flatMap((e) => e.files);
    const all = files.length > 0 && season.episodes.every((e) => !e.files.length || progress.get(e.files[0].id)?.watched);
    markAll.replaceChildren(icon("check"), all ? "Mark season unwatched" : "Mark season watched");
    count.textContent = seasonCount(chosen, season, progress);
    markAll.hidden = !files.length;
    markAll.onclick = () => markSeries(series.id, chosen.season, !all, progress, markAll).then(() => draw());
  };
  draw();
  tools.replaceChildren(markAll);
  pane.replaceChildren(season.episodes.length ? list : h("p", { class: "status" }, "No episodes in this season yet."));
  return season.episodes.length;
}

// Marks a season (null: the whole series, specials too) watched or not in one request, then
// reads the series' progress afresh into `progress`, saying how it went. True once marked.
export async function markSeries(seriesId, season, watched, progress, button) {
  const what = season === null ? "the series" : seasonName(season);
  const how = watched ? "watched" : "unwatched";
  // A disabled button loses focus; it gets it back once it's done.
  const focused = document.activeElement === button;
  const done = (said, marked) => {
    button.disabled = false;
    if (focused) button.focus();
    announce(said);
    return marked;
  };
  button.disabled = true;
  try {
    await api(`/series/${seriesId}/watched`, { method: "PUT", body: season === null ? { watched } : { watched, season } });
  } catch (err) {
    if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
    return done(`Couldn't mark ${what} ${how}. ${err.message}`, false);
  }
  try {
    const fresh = await api(`/me/progress?series_id=${seriesId}`);
    progress.clear();
    for (const entry of fresh) progress.set(entry.file_id, entry);
  } catch (err) {
    if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
    return done(`Marked ${what} ${how}. Reload the page to see it.`, true);
  }
  return done(`Marked ${what} ${how}.`, true);
}

// Marks every file in `files` watched (or not), a few at a time, saying how it went.
export async function markFiles(files, watched, progress, button) {
  button.disabled = true;
  const queue = [...files];
  let failures = 0;
  const work = async () => {
    for (let file = queue.shift(); file; file = queue.shift()) {
      try {
        progress.set(file.id, await api(`/files/${file.id}/watched`, { method: "PUT", body: { watched } }));
      } catch (err) {
        if (err instanceof SignedOut) throw err;
        failures += 1;
      }
    }
  };
  try {
    await Promise.all([work(), work(), work(), work()]);
  } catch (err) {
    if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
  }
  button.disabled = false;
  announce(
    failures
      ? `${plural(failures, "file wasn't", "files weren't")} marked. Try again.`
      : `Marked ${plural(files.length, "file", "files")} as ${watched ? "watched" : "unwatched"}.`,
  );
}

// One episode: a link that plays it (its still with how far through drawn on it, its number and
// name, air date, length and overview), and Mark watched as a toggle at its end, and for an
// admin its file under them; without a file, a plain row. The one partway through has a fill.
function episodeRow(series, seasonNumber, e, progress, redraw, admin) {
  const file = e.files[0];
  const code = episodeCode(seasonNumber, e.episode);
  const p = file ? progress.get(file.id) : null;
  const going = partway(p);
  const done = p?.watched ? 1 : going ? p.position_seconds / p.duration_seconds : 0;
  const bar = done ? h("span", { class: `episode-progress${p?.watched ? " watched" : ""}`, "aria-hidden": "true" }, h("span", {})) : null;
  bar?.firstChild.style.setProperty("width", `${Math.min(100, Math.round(done * 100))}%`);
  const still = h("span", { class: "episode-still" }, e.still ? h("img", { src: e.still, alt: "", loading: "lazy" }) : null, bar);
  const name = e.name ?? `Episode ${e.episode}`;
  const facts = [going ? timeLeft(p) : p?.watched ? "Watched" : null, e.air_date ? `Aired ${airDate(e.air_date)}` : null, runtime(e.runtime_minutes)];
  const words = h(
    "span",
    { class: "episode-words" },
    h("span", { class: "episode-name" }, h("span", { class: "episode-number" }, seasonNumber === 0 ? code : String(e.episode)), name),
    h("span", { class: "episode-facts" }, facts.filter(Boolean).join(" · ")),
    e.overview ? h("span", { class: "episode-overview" }, e.overview) : null,
  );
  if (!file) {
    return h("li", { class: "episode-row missing" }, h("div", { class: "episode-play" }, still, words, h("span", { class: "note" }, "Not in the library")));
  }
  const href = playHref(file.id, { series: series.id, season: seasonNumber, episode: e.episode });
  const label = `${going ? "Resume" : "Play"} ${code}: ${name}`;
  const mark = h("button", { type: "button", class: "icon filled tip-end episode-mark", "aria-pressed": String(Boolean(p?.watched)), "aria-label": `Watched: ${code}` }, icon("check"));
  mark.addEventListener("click", () => markFiles(e.files, !p?.watched, progress, mark).then(redraw));
  return h("li", { class: `episode-row${going ? " going" : ""}` }, h("a", { class: "episode-play", href, "aria-label": label }, still, words), mark, admin ? episodeFiles(e.files, code) : null);
}

// For an admin: an episode's file (or files), folded, read when it's opened, as About shows a
// movie's; one Melete brought in from a download has "Wrong file?".
function episodeFiles(files, code) {
  const list = h("ul", { class: "files about-files" });
  const box = h("details", { class: "episode-file" }, h("summary", { "aria-label": `${files.length > 1 ? "Files" : "File"} of ${code}` }, files.length > 1 ? "Files" : "File"), list);
  box.addEventListener("toggle", () => {
    if (box.open && !list.childElementCount) list.replaceChildren(...files.map((f) => fileItem(f, true)));
  });
  return box;
}
