// Watch's Shows, Anime and Movies pages: the home's template narrowed to one kind (anime never
// among the shows), then every title of that kind in a grid with search, sort and genre.
//
// The hero and rows come from the page's own home, `/home?kind=`: a movie is a movie, and a
// series is anime when `/series?anime=only` lists it (wanted as anime, or in an anime library),
// else a show; its "Because you watched" rows come from titles of the page's kind. The grid
// stands on its own, so when the home can't be had the page is the grid.
//
// When a title comes or goes, or My List changes (watch-live.js), the rows and the grid are
// drawn again in place; the hero stays as it is.

import { SignedOut, api, h, show } from "/web/lib.js";
import { generation, leaving, signIn } from "/web/page-state.js";
import { placeholders } from "/web/page-parts.js";
import { everything } from "/web/browse.js";
import { hero } from "/web/watch-hero.js";
import { followWatch, keepPlace } from "/web/watch-live.js";
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
  const top = home ? narrowed(page, home) : null;
  if (top) leaving.push(top.stop);
  const rows = top ? h("div", { class: "watch-rows", hidden: !top.rows.length }, top.rows) : null;
  let refreshGrid = null;
  show(
    about.title,
    h("h1", { class: top ? "visually-hidden" : null }, about.title),
    top?.hero,
    rows,
    everything(page, params, gen, { genres, libraries, ai, follow: (refresh) => (refreshGrid = refresh) }),
  );
  const again = async () => {
    refreshGrid?.();
    if (!top) return;
    let fresh;
    try {
      fresh = await api(`/home?kind=${page}`);
    } catch (err) {
      if (err instanceof SignedOut) signIn("Your session ended. Sign in again.");
      return;
    }
    if (gen !== generation) return;
    const watching = watchingOf(fresh.rows.filter((r) => ["continue_watching", "next_up", "new_episodes"].includes(r.key)).flatMap((r) => r.items));
    const drawn = homeRows(fresh.rows, top.top, watching, about.heading).filter(Boolean);
    keepPlace(rows, drawn);
    rows.hidden = !drawn.length;
  };
  followWatch(gen, { library: again, favourites: again });
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
function narrowed(page, home) {
  const about = PAGES[page];
  const latest = home.rows.find((r) => r.key === about.latest)?.items ?? [];
  // Picks: the home's, then the newest of this kind, each once.
  const seen = new Set();
  const picks = [...home.hero, ...latest].filter((i) => !seen.has(i.id) && seen.add(i.id)).slice(0, HERO);
  if (!picks.length) return null;
  const watching = watchingOf(home.rows.filter((r) => ["continue_watching", "next_up", "new_episodes"].includes(r.key)).flatMap((r) => r.items));
  const top = hero(picks, { watching });
  top.spotlight.add(picks);
  const rows = homeRows(home.rows, top, watching, about.heading);
  return { top, hero: top.el, rows: rows.filter(Boolean), stop: top.stop };
}
