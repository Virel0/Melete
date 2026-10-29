// My List: the household's name for favourites, kept on the server for every device. The
// toggle on a title's page and the hero, and the page listing them all.

import { SignedOut, announce, api, h, plural, show } from "/web/lib.js";
import { generation, signIn } from "/web/page-state.js";
import { art } from "/web/cards.js";
import { icon } from "/web/music-controls.js";
import { emptyState, failed, placeholders } from "/web/page-parts.js";
import { animeIds, titleHref } from "/web/watch-play.js";

const path = (kind, id) => `/me/favourites/${kind === "movie" ? "movies" : "series"}/${id}`;

// My List on a title: a toggle that can be pointed at one title after another (the hero's
// picks), reading each one's state once. Returns { el, show(item) }; `item` has `kind`
// ("movie" or "series"), `id` and `title`.
export function myListToggle() {
  const mark = h("span", { class: "mark" }, icon("plus"));
  const button = h("button", { type: "button", class: "quiet my-list", "aria-pressed": "false", disabled: true }, mark, "My List");
  const states = new Map();
  let item = null;
  const key = (i) => `${i.kind}:${i.id}`;
  const draw = () => {
    const on = item ? states.get(key(item)) : undefined;
    button.disabled = on === undefined;
    button.setAttribute("aria-pressed", String(Boolean(on)));
    mark.replaceChildren(icon(on ? "check" : "plus"));
  };
  const read = (wanted) =>
    api(path(wanted.kind, wanted.id))
      .then((state) => {
        states.set(key(wanted), state.favourite);
        if (item === wanted) draw();
      })
      .catch((err) => {
        if (err instanceof SignedOut) signIn("Your session ended. Sign in again.");
      });
  button.addEventListener("click", async () => {
    const which = item;
    const on = states.get(key(which));
    button.disabled = true;
    try {
      const state = await api(path(which.kind, which.id), { method: on ? "DELETE" : "PUT" });
      states.set(key(which), state.favourite);
      announce(state.favourite ? `${which.title} is on My List.` : `${which.title} is off My List.`);
    } catch (err) {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      announce(`Couldn't change My List. ${err.message}`);
    }
    if (item === which) draw();
  });
  return {
    el: button,
    show(next) {
      item = next;
      draw();
      if (!states.has(key(next))) read(next);
    },
  };
}

// The page's parts: All, then each kind, anime apart from the shows.
const KINDS = [
  ["", "All"],
  ["movies", "Movies"],
  ["shows", "Shows"],
  ["anime", "Anime"],
];

// Every title on My List, newest first, narrowed to a kind; each with Remove, and Undo after.
export async function favouritesPage(params, gen) {
  const kind = KINDS.some(([k]) => k === params.get("kind")) ? params.get("kind") : "";
  show("My List", h("h1", {}, "My List"), placeholders("My List", { kind: "cards", count: 12 }));
  let list;
  let anime;
  try {
    [list, anime] = await Promise.all([api("/me/favourites?limit=500"), animeIds()]);
  } catch (err) {
    if (err instanceof SignedOut || gen !== generation) throw err;
    show("My List", h("h1", {}, "My List"), failed("My List", err, () => favouritesPage(params, generation)));
    return;
  }
  if (gen !== generation) return;
  const kindOf = (item) => (item.kind === "movie" ? "movies" : anime.has(item.id) ? "anime" : "shows");
  const counts = new Map(KINDS.map(([k]) => [k, k ? list.items.filter((i) => kindOf(i) === k).length : list.items.length]));
  const items = list.items.filter((i) => !kind || kindOf(i) === kind);
  const status = h("p", { class: "status", role: "status" }, items.length ? plural(items.length, "title", "titles") : "");
  const grid = h("ul", { class: "grid my-list-grid" });
  grid.replaceChildren(...items.map((item) => listCard(item, { grid, status })));
  // Kinds with nothing on the list aren't offered, except the one asked for.
  const tabs = h(
    "nav",
    { class: "tabs", "aria-label": "Kind" },
    KINDS.filter(([k]) => !k || counts.get(k) || k === kind).map(([k, label]) =>
      h("a", { href: `#/favourites${k ? `?kind=${k}` : ""}`, "aria-current": k === kind ? "page" : null }, label),
    ),
  );
  show(
    "My List",
    h("h1", {}, "My List"),
    list.items.length ? tabs : null,
    items.length
      ? [status, grid]
      : emptyState(
          list.items.length ? "Nothing of this kind on My List." : "Nothing on My List yet. Use My List on a title to keep it here, on every device.",
          { href: "#/", label: "Find something to watch" },
        ),
  );
}

// A title on My List: its card (its page when it's in the library) and Remove. Removing takes
// it off the page, says so, and offers Undo.
function listCard(item, { grid, status }) {
  const facts = [h("div", { class: "title" }, item.title), h("div", { class: "meta" }, item.present ? (item.year ? String(item.year) : "") : "Not in the library now")];
  const card = item.present ? h("a", { class: "card", href: titleHref(item) }, art(item.poster), facts) : h("div", { class: "card" }, art(item.poster), facts);
  const remove = h("button", { type: "button", class: "quiet", "aria-label": `Remove ${item.title} from My List` }, "Remove");
  const li = h("li", { class: "my-list-item" }, card, remove);
  remove.addEventListener("click", async () => {
    remove.disabled = true;
    try {
      await api(path(item.kind, item.id), { method: "DELETE" });
    } catch (err) {
      if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
      remove.disabled = false;
      return announce(`Couldn't remove ${item.title}. ${err.message}`);
    }
    // Focus moves to the next title's Remove (or the one before), not off the page.
    const next = li.nextElementSibling ?? li.previousElementSibling;
    const at = [...grid.children].indexOf(li);
    li.remove();
    const undo = h("button", { type: "button", class: "link" }, "Undo");
    undo.addEventListener("click", async () => {
      undo.disabled = true;
      try {
        await api(path(item.kind, item.id), { method: "PUT" });
      } catch (err) {
        if (err instanceof SignedOut) return signIn("Your session ended. Sign in again.");
        undo.disabled = false;
        return announce(`Couldn't put ${item.title} back. ${err.message}`);
      }
      const back = listCard(item, { grid, status });
      grid.insertBefore(back, grid.children[at] ?? null);
      status.replaceChildren(`${item.title} is back on My List.`);
      back.querySelector("button").focus();
    });
    status.replaceChildren(`Removed ${item.title} from My List. `, undo);
    (next?.querySelector("button") ?? undo).focus();
  });
  return li;
}
