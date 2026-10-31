// A series' seasons and episodes, as two panes: seasons down the left (Specials last), the
// chosen season's episodes on the right, each row one link that plays it (from where it was
// left), with Mark watched beside it; the season can be marked watched as a whole. For an
// admin, each episode's file is folded under it, with "Wrong file?" for one Melete brought in.
// Below 800 px the panes stack, the seasons a row of their own.
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

// Numbered seasons first, Specials last.
export function seasonOrder(seasons) {
  return [...seasons].sort((a, b) => (a.season === 0) - (b.season === 0) || a.season - b.season);
}

export function episodesPicker(series, progress, { opening, gen, admin = false }) {
  const seasons = seasonOrder(series.seasons);
  const pane = h("div", { class: "episode-pane", id: "episode-pane" });
  const buttons = seasons.map((x) =>
    h(
      "button",
      { type: "button", class: "season", "aria-pressed": "false", "aria-controls": "episode-pane" },
      seasonName(x.season),
      h("small", {}, x.episodes > x.available ? `${x.available} of ${plural(x.episodes, "episode", "episodes")}` : plural(x.available, "episode", "episodes")),
    ),
  );
  // Focus stays on the season pressed; the season's name and count are announced.
  const choose = (i, said = false) => {
    buttons.forEach((b, j) => b.setAttribute("aria-pressed", String(i === j)));
    const params = new URLSearchParams(location.hash.split("?")[1] ?? "");
    params.set("season", seasons[i].season);
    history.replaceState(history.state, "", `#/series/${series.id}?${params}`);
    showSeason(series, seasons[i], progress, pane, gen, admin).then((count) => {
      if (said && count !== undefined) announce(`${seasonName(seasons[i].season)}, ${plural(count, "episode", "episodes")}`);
    });
  };
  buttons.forEach((b, i) => b.addEventListener("click", () => choose(i, true)));
  const first = Math.max(0, seasons.findIndex((x) => x.season === opening));
  choose(first);
  return h(
    "section",
    { class: "episodes-picker", "aria-labelledby": "seasons-head" },
    h("h2", { id: "seasons-head", class: "visually-hidden" }, "Seasons and episodes"),
    h("div", { class: "season-list", role: "group", "aria-label": "Seasons" }, buttons),
    pane,
  );
}

// Loads a season into the pane: its heading, Mark season watched, and its episodes. Returns
// how many episodes it has, or nothing when another season was chosen meanwhile.
async function showSeason(series, chosen, progress, pane, gen, admin) {
  const mine = String(chosen.season);
  pane.dataset.season = mine;
  pane.replaceChildren(placeholders(seasonName(chosen.season).toLowerCase(), { kind: "rows", count: 4 }));
  let season;
  try {
    season = await api(`/series/${series.id}/seasons/${chosen.season}`);
  } catch (err) {
    if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
    if (pane.dataset.season !== mine) return;
    pane.replaceChildren(failed(seasonName(chosen.season).toLowerCase(), err, () => showSeason(series, chosen, progress, pane, gen, admin)));
    return;
  }
  if (gen !== generation || pane.dataset.season !== mine) return;
  const list = h("ol", { class: "episode-list" });
  const markAll = h("button", { type: "button", class: "quiet" });
  // Redrawn after marking; focus goes back to the button that was pressed, in its new row.
  const draw = (focusOn = null) => {
    list.replaceChildren(...season.episodes.map((e) => episodeRow(series, chosen.season, e, progress, () => draw(e), admin)));
    if (focusOn) list.children[season.episodes.indexOf(focusOn)]?.querySelector(".episode-mark")?.focus();
    const files = season.episodes.flatMap((e) => e.files);
    const all = files.length > 0 && season.episodes.every((e) => !e.files.length || progress.get(e.files[0].id)?.watched);
    markAll.textContent = all ? "Mark season unwatched" : "Mark season watched";
    markAll.hidden = !files.length;
    markAll.onclick = () => markSeries(series.id, chosen.season, !all, progress, markAll).then(() => draw());
  };
  draw();
  pane.replaceChildren(
    h("div", { class: "episode-pane-head" }, h("h2", {}, seasonName(chosen.season)), markAll),
    season.episodes.length ? list : h("p", { class: "status" }, "No episodes in this season yet."),
  );
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

// One episode: a link that plays it (its still with "S2:E4" and how far through, its name,
// air date, length and overview), and Mark watched beside it, and for an admin its file under
// them; without a file, a plain row.
function episodeRow(series, seasonNumber, e, progress, redraw, admin) {
  const file = e.files[0];
  const code = episodeCode(seasonNumber, e.episode);
  const p = file ? progress.get(file.id) : null;
  const chip = p?.watched ? "Watched" : partway(p) ? timeLeft(p) : null;
  const still = h(
    "span",
    { class: "episode-still" },
    e.still ? h("img", { src: e.still, alt: "", loading: "lazy" }) : null,
    h("span", { class: "episode-code" }, code),
    chip ? h("span", { class: `episode-chip${p?.watched ? "" : " on"}` }, chip) : null,
  );
  const words = h(
    "span",
    { class: "episode-words" },
    h("span", { class: "episode-name" }, e.name ?? `Episode ${e.episode}`),
    h("span", { class: "episode-facts" }, [e.air_date ? `Aired ${airDate(e.air_date)}` : null, runtime(e.runtime_minutes)].filter(Boolean).join(" · ")),
    e.overview ? h("span", { class: "episode-overview" }, e.overview) : null,
  );
  if (!file) {
    return h("li", { class: "episode-row missing" }, h("div", { class: "episode-play" }, still, words, h("span", { class: "note" }, "Not in the library")));
  }
  const href = playHref(file.id, { series: series.id, season: seasonNumber, episode: e.episode });
  const label = `${partway(p) ? "Resume" : "Play"} ${code}: ${e.name ?? `Episode ${e.episode}`}`;
  const mark = h("button", { type: "button", class: "quiet episode-mark", "aria-label": `${p?.watched ? "Mark unwatched" : "Mark watched"}: ${code}` }, p?.watched ? "Mark unwatched" : "Mark watched");
  mark.addEventListener("click", () => markFiles(e.files, !p?.watched, progress, mark).then(redraw));
  return h("li", { class: "episode-row" }, h("a", { class: "episode-play", href, "aria-label": label }, still, words), mark, admin ? episodeFiles(e.files, code) : null);
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
