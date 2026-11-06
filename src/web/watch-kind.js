// Watch's Shows, Anime and Movies pages: the home's template narrowed to one kind (anime never
// among the shows), then every title of that kind in a grid with search, sort and genre.
//
// The hero and rows come from the page's own home, `/home?kind=`: a movie is a movie, and a
// series is anime when `/series?anime=only` lists it (wanted as anime, or in an anime library),
// else a show; its "Because you watched" rows come from titles of the page's kind. The grid
// stands on its own, so when the home can't be had the page is the grid.

import { SignedOut, api, h, show } from "/web/lib.js";
import { generation, leaving } from "/web/page-state.js";
import { placeholders } from "/web/page-parts.js";
import { everything } from "/web/browse.js";
import { hero } from "/web/watch-hero.js";
import { watchingOf } from "/web/watch-tiles.js";
import { homeRows } from "/web/home.js";

// As many picks as the home's hero has.
const HERO = 8;
const PAGES = {
  shows: { title: "Shows", latest: "latest_shows", heading: "Latest shows" },
  anime: { title: "Anime", latest: "latest_anime", heading: "Latest anime" },
  movies: { title: "Movies", latest: "latest_movies", heading: "Latest movies" },
};

// Asked for quietly: the page shows what it can without it, though signing out still signs out.
const quiet = (promise, fallback) => promise.catch((err) => (err instanceof SignedOut ? Promise.reject(err) : fallback));

export async function kindPage(page, params, gen) {
  const about = PAGES[page];
  show(about.title, h("h1", {}, about.title), placeholders(about.title.toLowerCase(), { kind: "cards", count: 12 }));
  const [home, genres, libraries, ai] = await Promise.all([
    quiet(api(`/home?kind=${page}`), null),
    quiet(api("/browse/genres"), []),
    quiet(api("/browse/libraries"), []),
    quiet(api("/ai"), { enabled: false }),
  ]);
  if (gen !== generation) return;
  // The series being watched, which wear "N unwatched" on their tiles and in the grid.
  const watching = home ? watchingOf(home.rows.filter((r) => ["continue_watching", "next_up", "new_episodes"].includes(r.key)).flatMap((r) => r.items)) : new Map();
  const top = home ? narrowed(page, home, watching) : null;
  if (top) leaving.push(top.stop);
  show(
    about.title,
    h("h1", { class: top ? "visually-hidden" : null }, about.title),
    top?.hero,
    top?.rows.length ? h("div", { class: "watch-rows" }, top.rows) : null,
    everything(page, params, gen, { genres, libraries, ai, watching }),
  );
}

// The old series list's address (`#/series`, links and bookmarks): anime has its own page now,
// and the rest are the shows. Replaced, so Back doesn't come here again.
export function oldSeriesList(params) {
  const next = new URLSearchParams(params);
  const to = next.get("category") === "anime" ? "anime" : "shows";
  if (to === "anime") next.delete("category");
  location.replace(`#/${to}${next.toString() ? `?${next}` : ""}`);
}

// This page's home (`/home?kind=`): its hero and rows, its "Because you watched" ones made from
// titles of its kind alone; null when it has no picks.
function narrowed(page, home, watching) {
  const about = PAGES[page];
  const latest = home.rows.find((r) => r.key === about.latest)?.items ?? [];
  // Picks: the home's, then the newest of this kind, each once.
  const seen = new Set();
  const picks = [...home.hero, ...latest].filter((i) => !seen.has(i.id) && seen.add(i.id)).slice(0, HERO);
  if (!picks.length) return null;
  const top = hero(picks, { watching });
  top.spotlight.add(picks);
  const rows = homeRows(home.rows, top, watching, about.heading);
  return { hero: top.el, rows: rows.filter(Boolean), stop: top.stop };
}
