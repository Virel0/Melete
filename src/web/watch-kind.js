// Watch's Shows, Anime and Movies pages: the home's template narrowed to one kind (anime never
// among the shows), then every title of that kind in a grid with search, sort and genre.
//
// The hero and rows come from `/home`, sorted into kinds: a movie is a movie, and a series is
// anime when `/series?anime=only` lists it (wanted as anime, or in an anime library), else a
// show. The grid stands on its own, so when the home can't be had the page is the grid.

import { SignedOut, api, h, show } from "/web/lib.js";
import { generation, leaving } from "/web/page-state.js";
import { placeholders } from "/web/page-parts.js";
import { everything } from "/web/browse.js";
import { hero } from "/web/watch-hero.js";
import { row, watchingOf } from "/web/watch-tiles.js";
import { animeIds } from "/web/watch-play.js";

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
  const [home, anime, genres, libraries, ai] = await Promise.all([
    quiet(api("/home"), null),
    page === "movies" ? new Set() : quiet(animeIds(), null),
    quiet(api("/browse/genres"), []),
    quiet(api("/browse/libraries"), []),
    quiet(api("/ai"), { enabled: false }),
  ]);
  if (gen !== generation) return;
  const top = home && anime ? narrowed(page, home, anime) : null;
  if (top) leaving.push(top.stop);
  show(
    about.title,
    h("h1", { class: top ? "visually-hidden" : null }, about.title),
    top?.hero,
    top?.rows.length ? h("div", { class: "watch-rows" }, top.rows) : null,
    everything(page, params, gen, { genres, libraries, ai }),
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

// The home's hero and rows narrowed to this page's kind; null when it has no picks.
function narrowed(page, home, anime) {
  const about = PAGES[page];
  const kindOf = (item) => (item.kind === "movie" ? "movies" : anime.has(item.id) ? "anime" : "shows");
  const mine = (items) => items.filter((item) => kindOf(item) === page);
  const rowOf = (key) => mine(home.rows.find((r) => r.key === key)?.items ?? []);
  // Picks: the home's of this kind, then the newest of it, each once.
  const seen = new Set();
  const picks = [...mine(home.hero), ...rowOf(about.latest)].filter((i) => !seen.has(i.id) && seen.add(i.id)).slice(0, HERO);
  if (!picks.length) return null;
  const watching = watchingOf([...rowOf("continue_watching"), ...rowOf("next_up")]);
  const top = hero(picks, { watching });
  top.spotlight.add(picks);
  const rows = [
    ["continue_watching", "Continue watching", true],
    ["next_up", "Next up", true],
    ["favourites", "On My List", false],
    [about.latest, about.heading, false],
  ].map(([key, heading, wide]) => row(key, heading, rowOf(key), { wide, spotlight: top.spotlight, watching }));
  return { hero: top.el, rows: rows.filter(Boolean), stop: top.stop };
}
