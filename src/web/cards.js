// The cards titles are shown on in grids and shelves, and the player's address for a file.

import { episodeLabel, h } from "/web/lib.js";
import { drawnArt } from "/web/drawn-art.js";

// The title is under the picture, so the picture says nothing to screen readers. Without a
// picture, art is drawn from `name` when one is given.
export function art(src, name = null) {
  return h("div", { class: "art" }, src ? h("img", { src, alt: "", loading: "lazy" }) : name ? drawnArt(name, { named: true }) : null);
}

// A title's card in a grid: its poster, then one line with its title and, after it, `badge`
// ("New", "3 unwatched", from watch-tiles.js badgeOf) when one is given, else its year.
export function titleCard(kind, item, badge = null) {
  return h(
    "li",
    {},
    h(
      "a",
      { class: "card", href: `#/${kind}/${item.id}` },
      art(item.poster, item.title),
      h(
        "span",
        { class: "card-line" },
        h("span", { class: "title clip", title: item.title }, item.title),
        badge
          ? h("span", { class: "tile-badge" }, h("span", { class: "visually-hidden" }, ", "), badge)
          : item.year
            ? h("span", { class: "meta" }, h("span", { class: "visually-hidden" }, ", "), String(item.year))
            : null,
      ),
    ),
  );
}

// The player's address for a file, with what it belongs to.
export function playHref(fileId, context, fromStart = false) {
  const params = new URLSearchParams(context);
  if (fromStart) params.set("start", "0");
  return `#/play/${fileId}?${params}`;
}

export function continueCard(item) {
  const href =
    item.movie_id || item.season === null
      ? playHref(item.file_id, item.movie_id ? { movie: item.movie_id } : {})
      : playHref(item.file_id, { series: item.series_id, season: item.season, episode: item.episode });
  const episode =
    item.season !== null && item.episode !== null ? episodeLabel(item.season, item.episode, item.episode_name) : null;
  const left = item.duration_seconds
    ? Math.max(1, Math.round((item.duration_seconds - item.position_seconds) / 60))
    : null;
  return h(
    "li",
    {},
    h(
      "a",
      { class: "card", href },
      art(item.still ?? item.poster),
      h("div", { class: "title" }, item.title),
      episode ? h("div", { class: "meta" }, episode) : null,
      left ? h("div", { class: "meta" }, `${left} min left`) : null,
    ),
  );
}
