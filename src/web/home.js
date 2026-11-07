// Watch's home, from `/home` in one request: the hero stepping between picks (what you're
// partway through, then the newest), then Continue watching, Next up, My List, Featured for you
// (AI picks, while they're on), "Because you watched …" for what you finished most recently,
// and the latest shows, anime and movies. A row with nothing in it isn't there; a home with
// nothing at all says so, with a way to ask for something.
//
// The last answer is kept in this browser under the user's id and drawn at once next time,
// then replaced when `/home` answers (if it changed); a failed refresh leaves it on screen.
// Signing out, or a session found ended, forgets every saved copy (forgetSavedHomes), so none
// is ever drawn for someone else.
//
// It follows the server's live updates (watch-live.js): when a title comes or goes, or My List
// changes (on any device), the rows are drawn again in place, the hero left as it is.

import { SignedOut, api, h, show } from "/web/lib.js";
import { generation, leaving, signIn } from "/web/page-state.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";
import { hero } from "/web/watch-hero.js";
import { followWatch, keepPlace } from "/web/watch-live.js";
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

const SAVED = "melete.home.";

// The copy of `/home` kept for `user` (their id), or null; storage can be off or full.
function savedHome(user) {
  try {
    return JSON.parse(localStorage.getItem(SAVED + user) ?? "null");
  } catch {
    return null;
  }
}

function keepHome(user, answer) {
  try {
    localStorage.setItem(SAVED + user, JSON.stringify(answer));
  } catch {
    // Storage off or full: Home just waits for the server next time.
  }
}

// Forgets every saved Home in this browser, whoever's: on signing out or a session found ended.
export function forgetSavedHomes() {
  try {
    for (const key of Object.keys(localStorage)) if (key.startsWith(SAVED)) localStorage.removeItem(key);
  } catch {
    // Storage off: nothing was kept.
  }
}

// Home for `user` (their id): the saved copy at once if there is one, then the server's.
export async function home(gen, user) {
  // What's drawn: the hero (null before it's drawn, or with nothing to show) and its rows' box.
  const page = { gen, user, top: null, rows: null };
  const saved = savedHome(user);
  if (saved) draw(page, saved);
  else {
    show(
      "Home",
      h("h1", { class: "visually-hidden" }, "Home"),
      h("div", { class: "hero-placeholder placeholder", "aria-hidden": "true" }),
      placeholders("your home", { kind: "cards", count: 6 }),
    );
  }
  const again = () => load(page, true);
  followWatch(gen, { library: again, favourites: again });
  await load(page, false, saved);
}

// Asks for the home and draws it; `again` for a live update, which leaves the page as it is
// when the answer can't be had. After a saved copy, the answer is drawn whole (unless it's the
// same), keeping the keyboard on the same link.
async function load(page, again, saved = null) {
  let answer;
  try {
    answer = await api("/home");
  } catch (err) {
    if (page.gen !== generation) return;
    if (again) return err instanceof SignedOut ? signIn("Your session ended. Sign in again.") : undefined;
    if (err instanceof SignedOut) throw err;
    // A saved copy stays on screen rather than giving way to an error.
    if (!saved) show("Home", h("h1", {}, "Home"), failed("your home", err, () => load(page, false)));
    return;
  }
  if (page.gen !== generation) return;
  keepHome(page.user, answer);
  if (!saved) return draw(page, answer);
  if (JSON.stringify(saved) === JSON.stringify(answer)) return;
  // Anyone already on a link of the saved copy stays on the same one in the fresh.
  const on = document.activeElement?.closest?.("#main a[href], #main button");
  const same = (el) => el.getAttribute("href") === on.getAttribute("href") && el.className === on.className && el.textContent === on.textContent;
  page.top?.stop();
  page.top = null;
  draw(page, answer);
  if (on) [...document.querySelectorAll("#main a[href], #main button")].find(same)?.focus({ preventScroll: true });
}

function draw(page, answer) {
  const watching = watchingOf(answer.rows.filter((r) => ["continue_watching", "next_up", "new_episodes"].includes(r.key)).flatMap((r) => r.items));
  // A live update with a hero already up: the rows again, in place.
  if (page.top && answer.hero.length) {
    keepPlace(page.rows, homeRows(answer.rows, page.top, watching).filter(Boolean));
    return;
  }
  page.top?.stop();
  page.top = null;
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
  const top = hero(answer.hero, { watching });
  leaving.push(top.stop);
  top.spotlight.add(answer.hero);
  page.top = top;
  page.rows = h("div", { class: "watch-rows" }, homeRows(answer.rows, top, watching));
  show("Home", h("h1", { class: "visually-hidden" }, "Home"), top.el, page.rows);
}
