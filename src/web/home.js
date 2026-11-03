// Watch's home, from `/home` in one request: the hero stepping between picks (what you're
// partway through, then the newest), then Continue watching, Next up, My List, Featured for you
// (AI picks, while they're on), "Because you watched …" for what you finished most recently,
// and the latest shows, anime and movies. A row with nothing in it isn't there; a home with
// nothing at all says so, with a way to ask for something.

import { SignedOut, api, h, show } from "/web/lib.js";
import { generation, leaving } from "/web/page-state.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";
import { hero } from "/web/watch-hero.js";
import { row, watchingOf } from "/web/watch-tiles.js";

// Each row the server sends: its heading, whether its tiles are landscape, and where "See all"
// goes.
const ROWS = {
  continue_watching: { heading: "Continue watching", wide: true },
  next_up: { heading: "Next up", wide: true },
  new_episodes: { heading: "New episodes", wide: true },
  favourites: { heading: "My List", more: { href: "#/favourites", label: "All of My List" } },
  featured_for_you: { heading: "Featured for you" },
  latest_shows: { heading: "Latest shows", more: { href: "#/shows", label: "All shows" } },
  latest_anime: { heading: "Latest anime", more: { href: "#/anime", label: "All anime" } },
  latest_movies: { heading: "Latest movies", more: { href: "#/movies", label: "All movies" } },
  next_in_collection: { heading: "Next in the collection" },
  highly_rated: { heading: "Highly rated, not seen yet" },
  watch_again: { heading: "Watch again" },
};

// A home's rows as `/home` answers them, drawn under its hero `top`; `latestHeading` names the
// newest titles' row on a page of one kind, which has no link to itself.
export function homeRows(rows, top, watching, latestHeading = null) {
  return rows.map(({ key, seed, items }) => {
    // One for each title you finished, so each has its own id.
    if (key === "because_you_watched" && seed) {
      return row(`because-${seed.kind}-${seed.id}`, `Because you watched ${seed.title}`, items, { spotlight: top.spotlight, watching });
    }
    const about = ROWS[key];
    if (!about) return null;
    const latest = latestHeading && key.startsWith("latest_");
    return row(key, latest ? latestHeading : about.heading, items, { wide: about.wide, more: latest ? null : about.more, spotlight: top.spotlight, watching });
  });
}

export async function home(gen) {
  show(
    "Home",
    h("h1", { class: "visually-hidden" }, "Home"),
    h("div", { class: "hero-placeholder placeholder", "aria-hidden": "true" }),
    placeholders("your home", { kind: "cards", count: 6 }),
  );
  let answer;
  try {
    answer = await api("/home");
  } catch (err) {
    if (err instanceof SignedOut || gen !== generation) throw err;
    show("Home", h("h1", {}, "Home"), failed("your home", err, () => home(generation)));
    return;
  }
  if (gen !== generation) return;
  if (!answer.hero.length && !answer.rows.length) {
    show(
      "Home",
      h("h1", {}, "Home"),
      emptyState("Nothing to watch yet. Titles show up here once the libraries are scanned, and anything you ask for once it's in.", {
        href: "#/requests/ask",
        label: "Ask for something",
      }),
    );
    return;
  }
  const watching = watchingOf(answer.rows.filter((r) => ["continue_watching", "next_up", "new_episodes"].includes(r.key)).flatMap((r) => r.items));
  const top = hero(answer.hero, { watching });
  leaving.push(top.stop);
  top.spotlight.add(answer.hero);
  const rows = homeRows(answer.rows, top, watching);
  show("Home", h("h1", { class: "visually-hidden" }, "Home"), top.el, h("div", { class: "watch-rows" }, rows));
}
