// Watch's rows and tiles: a row of titles scrolled sideways, its tiles reached by Tab once and
// moved along with the arrow keys; a tile lifts a little on hover or focus (never growing in
// place) and, after a moment, shows its title in the hero above.
//
// - row(key, heading, items, { wide, more, spotlight, watching }): a <section> with its
//   heading, a link to the whole list when `more` is { href, label }, and a tile per item;
//   null for none.
// - posterTile(item, href, badge) and wideTile(item, href, badge): one tile each; in a row, a
//   wide tile that plays has a "Go to series" (or movie) link under it.
// - badgeOf(item, watching): the one badge a tile wears, or null; badgeChip(badge) draws it.

import { h } from "/web/lib.js";
import { clip } from "/web/page-parts.js";
import { episodeCode, playsAt, timeLeft, titleHref } from "/web/watch-play.js";

// How long "New" lasts, as Mneme has it.
const NEW_FOR_SECONDS = 14 * 24 * 3600;

// One badge per tile, the first that applies: episodes waiting (for a series being watched),
// then new.
export function badgeOf(item, watching = false) {
  if (item.kind === "series" && watching && item.unwatched_episodes > 0) {
    return `${item.unwatched_episodes} unwatched`;
  }
  const fresh = item.added_at && Date.now() / 1000 - item.added_at < NEW_FOR_SECONDS;
  return fresh && !item.progress?.watched ? "New" : null;
}

// The series someone's watching among `items` (Continue watching and Next up), each with the
// entry naming its episode, by series id.
export function watchingOf(items) {
  const watching = new Map();
  for (const item of items) if (item.kind === "series" && !watching.has(item.id)) watching.set(item.id, item);
  return watching;
}

// A picture, or the title in type on a dark ground when there's none.
function picture(src, title, wide) {
  return h(
    "div",
    { class: `tile-art${wide ? " wide" : ""}` },
    src ? h("img", { src, alt: "", loading: "lazy" }) : h("span", { class: "tile-fallback", "aria-hidden": "true" }, title),
  );
}

// A movie's or series' poster tile, linking to `href`, with why it was picked under the title
// when the item says (an AI pick's `reason`).
export function posterTile(item, href, badge = badgeOf(item)) {
  return h(
    "a",
    { class: "tile", href, "data-title": `${item.kind}:${item.id}` },
    picture(item.poster, item.title, false),
    clip(item.title, "span"),
    item.reason ? h("span", { class: "tile-reason", title: item.reason }, h("span", { class: "visually-hidden" }, ": "), item.reason) : null,
    badgeChip(badge),
  );
}

// A tile's badge, after its title so the link is read "Ninefold, New", drawn over the
// picture's corner; null for none.
export function badgeChip(badge) {
  return badge ? h("span", { class: "tile-badge" }, h("span", { class: "visually-hidden" }, ", "), badge) : null;
}

// A landscape tile for an episode or a title partway through: its still (else backdrop or
// poster), a progress bar with its track, a caption and its badge.
export function wideTile(item, href, badge = badgeOf(item)) {
  const e = item.episode;
  const still = e?.still ?? item.backdrop ?? item.poster;
  const code = e ? episodeCode(e.season, e.episode) : null;
  const progress = item.progress && !item.progress.watched && item.progress.duration_seconds ? item.progress : null;
  const left = progress ? timeLeft(progress) : null;
  const share = progress ? Math.min(100, (100 * progress.position_seconds) / progress.duration_seconds) : 0;
  const bar = progress ? h("span", { class: "tile-progress", "aria-hidden": "true" }, h("span", {})) : null;
  bar?.firstChild.style.setProperty("width", `${share}%`);
  return h(
    "a",
    { class: "tile wide", href, "data-title": `${item.kind}:${item.id}` },
    picture(still, item.title, true),
    bar,
    h(
      "span",
      { class: "tile-caption" },
      clip(code ? `${code} · ${item.title}` : item.title, "span"),
      left ? h("span", { class: "tile-note" }, left) : e?.name ? clip(e.name, "span") : null,
    ),
    badgeChip(badge),
  );
}

// A row of `items` under its heading, landscape tiles that play (`wide`) or posters that open
// the title; null when there are none. The spotlight, if any, learns the items and follows
// the row's tiles; `watching` (series ids being watched) picks the badges.
export function row(key, heading, items, { wide = false, more = null, spotlight = null, watching = new Map() } = {}) {
  if (!items.length) return null;
  spotlight?.add(items);
  const cells = items.map((item) => {
    const badge = badgeOf(item, watching.has(item.id));
    return wide ? [wideTile(item, playsAt(item), badge), openLink(item)] : [posterTile(item, titleHref(item), badge)];
  });
  const list = h(
    "ul",
    { class: `watch-shelf${wide ? " wide" : ""}`, "aria-labelledby": `row-${key}` },
    cells.map((cell) => h("li", {}, cell)),
  );
  rove(list);
  if (spotlight) spotlight.watch(list);
  return h(
    "section",
    { class: "watch-row", "aria-labelledby": `row-${key}` },
    h(
      "div",
      { class: "section-head" },
      h("h2", { id: `row-${key}` }, heading),
      more ? h("a", { href: more.href }, more.label) : null,
    ),
    list,
  );
}

// Under a tile that plays, a link to its series' or movie's page, for another episode or the
// rest of it; null when the tile opens the page itself.
function openLink(item) {
  const href = titleHref(item);
  if (playsAt(item) === href) return null;
  const what = item.kind === "movie" ? "movie" : "series";
  return h("a", { class: "tile-open", href, "aria-label": `Go to ${what}, ${item.title}` }, `Go to ${what}`);
}

// One Tab stop per row: the arrow keys move along it, Home and End to its ends, and Tab comes
// back to the tile last on; the next Tab reaches that tile's own "Go to" link, if it has one.
function rove(list) {
  const links = () => [...list.querySelectorAll("a.tile")];
  const reach = (tile, on) => {
    tile.setAttribute("tabindex", on ? "0" : "-1");
    tile.parentElement?.querySelector("a.tile-open")?.setAttribute("tabindex", on ? "0" : "-1");
  };
  links().forEach((a, i) => reach(a, !i));
  list.addEventListener("keydown", (event) => {
    const all = links();
    // From a tile or its "Go to" link alike.
    const at = all.indexOf(document.activeElement?.closest("li")?.querySelector("a.tile"));
    if (at < 0) return;
    const to = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: all.length - 1 }[event.key];
    if (to === undefined || !all[to]) return;
    event.preventDefault();
    reach(all[at], false);
    reach(all[to], true);
    all[to].focus();
  });
  list.addEventListener("focusin", (event) => {
    const tile = event.target.closest("li")?.querySelector("a.tile");
    if (!tile) return;
    for (const a of links()) reach(a, a === tile);
  });
}
