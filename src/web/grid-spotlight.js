// The spotlight beside a grid of titles (All shows, All anime, All movies, My List), on screens
// wide enough for it: a panel that stays in view while the grid scrolls, showing the card pointed
// at or focused (its backdrop, logo or title, facts and summary), as Mneme's tile spotlight does.
// On narrower screens there's no room beside the grid, so there's no panel and nothing is looked
// up; the cards say their title and year as they always have.
//
// List items carry no backdrop or summary, so a card's title is looked up (`/movies/{id}`,
// `/series/{id}`) once it has been held for the follow delay, one at a time and kept for the
// page's life: sweeping the pointer across the grid, or scrolling under it, asks for nothing.
//
// spotlit(grid) → the grid with the panel beside it, to put where the grid went.

import { SignedOut, api, h, runtime } from "/web/lib.js";
import { leaving, signIn } from "/web/page-state.js";

// The hero's timing: follow a card after 160 ms, so sweeping across the grid changes nothing.
const FOLLOW_MS = 160;
// Pointer moves this soon after a scroll are the page moving under the pointer, not a choice.
const SCROLL_QUIET_MS = 200;
const wide = matchMedia("(min-width: 75rem)");

// Each title looked up, by "movies:5" or "series:7": a promise of its details, or of null.
const looked = new Map();

function lookUp(key) {
  if (!looked.has(key)) {
    const [kind, id] = key.split(":");
    looked.set(
      key,
      api(`/${kind}/${id}`).catch((err) => {
        looked.delete(key);
        if (err instanceof SignedOut) signIn("Your session ended. Sign in again.");
        return null;
      }),
    );
  }
  return looked.get(key);
}

// The card's title, as "movies:5", from its link; null for a card that isn't a title's.
function keyOf(card) {
  const found = card?.getAttribute("href")?.match(/^#\/(movies|series)\/(\d+)$/);
  return found ? `${found[1]}:${found[2]}` : null;
}

// Kind, year, two genres and a movie's length.
function facts(kind, title) {
  return [
    kind === "movies" ? "Movie" : "Series",
    title.year ? String(title.year) : null,
    title.genres?.slice(0, 2).join(", ") || null,
    kind === "movies" ? runtime(title.runtime_minutes) : null,
  ].filter(Boolean);
}

export function spotlit(grid) {
  const art = h("img", { alt: "", class: "spotlight-picture" });
  const name = h("h3", { class: "spotlight-title" });
  const meta = h("ul", { class: "meta-row spotlight-facts" });
  const logline = h("p", { class: "spotlight-logline" });
  const hint = h("p", { class: "spotlight-hint" }, "Point at a title, or move to one with the keyboard, to see more about it here.");
  const panel = h(
    "aside",
    { class: "grid-spotlight", "aria-label": "Spotlight" },
    h("div", { class: "spotlight-art" }, art),
    h("div", { class: "spotlight-body" }, hint, name, meta, logline),
  );
  const s = { shown: null, wanted: null, asking: false, follow: null, byPointer: false, scrolled: 0 };

  const draw = (key, title) => {
    s.shown = key;
    hint.hidden = true;
    const back = title.textless_backdrop ?? title.backdrop;
    const focus = title.textless_backdrop ? title.textless_backdrop_focus : title.backdrop_focus;
    panel.classList.toggle("no-art", !back);
    art.classList.remove("shown");
    if (back) {
      art.style.setProperty("object-position", focus ? `${focus.x * 100}% ${focus.y * 100}%` : "50% 30%");
      art.addEventListener("load", () => art.classList.add("shown"), { once: true });
      art.src = back;
    } else art.removeAttribute("src");
    name.replaceChildren(title.logo ? h("img", { src: title.logo, alt: title.title, class: "spotlight-logo" }) : title.title);
    meta.replaceChildren(...facts(key.split(":")[0], title).map((f) => h("li", {}, f)));
    logline.textContent = title.overview ?? "";
  };

  // Shows the card wanted last, asking for one title at a time; one asked for meanwhile waits
  // its turn, and any passed over on the way are never asked for.
  const show = async () => {
    if (s.asking || !s.wanted || s.wanted === s.shown) return;
    const key = s.wanted;
    s.asking = true;
    const title = await lookUp(key);
    s.asking = false;
    // One that couldn't be had isn't asked for again until it's pointed at again.
    if (!title && key === s.wanted) s.wanted = null;
    if (title && key === s.wanted && panel.isConnected) draw(key, title);
    show();
  };

  const hold = (card, byPointer) => {
    clearTimeout(s.follow);
    const key = keyOf(card);
    if (!key || !wide.matches) return;
    s.byPointer = byPointer;
    s.follow = setTimeout(() => {
      s.wanted = key;
      show();
    }, FOLLOW_MS);
  };
  const cardOf = (event) => event.target.closest?.("a.card");
  grid.addEventListener("pointerover", (event) => {
    if (event.pointerType === "touch" || performance.now() - s.scrolled < SCROLL_QUIET_MS) return;
    if (cardOf(event)) hold(cardOf(event), true);
  });
  grid.addEventListener("pointerleave", () => clearTimeout(s.follow));
  // A card moved to with the keyboard is scrolled into view; that scroll doesn't undo it.
  grid.addEventListener("focusin", (event) => cardOf(event) && hold(cardOf(event), false));
  const scrolled = () => {
    s.scrolled = performance.now();
    if (s.byPointer) clearTimeout(s.follow);
  };
  addEventListener("scroll", scrolled, { passive: true });
  leaving.push(() => removeEventListener("scroll", scrolled));
  return h("div", { class: "spotlit" }, grid, panel);
}
